// Copyright 2026 houseme
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! File selection: extension whitelist + directory blacklist double gate.
//!
//! Both gates always apply. An extension must be whitelisted AND the path
//! must not hit an excluded directory name, prefix rule, or glob — this is
//! what protects `dep/`, binaries, and build outputs from modification.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;

/// Default extension whitelist (source code + config/data + docs).
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    // C / C++
    "c", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "h++", "inl", "ipp", "tpp", "ixx", "m",
    "mm", // .NET / JVM
    "cs", "csx", "java", "kt", "kts", // Other languages
    "rs", "go", "py", "pyi", "rb", "php", "pl", "pm", "lua", "js", "jsx", "ts", "tsx", "mjs",
    "cjs", // Shell
    "sh", "bash", "zsh", "ps1", "bat", "cmd", // Data / config
    "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg", "conf", "xml", "csv", "tsv", "sql",
    // Docs / protocol
    "md", "markdown", "txt", "rst", "proto",
];

/// Opt-in resource-script extensions (`--include-rc`). rc.exe has limited
/// UTF-8 support, so these stay off the default whitelist.
pub const RC_EXTENSIONS: &[&str] = &["rc", "rc2", "dlg"];

/// Opt-in MSBuild / Visual Studio extensions (`--include-msbuild`). VS
/// re-saves these with a BOM, so they stay off the default whitelist.
pub const MSBUILD_EXTENSIONS: &[&str] = &[
    "sln", "vcxproj", "vcproj", "props", "targets", "filters", "user",
];

/// Directory names excluded by default, matched case-insensitively on the
/// directory name itself.
pub const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    "dep",
    "deps",
    "third_party",
    "third-party",
    "3rdparty",
    "vendor",
    "external",
    "extern",
    "_tools",
    "tools_bin",
    ".svn",
    ".git",
    ".hg",
    ".vs",
    ".idea",
    ".vscode",
    "build",
    "out",
    "bin",
    "obj",
    "debug",
    "release",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "temp",
    "tmp",
    "cache",
];

/// Directory-name prefixes excluded by default (`OutputFile*` /
/// `OutputFiles*`).
pub const DEFAULT_EXCLUDE_PREFIXES: &[&str] = &["outputfile"];

/// Raw filter inputs, normalized by [`FilterConfig::build`].
#[derive(Debug, Clone, Default)]
pub struct FilterOptions {
    /// Overrides the default whitelist entirely when non-empty.
    pub ext_override: Vec<String>,
    /// Appended removals from the effective whitelist.
    pub exclude_ext: Vec<String>,
    /// Extra excluded directory names (appended to the defaults).
    pub exclude_dirs: Vec<String>,
    /// Extra glob patterns matched against root-relative paths.
    pub exclude_globs: Vec<String>,
    /// Adds `RC_EXTENSIONS` to the default whitelist.
    pub include_rc: bool,
    /// Adds `MSBUILD_EXTENSIONS` to the default whitelist.
    pub include_msbuild: bool,
    /// Processes dot-prefixed files/dirs (default excludes still apply).
    pub include_hidden: bool,
}

/// Compiled, effective filter configuration. The fields are the
/// post-normalization output of [`FilterConfig::build`] (lowercased,
/// validated, compiled) — constructing them by hand bypasses that
/// normalization, so always go through `build`.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    /// Effective extension whitelist (lowercased).
    pub extensions: HashSet<String>,
    /// Effective excluded directory names (lowercased, defaults + user).
    pub exclude_dirs: HashSet<String>,
    /// Effective excluded directory-name prefixes (lowercased).
    pub exclude_prefixes: Vec<String>,
    /// Compiled user glob patterns.
    pub exclude_globs: Vec<Glob>,
    /// Whether dot-prefixed entries are processed.
    pub include_hidden: bool,
}

