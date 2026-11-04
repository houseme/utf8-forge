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

//! File-selection integration tests: whitelist + blacklist double gate,
//! traversal pruning, hidden files, globs, size cap.

use std::fs;
use std::path::Path;

use utf8_forge::filter::{FilterConfig, FilterOptions, Glob, extension_of};
use utf8_forge::walker::collect_files;

/// Write bytes to `root/rel`, creating parent directories.
fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, bytes).unwrap();
}

/// Walk a fresh filter config over the tree, returning sorted file names
/// relative to `root`.
fn walk(root: &Path, opts: &FilterOptions) -> Vec<String> {
    let cfg = FilterConfig::build(opts).unwrap();
    let res = collect_files(root, &cfg, &[], None);
    res.files
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn default_opts() -> FilterOptions {
    FilterOptions::default()
}

/// Standard tree exercising every gate.
fn build_tree(root: &Path) {
    put(root, "a.cpp", b"ok");
    put(root, "sub/b.h", b"ok");
    put(root, "dep/skip.cpp", b"must-not-touch");
    put(root, "deps/skip.cpp", b"must-not-touch");
    put(root, "third_party/lib/skip.cpp", b"must-not-touch");
    put(root, ".svn/entries.cpp", b"must-not-touch");
    put(root, ".git/x.cpp", b"must-not-touch");
    put(root, "node_modules/pkg/skip.cpp", b"must-not-touch");
    put(root, "__pycache__/x/skip.cpp", b"must-not-touch");
    put(root, "build/out/skip.cpp", b"must-not-touch");
    put(root, "OutputFile1/skip.cpp", b"must-not-touch");
    put(root, "OutputFiles/skip.cpp", b"must-not-touch");
    put(root, "bin/skip.cpp", b"must-not-touch");
    put(root, ".hidden.cpp", b"hidden-by-default");
    put(root, ".hidden/a.cpp", b"hidden-dir-by-default");
    put(root, "no_ext", b"no-extension");
    put(root, "not_whitelisted.png", b"binary");
    put(root, "NOT_LOWERCASE.CPP", b"uppercase-ext-ok");
}

#[test]
fn foo_png_is_not_whitelisted() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.png", b"x");
    put(dir.path(), "a.cpp", b"x");
    let files = walk(dir.path(), &default_opts());
    assert_eq!(files, vec!["a.cpp"]);
}

#[test]
fn extension_matching_is_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "FOO.CPP", b"x");
    put(dir.path(), "BAR.Hpp", b"x");
    let files = walk(dir.path(), &default_opts());
    assert_eq!(files, vec!["BAR.Hpp", "FOO.CPP"]);
}

#[test]
fn default_excluded_dirs_and_prefixes_are_pruned() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let files = walk(dir.path(), &default_opts());
    assert_eq!(files, vec!["NOT_LOWERCASE.CPP", "a.cpp", "sub/b.h"]);
}

#[test]
fn hidden_files_skipped_unless_include_hidden() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());

    let mut opts = default_opts();
    opts.include_hidden = true;
    let files = walk(dir.path(), &opts);
    // Dot-excluded dirs (.svn/.git) stay pruned even with --include-hidden —
    // the blacklist always applies.
    assert_eq!(
        files,
        vec![
            ".hidden/a.cpp",
            ".hidden.cpp",
            "NOT_LOWERCASE.CPP",
            "a.cpp",
            "sub/b.h"
        ]
    );
}

#[test]
fn opt_in_rc_and_msbuild() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "dlg.rc", b"x");
    put(dir.path(), "dlg.rc2", b"x");
    put(dir.path(), "ui.dlg", b"x");
    put(dir.path(), "proj.sln", b"x");
    put(dir.path(), "proj.vcxproj", b"x");
    put(dir.path(), "a.cpp", b"x");

    let files = walk(dir.path(), &default_opts());
    assert_eq!(files, vec!["a.cpp"]);

    let mut opts = default_opts();
    opts.include_rc = true;
    assert_eq!(
        walk(dir.path(), &opts),
        vec!["a.cpp", "dlg.rc", "dlg.rc2", "ui.dlg"]
    );

    let mut opts = default_opts();
    opts.include_msbuild = true;
    assert_eq!(
        walk(dir.path(), &opts),
        vec!["a.cpp", "proj.sln", "proj.vcxproj"]
    );
}

