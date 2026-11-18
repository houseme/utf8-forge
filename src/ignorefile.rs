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

//! Gitignore-style exclusion files (`--ignore-file` / auto-discovered
//! `<root>/.utf8forgeignore`).
//!
//! Supported syntax (a deliberate subset, no escapes — a backslash is a
//! literal character):
//! - blank lines and `#` comments are skipped;
//! - a trailing `/` marks a dir-only pattern;
//! - a leading `/` (or any `/` inside the pattern) anchors it to the
//!   root-relative path; a pattern without `/` matches the basename at any
//!   depth;
//! - `!` negates; the LAST matching pattern wins (gitignore ordering);
//! - `**` spans path components, `*` / `?` match within one component;
//!   matching is case-insensitive like every other matcher in this tool.
//!
//! Limitation inherited from git: a file cannot be re-included (`!`) when its
//! parent directory is excluded — the walker prunes the subtree on the
//! directory verdict and children are never evaluated.

use std::fs;
use std::path::Path;

use crate::filter::Glob;

/// One compiled ignore-file line.
#[derive(Debug, Clone)]
pub struct IgnorePattern {
    /// Compiled matcher for the effective pattern (negation / dir marker
    /// stripped, leading `/` removed, lowercased, star-collapsed).
    glob: Glob,
    /// `!pattern` — flips the verdict when this is the last match.
    negated: bool,
    /// `pattern/` — applies to directories only.
    dir_only: bool,
    /// Pattern is anchored to the root-relative path (leading or inner `/`).
    anchored: bool,
}

/// A parsed ignore file: patterns in file order, last-match-wins.
#[derive(Debug, Clone, Default)]
pub struct IgnoreFile {
    patterns: Vec<IgnorePattern>,
}

/// Outcome of parsing one line: a compiled pattern, nothing (blank/comment),
/// or a rejection reason.
enum Line {
    Pattern(IgnorePattern),
    Skip,
    Invalid(String),
}

/// Parse one line per iteration, yielding the 1-based line number with its
/// outcome. Shared by the strict and lenient parsers so their grammars can
/// never drift apart.
fn parse_lines(text: &str) -> impl Iterator<Item = (usize, Line)> {
    text.lines().enumerate().map(|(idx, raw_line)| {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            return (idx + 1, Line::Skip);
        }
        let mut rest = line;
        let negated = rest.starts_with('!');
        if negated {
            rest = &rest[1..];
        }
        let dir_only = rest.ends_with('/');
        if dir_only {
            rest = rest.trim_end_matches('/');
        }
        let anchored = rest.contains('/');
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return (idx + 1, Line::Invalid("empty pattern".into()));
        }
        match Glob::compile_raw(rest) {
            Ok(glob) => (
                idx + 1,
                Line::Pattern(IgnorePattern {
                    glob,
                    negated,
                    dir_only,
                    anchored,
                }),
            ),
            Err(reason) => (idx + 1, Line::Invalid(reason)),
        }
    })
}