impl FilterConfig {
    /// Build the effective config. Fatal-input problems (conflicting flags,
    /// malformed globs) return `Err` → CLI exit code 2.
    pub fn build(opts: &FilterOptions) -> Result<FilterConfig, String> {
        let exclude_ext: HashSet<String> = normalize_ext_list(&opts.exclude_ext)?;

        // Opt-in sets must not be simultaneously included and excluded; that
        // configuration is contradictory, so it is a fatal input error.
        if opts.include_rc && RC_EXTENSIONS.iter().any(|e| exclude_ext.contains(*e)) {
            return Err("--include-rc conflicts with --exclude-ext (rc/rc2/dlg)".to_string());
        }
        if opts.include_msbuild && MSBUILD_EXTENSIONS.iter().any(|e| exclude_ext.contains(*e)) {
            return Err(concat!(
                "--include-msbuild conflicts with --exclude-ext ",
                "(sln/vcxproj/vcproj/props/targets/filters/user)"
            )
            .to_string());
        }

        // `-e/--ext` overrides the default whitelist; opt-in flags only apply
        // to the default set. An explicit override that normalizes to nothing
        // (`--ext ""`) would silently scan zero files, so it is fatal.
        let mut extensions: HashSet<String> = if !opts.ext_override.is_empty() {
            let normalized = normalize_ext_list(&opts.ext_override)?;
            if normalized.is_empty() {
                return Err("empty --ext list: no usable extensions".to_string());
            }
            normalized
        } else {
            let mut base: HashSet<String> = DEFAULT_EXTENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            if opts.include_rc {
                base.extend(RC_EXTENSIONS.iter().map(|s| (*s).to_string()));
            }
            if opts.include_msbuild {
                base.extend(MSBUILD_EXTENSIONS.iter().map(|s| (*s).to_string()));
            }
            base
        };
        for e in &exclude_ext {
            extensions.remove(e);
        }

        let mut exclude_dirs: HashSet<String> = DEFAULT_EXCLUDE_DIRS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        exclude_dirs.extend(normalize_dir_list(&opts.exclude_dirs));

        let exclude_prefixes: Vec<String> = DEFAULT_EXCLUDE_PREFIXES
            .iter()
            .map(|s| (*s).to_string())
            .collect();

        let mut exclude_globs = Vec::new();
        for pattern in &opts.exclude_globs {
            match Glob::compile(pattern) {
                Ok(g) => exclude_globs.push(g),
                Err(e) => return Err(format!("invalid --exclude-glob `{pattern}`: {e}")),
            }
        }

        Ok(FilterConfig {
            extensions,
            exclude_dirs,
            exclude_prefixes,
            exclude_globs,
            include_hidden: opts.include_hidden,
        })
    }

    /// Case-insensitive directory-name / prefix exclusion check.
    pub fn is_excluded_dir_name(&self, name: &OsStr) -> bool {
        let n = name.to_string_lossy().to_ascii_lowercase();
        self.exclude_dirs.contains(&n) || self.exclude_prefixes.iter().any(|p| n.starts_with(p))
    }

    /// True when the dot-prefixed entry must be skipped.
    pub fn is_hidden_name(&self, name: &OsStr) -> bool {
        !self.include_hidden && name.to_string_lossy().starts_with('.')
    }

    /// True when any user glob matches the root-relative path components.
    pub fn is_excluded_by_glob(&self, rel_components: &[&str]) -> bool {
        self.exclude_globs.iter().any(|g| g.matches(rel_components))
    }
}

/// Lowercase + trim + drop empties for extension list values.
///
/// Exactly one leading dot is stripped so `--ext .cpp` matches the extension
/// `cpp` as the dot-less spelling does. A value that still starts with a dot
/// (`..h`) or contains a slash (`.cpp/`) can never match a real extension —
/// it would be a silent no-op indistinguishable from "no matching files" —
/// so those are fatal input errors instead.
fn normalize_ext_list(values: &[String]) -> Result<HashSet<String>, String> {
    let mut out = HashSet::new();
    for raw in values {
        let v = raw.trim();
        let v = v.strip_prefix('.').unwrap_or(v).to_ascii_lowercase();
        if v.starts_with('.') {
            return Err(format!(
                "invalid extension value `{raw}`: too many leading dots"
            ));
        }
        if v.contains('/') {
            return Err(format!("invalid extension value `{raw}`: contains `/`"));
        }
        if !v.is_empty() {
            out.insert(v);
        }
    }
    Ok(out)
}

/// Lowercase + trim + drop empties for directory-name list values.
/// Names are kept verbatim otherwise — `.cache` is a legitimate directory
/// name and must not lose its dot.
fn normalize_dir_list(values: &[String]) -> HashSet<String> {
    values
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Lowercased file extension of `path` (`foo.PNG` → `png`); `None` when the
/// file has no extension.
pub fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// A compiled `--exclude-glob` pattern.
///
/// Semantics: `/`-separated components; `**` matches zero or more path
/// components; `*` / `?` are wildcards inside one component. Matching is
/// case-insensitive (consistent with directory exclusion on Windows).
///
/// Star runs are collapsed at compile time (`***` → `**`, `a**b` → `a*b`) —
/// semantically identical since consecutive stars match the same text.
/// Iterative matchers retry only the latest wildcard, giving polynomial
/// worst-case work without recursion or a budget that can discard matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    parts: Vec<String>,
}

