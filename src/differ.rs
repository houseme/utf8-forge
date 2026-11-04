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
//! Reads any utf8-forge JSON report that carries a `files[]` array and
//! compares per path: `added`, `removed`, and `changed` (action or byte-count
//! drift). Paths are compared as exact strings — both reports must come from
//! the same `--root` and the same platform's path spelling; a differing
//! `root` field triggers a warning, not an error. Input reports are data, not
//! configuration, so unknown JSON fields are ignored (lenient
//! deserialization).

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

/// Lenient view of a whole report.
#[derive(Debug, Deserialize)]
pub struct DiffInput {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
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
    pub before_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_mode: Option<String>,
    pub summary: DiffSummary,
    pub added: Vec<DiffInputFile>,
    pub removed: Vec<DiffInputFile>,
    pub changed: Vec<ChangedEntry>,
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
}