impl IgnoreFile {
    /// True when no patterns survived parsing — the lenient loader uses this
    /// to report that the ignore layer is inactive when every line was
    /// invalid.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Parse and compile the file at `path`. `Err` names the 1-based line
    /// number for invalid patterns (empty effective pattern, lone `/` or `!`,
    /// or the `[`/`{` constructs this grammar rejects).
    pub fn load(path: &Path) -> Result<IgnoreFile, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("cannot read ignore file `{}`: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// Parse ignore rules from raw text. Any invalid line fails the whole
    /// file — the posture for an explicitly named `--ignore-file`.
    pub fn parse(text: &str) -> Result<IgnoreFile, String> {
        let mut patterns = Vec::new();
        for (idx, line) in parse_lines(text) {
            match line {
                Line::Pattern(pattern) => patterns.push(pattern),
                Line::Invalid(reason) => {
                    return Err(format!("ignore file line {}: {reason}", idx));
                }
                Line::Skip => {}
            }
        }
        Ok(IgnoreFile { patterns })
    }

    /// Parse ignore rules from raw text, skipping invalid lines with a
    /// per-line warning instead of failing the whole file — the posture for
    /// the auto-discovered `<root>/.utf8forgeignore`, where one unsupported
    /// line (e.g. a gitignore `[Tt]est*` habit) must not silently disable
    /// every other rule the operator wrote. Returns the surviving rules and
    /// one warning per skipped line (1-based line number included).
    pub fn parse_lenient(text: &str) -> (IgnoreFile, Vec<String>) {
        let mut patterns = Vec::new();
        let mut warnings = Vec::new();
        for (idx, line) in parse_lines(text) {
            match line {
                Line::Pattern(pattern) => patterns.push(pattern),
                Line::Invalid(reason) => {
                    warnings.push(format!("ignore file line {idx} skipped: {reason}"));
                }
                Line::Skip => {}
            }
        }
        (IgnoreFile { patterns }, warnings)
    }

    /// Last-match-wins verdict for a root-relative path split into
    /// components. `true` = ignored, `false` = kept (including "no pattern
    /// matched").
    pub fn ignores(&self, rel: &[&str], is_dir: bool) -> bool {
        let mut verdict = false;
        for pat in &self.patterns {
            if pat.dir_only && !is_dir {
                continue;
            }
            // Gitignore anchoring: a pattern without any `/` matches the
            // basename at any depth; an anchored pattern matches the full
            // root-relative path.
            let candidate: Vec<&str> = if pat.anchored {
                rel.to_vec()
            } else {
                match rel.last() {
                    Some(base) => vec![base],
                    None => continue,
                }
            };
            if pat.glob.matches(&candidate) {
                verdict = !pat.negated;
            }
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ign(text: &str, path: &str, is_dir: bool) -> bool {
        let f = IgnoreFile::parse(text).unwrap();
        let comps: Vec<&str> = path.split('/').collect();
        f.ignores(&comps, is_dir)
    }

    #[test]
    fn dir_only_pattern_matches_any_depth() {
        assert!(ign("logs/", "logs", true));
        assert!(ign("logs/", "a/b/logs", true));
        assert!(!ign("logs/", "logs/a.cpp", false));
        assert!(!ign("logs/", "a.cpp", false));
    }

    #[test]
    fn basename_pattern_matches_at_any_depth() {
        assert!(ign("*.log", "x.log", false));
        assert!(ign("*.log", "a/b/x.log", false));
        assert!(!ign("*.log", "x.cpp", false));
    }

    #[test]
    fn anchored_pattern_matches_only_at_root() {
        assert!(ign("/Generated", "Generated", true));
        assert!(!ign("/Generated", "a/Generated", true));
        assert!(ign("build/*.cpp", "build/x.cpp", false));
        assert!(!ign("build/*.cpp", "a/build/x.cpp", false));
    }

    #[test]
    fn negation_is_last_match_wins() {
        let text = "*.log\n!keep.log";
        assert!(ign(text, "a/drop.log", false));
        assert!(!ign(text, "a/keep.log", false));
        let text = "build/\n!build/";
        assert!(!ign(text, "build", true), "negated dir is traversable");
    }

    #[test]
    fn parent_dir_exclusion_cannot_be_negated_for_children() {
        // Documented git parity: children under an ignored dir are never
        // re-included — the walker prunes on the dir verdict.
        let text = "build/\n!build/keep.cpp";
        assert!(ign(text, "build", true));
    }

    #[test]
    fn comments_blank_lines_and_crlf() {
        let f = IgnoreFile::parse("# comment\r\n\r\n*.tmp\r\n").unwrap();
        // The comment line produced no pattern: a non-matching path stays
        // kept while the *.tmp rule still applies.
        assert!(!f.ignores(&["keep.txt"], false));
        let comps: Vec<&str> = "x.tmp".split('/').collect();
        assert!(f.ignores(&comps, false));
    }

    #[test]
    fn empty_effective_patterns_are_line_numbered_errors() {
        assert!(IgnoreFile::parse("/").unwrap_err().contains("line 1"));
        assert!(IgnoreFile::parse("!").unwrap_err().contains("line 1"));
        assert!(
            IgnoreFile::parse("*.tmp\n!")
                .unwrap_err()
                .contains("line 2")
        );
    }

    #[test]
    fn backslash_is_a_literal_not_a_separator() {
        // compile_raw keeps `\` literal; this pattern can never match a
        // normal component, but it must COMPILE (no normalization).
        assert!(IgnoreFile::parse("a\\b").is_ok());
        assert!(!ign("a\\b", "a/b", false));
    }

    #[test]
    fn negation_and_dir_only_patterns_honor_crlf() {
        let text = "*.log\r\n!keep.log\r\n";
        assert!(ign(text, "drop.log", false));
        assert!(!ign(text, "keep.log", false));
        assert!(ign("logs/\r\n", "logs", true));
        assert!(!ign("logs/\r\n", "logs/a.cpp", false));
    }

    #[test]
    fn double_star_spans_components() {
        assert!(ign("**/Generated/**", "a/Generated/x.cpp", false));
        assert!(ign("**/Generated/**", "Generated/y.cpp", false));
    }

    #[test]
    fn strict_parse_rejects_bracket_constructs() {
        // The grammar has no character classes; an explicit --ignore-file
        // must fail loudly instead of compiling the line as a literal.
        let err = IgnoreFile::parse("[Tt]est*").unwrap_err();
        assert!(err.contains("line 1"), "{err}");
        assert!(err.contains("grammar"), "{err}");
    }

    #[test]
    fn lenient_parse_keeps_surviving_lines_and_names_skipped_ones() {
        let text = "skipme/\n[Tt]est*\n*.tmp\n!";
        let (rules, warnings) = IgnoreFile::parse_lenient(text);
        // skipme/ and *.tmp survive; the bracket line and the lone ! are
        // skipped with line-numbered warnings.
        assert!(rules.ignores(&["skipme"], true));
        assert!(rules.ignores(&["x.tmp"], false));
        assert!(!rules.ignores(&["tEst.txt"], false));
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("line 2"), "{warnings:?}");
        assert!(warnings[0].contains("grammar"), "{warnings:?}");
        assert!(warnings[1].contains("line 4"), "{warnings:?}");
    }

    #[test]
    fn lenient_parse_of_a_fully_valid_file_has_no_warnings() {
        let (rules, warnings) = IgnoreFile::parse_lenient("# c\n\nlogs/\n*.tmp\n");
        assert!(rules.ignores(&["logs"], true));
        assert!(warnings.is_empty());
    }
}