impl Glob {
    /// Compile `pattern`. Empty patterns are rejected. Backslashes are
    /// normalized to `/` (convenient for Windows-style `--exclude-glob`).
    pub fn compile(pattern: &str) -> Result<Glob, String> {
        Glob::compile_inner(pattern, true)
    }

    /// Like [`Glob::compile`], but a backslash is an ordinary literal character —
    /// the ignore-file grammar deliberately has no escape support, so `\`
    /// must not silently become `/` there.
    pub fn compile_raw(pattern: &str) -> Result<Glob, String> {
        Glob::compile_inner(pattern, false)
    }

    fn compile_inner(pattern: &str, normalize_backslash: bool) -> Result<Glob, String> {
        let normalized = if normalize_backslash {
            pattern.replace('\\', "/")
        } else {
            pattern.to_string()
        };
        let parts: Vec<String> = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| collapse_stars(&s.to_ascii_lowercase()))
            .collect();
        if parts.is_empty() {
            return Err("empty pattern".to_string());
        }
        Ok(Glob { parts })
    }

    /// Match a root-relative path split into components.
    pub fn matches(&self, components: &[&str]) -> bool {
        let lowered: Vec<String> = components.iter().map(|c| c.to_ascii_lowercase()).collect();
        let refs: Vec<&str> = lowered.iter().map(String::as_str).collect();
        match_parts(&self.parts, &refs)
    }
}

