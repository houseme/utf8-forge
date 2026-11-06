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

//! Report-to-report comparison for the `diff` subcommand.
//!
//! Reads utf8-forge JSON reports that carry a `files[]` array and compares
//! per path: `added`, `removed`, and `changed` (action or byte-count drift).
//! A report without a `files` array is not a report and fails to parse, so a
//! mistyped input path can never masquerade as an empty baseline. Paths are
//! compared relative to each report's `root` (the root prefix is stripped,
//! separators normalized to `/`), so the same tree scanned under two
//! different roots diffs clean; a differing `root` field triggers a warning,
//! not an error. Unknown JSON fields are ignored (lenient deserialization).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Lenient view of one report row (also serialized into the diff output).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiffInputFile {
    pub path: String,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub bytes_before: Option<u64>,
    #[serde(default)]
    pub bytes_after: Option<u64>,
}

/// Lenient view of a whole report. `files` is required: any JSON object
/// without it (or a bare JSON array) is rejected as not-a-report.
#[derive(Debug, Deserialize)]
pub struct DiffInput {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
    pub files: Vec<DiffInputFile>,
}

/// Counts for the diff summary.
#[derive(Debug, Serialize)]
pub struct DiffSummary {
    pub added: u64,
    pub removed: u64,
    /// Common paths whose action or byte counts differ between the reports.
    pub action_changed: u64,
    pub unchanged: u64,
}

/// One common path whose record drifted.
#[derive(Debug, Serialize)]
pub struct ChangedEntry {
    pub path: String,
    pub before: DiffInputFile,
    pub after: DiffInputFile,
}

/// The `diff` output document (a new document type, not the run report).
#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub tool: &'static str,
    pub version: &'static str,
    pub mode: &'static str,
    pub before: String,
    pub after: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_mode: Option<String>,
    pub summary: DiffSummary,
    pub added: Vec<DiffInputFile>,
    pub removed: Vec<DiffInputFile>,
    pub changed: Vec<ChangedEntry>,
}

/// Rewrite each record path relative to the report's `root`: separators are
/// normalized to `/` and the root prefix (always with its trailing separator)
/// is stripped. Records whose path does not start with the root are kept
/// verbatim. Paths in the diff output are therefore root-relative and stable
/// across root spellings, which is what makes two scans of the same tree
/// under different roots comparable.
pub fn relativize_files(files: Vec<DiffInputFile>, root: Option<&str>) -> Vec<DiffInputFile> {
    let Some(root) = root else {
        return files;
    };
    let root_norm = root.replace('\\', "/");
    let prefix = if root_norm == "." {
        "./".to_string()
    } else if root_norm.ends_with('/') {
        root_norm
    } else {
        format!("{root_norm}/")
    };
    files
        .into_iter()
        .map(|mut f| {
            f.path = f.path.replace('\\', "/");
            if let Some(stripped) = f.path.strip_prefix(&prefix) {
                f.path = stripped.to_string();
            }
            f
        })
        .collect()
}