#[test]
fn ext_override_replaces_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"x");
    put(dir.path(), "b.h", b"x");
    put(dir.path(), "c.md", b"x");
    let mut opts = default_opts();
    opts.ext_override = vec!["md".into()];
    assert_eq!(walk(dir.path(), &opts), vec!["c.md"]);
}

#[test]
fn exclude_ext_removes_from_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"x");
    put(dir.path(), "b.h", b"x");
    let mut opts = default_opts();
    opts.exclude_ext = vec!["cpp".into()];
    assert_eq!(walk(dir.path(), &opts), vec!["b.h"]);
}

#[test]
fn user_exclude_dirs_appended_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "GameServer201/a.cpp", b"x");
    put(dir.path(), "test_client/b.cpp", b"x");
    put(dir.path(), "src/c.cpp", b"x");
    let mut opts = default_opts();
    opts.exclude_dirs = vec!["GameServer201".into(), "Test_Client".into()];
    assert_eq!(walk(dir.path(), &opts), vec!["src/c.cpp"]);
}

#[test]
fn exclude_glob_prunes_directories_and_files() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "GameServer/Generated/x.cpp", b"x");
    put(dir.path(), "Generated/y.cpp", b"x");
    put(dir.path(), "GameServer/keep.cpp", b"x");
    put(dir.path(), "keep.tmp.cpp", b"x");
    let mut opts = default_opts();
    opts.exclude_globs = vec!["**/Generated/**".into(), "**/*.tmp.*".into()];
    assert_eq!(walk(dir.path(), &opts), vec!["GameServer/keep.cpp"]);
}

#[test]
fn symlinked_directories_are_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"x");
    put(dir.path(), "real/b.cpp", b"x");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    }
    let files = walk(dir.path(), &default_opts());
    // `link/` (a symlink to a dir) is not followed; only real files are found.
    // (On non-unix the symlink isn't created and the assertion still holds.)
    assert_eq!(files, vec!["a.cpp", "real/b.cpp"]);
}

#[test]
fn symlinked_files_are_skipped() {
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "target.cpp", b"x");
        std::os::unix::fs::symlink(dir.path().join("target.cpp"), dir.path().join("alias.cpp"))
            .unwrap();
        let files = walk(dir.path(), &default_opts());
        assert_eq!(files, vec!["target.cpp"]);
    }
}

#[test]
fn leading_dot_ext_values_match() {
    let cfg = FilterConfig::build(&FilterOptions {
        ext_override: vec![".md".into(), ".Hpp".into()],
        ..Default::default()
    })
    .unwrap();
    assert!(cfg.extensions.contains("md"));
    assert!(cfg.extensions.contains("hpp"));
}

#[test]
fn star_run_collapse_is_semantically_equivalent() {
    // `***` collapses to `**` (multi-level); `a**b` collapses to `a*b`.
    let multi = Glob::compile("***/generated/**").unwrap();
    assert!(multi.matches(&["GameServer", "GENERATED", "x.cpp"]));
    let mixed = Glob::compile("a**b").unwrap();
    assert!(mixed.matches(&["axxxb"]));
}

#[test]
fn pathological_star_pattern_terminates_instantly() {
    // Before the step budget this ran for minutes (exponential backtracking).
    let g = Glob::compile("************zzzzzzz").unwrap();
    let name: String = "a".repeat(40);
    assert!(!g.matches(&[name.as_str()]));
}

#[test]
fn extension_of_lowercases_and_handles_multi_dot() {
    assert_eq!(extension_of(Path::new("a/b/c.CPP")), Some("cpp".into()));
    assert_eq!(extension_of(Path::new("x.Ts")), Some("ts".into()));
    assert_eq!(extension_of(Path::new("Makefile")), None);
}