/// Collapse runs of `*` inside one component: all-star components become the
/// multi-level `**` (`***` ≡ `**`), mixed components keep a single `*`
/// (`a**b` ≡ `a*b`). Purely syntactic — no matching semantics change.
fn collapse_stars(component: &str) -> String {
    if component.chars().all(|c| c == '*') && component.len() >= 2 {
        return "**".to_string();
    }
    let mut out = String::with_capacity(component.len());
    let mut last_was_star = false;
    for c in component.chars() {
        if c == '*' {
            if !last_was_star {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        last_was_star = c == '*';
    }
    out
}

/// Match path components, letting the latest `**` consume another component
/// on a mismatch. Earlier wildcards need no retry once a later one is reached.
/// There are at most O(pattern components * path components) comparisons.
fn match_parts(pattern: &[String], path: &[&str]) -> bool {
    let mut pattern_index = 0;
    let mut path_index = 0;
    let mut last_star = None;

    while path_index < path.len() {
        let part = pattern.get(pattern_index);
        if part.is_some_and(|part| part == "**") {
            last_star = Some((pattern_index, path_index));
            pattern_index += 1;
        } else if part.is_some_and(|part| match_component(part, path[path_index])) {
            pattern_index += 1;
            path_index += 1;
        } else if let Some((star_index, matched_until)) = &mut last_star {
            *matched_until += 1;
            path_index = *matched_until;
            pattern_index = *star_index + 1;
        } else {
            return false;
        }
    }

    pattern[pattern_index..].iter().all(|part| part == "**")
}

/// Match `*` and `?` within one component using Unicode scalar values.
/// Retrying only the latest `*` bounds work by O(pattern chars * text chars).
fn match_component(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut pattern_index = 0;
    let mut text_index = 0;
    let mut last_star = None;

    while text_index < text.len() {
        let token = pattern.get(pattern_index);
        if token == Some(&'*') {
            last_star = Some((pattern_index, text_index));
            pattern_index += 1;
        } else if token.is_some_and(|&token| token == '?' || token == text[text_index]) {
            pattern_index += 1;
            text_index += 1;
        } else if let Some((star_index, matched_until)) = &mut last_star {
            *matched_until += 1;
            text_index = *matched_until;
            pattern_index = *star_index + 1;
        } else {
            return false;
        }
    }

    pattern[pattern_index..].iter().all(|&token| token == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_double_star_matches_nested_and_root() {
        let g = Glob::compile("**/Generated/**").unwrap();
        assert!(g.matches(&["GameServer", "Generated", "a.cpp"]));
        assert!(g.matches(&["Generated", "b.h"]));
        assert!(!g.matches(&["GameServer", "a.cpp"]));
        assert!(!g.matches(&["Generated.cpp"]));
    }

    #[test]
    fn glob_is_case_insensitive_and_normalizes_slashes() {
        let g = Glob::compile("**\\generated\\**").unwrap();
        assert!(g.matches(&["GameServer", "GENERATED", "x.cpp"]));
    }

    #[test]
    fn glob_component_wildcards() {
        let g = Glob::compile("**/*.tmp.*").unwrap();
        assert!(g.matches(&["a", "x.tmp.cpp"]));
        assert!(!g.matches(&["a", "y.cpp"]));
        let q = Glob::compile("??-gen").unwrap();
        assert!(q.matches(&["ab-gen"]));
        assert!(!q.matches(&["abc-gen"]));
    }

    #[test]
    fn glob_empty_pattern_rejected() {
        assert!(Glob::compile("").is_err());
        assert!(Glob::compile("///").is_err());
    }

    #[test]
    fn glob_retries_after_expensive_nonmatching_component() {
        let pattern = format!("**/{}b.cpp", "*a".repeat(8));
        let directory = "a".repeat(40);
        let glob = Glob::compile(&pattern).unwrap();
        // A shared recursion budget used to expire on the directory before
        // the matcher could reach the matching filename.
        assert!(glob.matches(&[&directory, "aaaaaaaab.cpp"]));
        assert!(!glob.matches(&[&directory, "aaaaaaaac.cpp"]));
    }

    #[test]
    fn glob_handles_long_literals_and_deep_paths_without_recursion() {
        let literal = "a".repeat(20_000);
        assert!(Glob::compile(&literal).unwrap().matches(&[&literal]));
        let components = vec!["a"; 10_000];
        let pattern = components.join("/");
        assert!(Glob::compile(&pattern).unwrap().matches(&components));
    }

    #[test]
    fn glob_preserves_unicode_and_raw_backslash_semantics() {
        let glob = Glob::compile("**/?文*.CPP").unwrap();
        assert!(glob.matches(&["目录", "中文本.cpp"]));
        assert!(!glob.matches(&["目录", "文.cpp"]));
        assert!(Glob::compile_raw("a\\?文").unwrap().matches(&["A\\中文"]));
        assert!(!Glob::compile_raw("a\\?文").unwrap().matches(&["a", "中文"]));
    }

    /// Enumerate small finite input spaces for the recursive test oracle.
    fn short_sequences<T: Clone>(alphabet: &[T], max_len: usize) -> Vec<Vec<T>> {
        let mut all = vec![Vec::new()];
        let mut level = vec![Vec::new()];
        for _ in 0..max_len {
            let mut next = Vec::new();
            for prefix in &level {
                for token in alphabet {
                    let mut sequence = prefix.clone();
                    sequence.push(token.clone());
                    next.push(sequence);
                }
            }
            all.extend(next.iter().cloned());
            level = next;
        }
        all
    }

    /// Exhaustive wildcard expansion is deliberately limited to tiny tests.
    fn reference_chars(pattern: &[char], text: &[char]) -> bool {
        match pattern.first() {
            None => text.is_empty(),
            Some('*') => {
                (0..=text.len()).any(|length| reference_chars(&pattern[1..], &text[length..]))
            }
            Some(token) => {
                text.first()
                    .is_some_and(|first| *token == '?' || token == first)
                    && reference_chars(&pattern[1..], &text[1..])
            }
        }
    }

    /// Independent component recursion used only for short generated paths.
    fn reference_parts(pattern: &[&str], path: &[&str]) -> bool {
        match pattern.first() {
            None => path.is_empty(),
            Some(&"**") => {
                (0..=path.len()).any(|length| reference_parts(&pattern[1..], &path[length..]))
            }
            Some(part) => {
                path.first().is_some_and(|component| {
                    reference_chars(
                        &part.chars().collect::<Vec<_>>(),
                        &component.chars().collect::<Vec<_>>(),
                    )
                }) && reference_parts(&pattern[1..], &path[1..])
            }
        }
    }

    #[test]
    fn component_matcher_agrees_with_exhaustive_short_input_oracle() {
        let patterns = short_sequences(&['a', 'b', '?', '*'], 5);
        let texts = short_sequences(&['a', 'b', '你'], 4);
        for pattern in &patterns {
            let pattern_string: String = pattern.iter().collect();
            for text in &texts {
                let text_string: String = text.iter().collect();
                assert_eq!(
                    match_component(&pattern_string, &text_string),
                    reference_chars(pattern, text),
                    "pattern={pattern_string:?}, text={text_string:?}"
                );
            }
        }
    }

    #[test]
    fn path_matcher_agrees_with_exhaustive_short_input_oracle() {
        let patterns = short_sequences(&["a", "b", "?", "a*", "**"], 3);
        let paths = short_sequences(&["a", "b", "aa", "你"], 3);
        for pattern in &patterns {
            let parts: Vec<String> = pattern.iter().map(|part| part.to_string()).collect();
            for path in &paths {
                assert_eq!(
                    match_parts(&parts, path),
                    reference_parts(pattern, path),
                    "pattern={pattern:?}, path={path:?}"
                );
            }
        }
    }

    #[test]
    fn build_default_whitelist_contains_cpp_not_rc() {
        let cfg = FilterConfig::build(&FilterOptions::default()).unwrap();
        assert!(cfg.extensions.contains("cpp"));
        assert!(cfg.extensions.contains("proto"));
        assert!(!cfg.extensions.contains("rc"));
        assert!(!cfg.extensions.contains("vcxproj"));
    }

    #[test]
    fn build_opt_in_flags_add_their_sets() {
        let cfg = FilterConfig::build(&FilterOptions {
            include_rc: true,
            include_msbuild: true,
            ..Default::default()
        })
        .unwrap();
        assert!(cfg.extensions.contains("rc"));
        assert!(cfg.extensions.contains("rc2"));
        assert!(cfg.extensions.contains("dlg"));
        assert!(cfg.extensions.contains("vcxproj"));
        assert!(cfg.extensions.contains("sln"));
    }

    #[test]
    fn build_ext_override_replaces_default_set() {
        let cfg = FilterConfig::build(&FilterOptions {
            ext_override: vec!["H".into(), "cpp".into()],
            include_rc: true,
            ..Default::default()
        })
        .unwrap();
        assert!(cfg.extensions.contains("h"));
        assert!(cfg.extensions.contains("cpp"));
        assert!(!cfg.extensions.contains("md"));
        // Override replaces the default set, so the opt-in flag is inert.
        assert!(!cfg.extensions.contains("rc"));
    }

    #[test]
    fn build_exclude_ext_removes_from_whitelist() {
        let cfg = FilterConfig::build(&FilterOptions {
            exclude_ext: vec!["CPP".into(), "".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(!cfg.extensions.contains("cpp"));
        assert!(cfg.extensions.contains("h"));
    }

    #[test]
    fn build_detects_documented_conflicts() {
        let err = FilterConfig::build(&FilterOptions {
            include_rc: true,
            exclude_ext: vec!["rc".into()],
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("--include-rc"));

        let err = FilterConfig::build(&FilterOptions {
            include_msbuild: true,
            exclude_ext: vec!["vcxproj".into()],
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("--include-msbuild"));
    }

    #[test]
    fn build_rejects_bad_glob() {
        // No bad glob can exist today (empty is the only failure), but the
        // plumbing must surface it as Err.
        let res = FilterConfig::build(&FilterOptions {
            exclude_globs: vec!["".into()],
            ..Default::default()
        });
        assert!(res.is_err());
    }

    #[test]
    fn excluded_dir_name_is_case_insensitive_with_prefix_rule() {
        let cfg = FilterConfig::build(&FilterOptions {
            exclude_dirs: vec!["GameServer201".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(cfg.is_excluded_dir_name(OsStr::new("dep")));
        assert!(cfg.is_excluded_dir_name(OsStr::new("DEP")));
        assert!(cfg.is_excluded_dir_name(OsStr::new(".svn")));
        assert!(cfg.is_excluded_dir_name(OsStr::new("OutputFile1")));
        assert!(cfg.is_excluded_dir_name(OsStr::new("OutputFiles")));
        assert!(cfg.is_excluded_dir_name(OsStr::new("gameserver201")));
        assert!(!cfg.is_excluded_dir_name(OsStr::new("src")));
        assert!(!cfg.is_excluded_dir_name(OsStr::new("temp.md")));
    }

    #[test]
    fn hidden_respects_include_flag() {
        let cfg = FilterConfig::build(&FilterOptions::default()).unwrap();
        assert!(cfg.is_hidden_name(OsStr::new(".hidden.cpp")));
        assert!(!cfg.is_hidden_name(OsStr::new("a.cpp")));
        let cfg = FilterConfig::build(&FilterOptions {
            include_hidden: true,
            ..Default::default()
        })
        .unwrap();
        assert!(!cfg.is_hidden_name(OsStr::new(".hidden.cpp")));
    }

    #[test]
    fn extension_of_lowercases() {
        assert_eq!(extension_of(Path::new("a.CPP")), Some("cpp".to_string()));
        assert_eq!(extension_of(Path::new("a.tar.gz")), Some("gz".to_string()));
        assert_eq!(extension_of(Path::new("Makefile")), None);
    }
}