/// Compare two parsed reports. Duplicate paths within one report resolve to
/// the last occurrence (the tool never produces duplicates; sorted unique by
/// construction). Arrays are sorted by path.
pub fn compare(before: &DiffInput, after: &DiffInput) -> DiffReport {
    let index_before: HashMap<&str, &DiffInputFile> =
        before.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let index_after: HashMap<&str, &DiffInputFile> =
        after.files.iter().map(|f| (f.path.as_str(), f)).collect();

    let mut added: Vec<DiffInputFile> = Vec::new();
    let mut removed: Vec<DiffInputFile> = Vec::new();
    let mut changed: Vec<ChangedEntry> = Vec::new();
    let mut unchanged: u64 = 0;

    for (path, after_file) in &index_after {
        match index_before.get(path) {
            None => added.push((*after_file).clone()),
            Some(before_file) => {
                if before_file.action != after_file.action
                    || before_file.bytes_before != after_file.bytes_before
                    || before_file.bytes_after != after_file.bytes_after
                {
                    changed.push(ChangedEntry {
                        path: (*path).to_string(),
                        before: (*before_file).clone(),
                        after: (*after_file).clone(),
                    });
                } else {
                    unchanged += 1;
                }
            }
        }
    }
    for (path, before_file) in &index_before {
        if !index_after.contains_key(path) {
            removed.push((*before_file).clone());
        }
    }

    let sort_by_path = |files: &mut [DiffInputFile]| files.sort_by(|a, b| a.path.cmp(&b.path));
    sort_by_path(&mut added);
    sort_by_path(&mut removed);
    changed.sort_by(|a, b| a.path.cmp(&b.path));

    let summary = DiffSummary {
        added: added.len() as u64,
        removed: removed.len() as u64,
        action_changed: changed.len() as u64,
        unchanged,
    };
    DiffReport {
        tool: "utf8-forge",
        version: env!("CARGO_PKG_VERSION"),
        mode: "diff",
        before: String::new(),
        after: String::new(),
        before_root: before.root.clone(),
        after_root: after.root.clone(),
        before_mode: before.mode.clone(),
        after_mode: after.mode.clone(),
        summary,
        added,
        removed,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(files: &[(&str, &str)]) -> DiffInput {
        DiffInput {
            mode: Some("convert".into()),
            root: Some("/tmp/tree".into()),
            files: files
                .iter()
                .map(|(p, a)| DiffInputFile {
                    path: (*p).to_string(),
                    action: Some((*a).to_string()),
                    bytes_before: Some(10),
                    bytes_after: Some(12),
                })
                .collect(),
        }
    }

    #[test]
    fn partitions_added_removed_changed_unchanged() {
        let before = report(&[("x.cpp", "converted"), ("y.cpp", "unchanged")]);
        let after = report(&[("y.cpp", "unchanged"), ("z.cpp", "converted")]);
        let doc = compare(&before, &after);
        assert_eq!(doc.summary.added, 1);
        assert_eq!(doc.summary.removed, 1);
        assert_eq!(doc.summary.action_changed, 0);
        assert_eq!(doc.summary.unchanged, 1);
        assert_eq!(doc.removed[0].path, "x.cpp");
        assert_eq!(doc.added[0].path, "z.cpp");
    }

    #[test]
    fn byte_count_drift_counts_as_changed() {
        let mut before = report(&[("a.cpp", "converted")]);
        before.files[0].bytes_after = Some(100);
        let after = report(&[("a.cpp", "converted")]);
        let doc = compare(&before, &after);
        assert_eq!(doc.summary.action_changed, 1);
    }

    #[test]
    fn identical_reports_are_all_unchanged() {
        let before = report(&[("a.cpp", "converted")]);
        let after = report(&[("a.cpp", "converted")]);
        let doc = compare(&before, &after);
        assert_eq!(doc.summary.action_changed, 0);
        assert_eq!(doc.summary.unchanged, 1);
    }

    #[test]
    fn same_file_twice_resolves_to_last_occurrence() {
        // The first occurrence says "converted"; the last says "unchanged".
        // Last-match-wins must resolve the record to "unchanged" and match
        // the after side exactly.
        let before = DiffInput {
            mode: None,
            root: None,
            files: vec![
                DiffInputFile {
                    path: "a".into(),
                    action: Some("converted".into()),
                    bytes_before: Some(10),
                    bytes_after: Some(99),
                },
                DiffInputFile {
                    path: "a".into(),
                    action: Some("unchanged".into()),
                    bytes_before: Some(10),
                    bytes_after: Some(12),
                },
            ],
        };
        let after = report(&[("a", "unchanged")]);
        let doc = compare(&before, &after);
        assert_eq!(doc.summary.unchanged, 1);
        assert_eq!(doc.summary.action_changed, 0);
    }

    #[test]
    fn missing_optional_fields_are_tolerated() {
        let raw = r#"{"files": [{"path": "a.cpp"}]}"#;
        let input: DiffInput = serde_json::from_str(raw).unwrap();
        assert_eq!(input.files[0].path, "a.cpp");
        assert!(input.files[0].action.is_none());
    }

    #[test]
    fn json_without_files_array_is_rejected() {
        // Anything that is not a utf8-forge report must fail loudly instead
        // of behaving like an empty baseline.
        assert!(serde_json::from_str::<DiffInput>(r#"{"schema": "wrong"}"#).is_err());
        assert!(serde_json::from_str::<DiffInput>("[]").is_err());
        assert!(serde_json::from_str::<DiffInput>(r#"{"files": "nope"}"#).is_err());
    }

    #[test]
    fn root_prefix_is_stripped_from_record_paths() {
        let files: Vec<DiffInputFile> = vec![
            file("tree/a.cpp"),
            file("./b.h"),
            file("outside/c.txt"),
            file(r"tree\sub\d.rs"),
        ];
        let rel = relativize_files(files, Some("tree"));
        let paths: Vec<&str> = rel.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.cpp", "./b.h", "outside/c.txt", "sub/d.rs"]);
    }

    #[test]
    fn dot_root_strips_leading_dot_slash() {
        let rel = relativize_files(vec![file("./a.cpp")], Some("."));
        assert_eq!(rel[0].path, "a.cpp");
    }

    #[test]
    fn missing_root_keeps_paths_untouched() {
        let rel = relativize_files(vec![file("tree/a.cpp")], None);
        assert_eq!(rel[0].path, "tree/a.cpp");
    }

    fn file(path: &str) -> DiffInputFile {
        DiffInputFile {
            path: path.to_string(),
            action: Some("unchanged".into()),
            bytes_before: Some(1),
            bytes_after: Some(1),
        }
    }
}
