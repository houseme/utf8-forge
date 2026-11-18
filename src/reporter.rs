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

//! Console output + JSON reports.
//!
//! ```rust
//! use utf8_forge::reporter::rfc3339_from_secs;
//!
//! // Timestamps render as RFC 3339 UTC with a trailing `Z`.
//! assert_eq!(rfc3339_from_secs(0), "1970-01-01T00:00:00Z");
//! ```
//!
//! Report guarantees:
//! - `files[]` is sorted by `path`, so audits are reproducible run to run.
//! - Each mode serializes exactly its own summary fields via an untagged enum
//!   (`scan` distribution, `convert`/`fetch` counters, `verify` gate).
//!
//! # JSON report schema (summary)
//!
//! Top level: `tool`, `version`, `mode`, `root` (tree modes) or `url`
//! (fetch), `started_at`, `duration_ms`, `options`, `summary`, `files[]`,
//! `unknown_files`, `error_files`, and the omit-when-empty `warnings`,
//! `verify_failed_files`. The `summary` shape depends on `mode` (scan:
//! encoding buckets; convert/fetch and fix: outcome counters; verify: gate
//! counts). Every field of [`FileRecord`], [`OptionsBlock`], the four
//! summary structs, and [`Report`] itself documents its JSON name; all of
//! them implement both `Serialize` and `Deserialize`, so downstream tools can
//! read reports
//! back without hand-written mirrors. [`Report`]'s `Deserialize` dispatches
//! on `mode` — the per-mode summaries share field names, so parsing a report
//! requires knowing its mode.
//!
//! Per-file `action` vocabulary (open set across versions — new actions may
//! appear): [`FILE_ACTIONS`].

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Every per-file action the tool currently emits. New actions may appear in
/// future versions (treat the list as an open set); unknown actions in a
/// parsed report are not an error.
pub const FILE_ACTIONS: &[&str] = &[
    "would_convert",
    "converted",
    "unchanged",
    "skipped_unknown",
    "skipped_too_large",
    "error",
    "detected",
    "passed",
    "failed",
];

/// A single processed file in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    /// Detection tag (`utf-8`, `gb18030`, ...); `None` when unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<String>,
    /// One of [`FILE_ACTIONS`].
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_after: Option<u64>,
    pub duration_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Effective CLI options snapshot, embedded in every JSON report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsBlock {
    pub target: String,
    pub dry_run: bool,
    pub extensions: Vec<String>,
    pub exclude_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_ext: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<String>,
    /// Config file in effect (`--config` value verbatim); omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_file: Option<String>,
    /// Ignore rules in effect (explicit path or auto-discovered
    /// `<root>/.utf8forgeignore`); omitted when none are active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_file: Option<String>,
}

/// `scan` summary: per-encoding distribution counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanSummary {
    pub total_files: u64,
    pub candidates: u64,
    pub utf8: u64,
    pub utf8_bom: u64,
    pub utf16le: u64,
    pub utf16be: u64,
    pub utf32le: u64,
    pub utf32be: u64,
    pub gb18030: u64,
    pub unknown: u64,
    /// Files that could not be read (I/O errors); they carry no encoding
    /// bucket, so `buckets + errors == candidates` (equivalently
    /// `buckets + errors + too_large == total_files`).
    pub errors: u64,
    pub too_large: u64,
}

/// `convert` / `fetch` summary.
///
/// `already_utf8` counts files whose bytes were already byte-identical at the
/// selected target encoding (`action == "unchanged"`) — the name is
/// historical and matches the default UTF-8 target.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConvertSummary {
    pub total_files: u64,
    pub candidates: u64,
    pub already_utf8: u64,
    pub converted: u64,
    pub unknown: u64,
    pub decode_errors: u64,
    /// Content that decodes cleanly but cannot be encoded into the requested
    /// target without producing unreadable bytes (e.g. the UTF-16LE NUL
    /// guard); tracked separately from decode and I/O failures.
    pub encode_errors: u64,
    pub io_errors: u64,
    pub too_large: u64,
    pub fetch_error: u64,
}

/// `verify` summary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifySummary {
    pub total_files: u64,
    pub candidates: u64,
    pub passed: u64,
    pub failed: u64,
}

/// `fix` summary: convert counters plus the verify gate result on the same
/// tree. `fetch_error` is impossible in fix mode and is omitted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FixSummary {
    pub total_files: u64,
    pub candidates: u64,
    pub already_utf8: u64,
    pub converted: u64,
    pub unknown: u64,
    pub decode_errors: u64,
    /// Same meaning as the convert summary's `encode_errors`.
    pub encode_errors: u64,
    pub io_errors: u64,
    pub too_large: u64,
    /// Verify pass count on the same tree.
    pub passed: u64,
    /// Verify fail count on the same tree (the fix gate).
    pub failed: u64,
}

/// Mode-specific summary, serialized inline (untagged).
///
/// Standalone deserialization is refused on purpose: the verify and fix
/// shapes share field names and fix is a superset, so an untagged parse
/// would silently coerce a fix report into the verify summary. Parse the
/// full [`Report`] instead — its deserializer dispatches on `mode`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Summary {
    Scan(ScanSummary),
    Convert(ConvertSummary),
    Verify(VerifySummary),
    Fix(FixSummary),
}

/// Full JSON report document.
///
/// Implements the full round trip: [`Deserialize`] dispatches on `mode` to
/// pick the summary shape (the per-mode summaries share field names, so an
/// untagged parse would silently coerce a fix report into the verify
/// summary).
#[derive(Debug, Serialize)]
pub struct Report {
    pub tool: String,
    pub version: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub started_at: String,
    pub duration_ms: u64,
    pub options: OptionsBlock,
    pub summary: Summary,
    pub files: Vec<FileRecord>,
    pub unknown_files: Vec<String>,
    pub error_files: Vec<String>,
    /// Paths whose verify record is `failed` (fix mode only). Omitted when
    /// the gate is clean, so other modes stay schema-compatible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verify_failed_files: Vec<String>,
    /// Non-fatal traversal/ignore warnings (unreadable directories etc.).
    /// In `verify`/`fix` an unreadable subtree fails the gate, so a non-empty
    /// list there always accompanies exit code 3. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Deserialization intermediate for [`Report`]: the `summary` stays a raw
/// value until `mode` selects its shape. The per-mode summaries share field
/// names (`passed`/`failed` exist in both verify and fix), so an untagged
/// parse would silently coerce a fix report into the verify summary.
impl<'de> Deserialize<'de> for Report {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            // Everything here is required: the tool's own reports always
            // emit these fields. (A `default` attribute would add a 'static
            // bound this manually implemented trait cannot offer.)
            tool: String,
            version: String,
            mode: String,
            root: Option<String>,
            url: Option<String>,
            started_at: String,
            duration_ms: u64,
            options: Option<OptionsBlock>,
            summary: serde_json::Value,
            files: Vec<FileRecord>,
            unknown_files: Vec<String>,
            error_files: Vec<String>,
            // Omitted-when-empty in the emitted JSON; Option gives the same
            // leniency without the `default` attribute (which would add a
            // 'static bound this manually implemented trait cannot offer).
            verify_failed_files: Option<Vec<String>>,
            warnings: Option<Vec<String>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let summary = match raw.mode.as_str() {
            "scan" => Summary::Scan(
                serde_json::from_value(raw.summary).map_err(serde::de::Error::custom)?,
            ),
            "verify" => Summary::Verify(
                serde_json::from_value(raw.summary).map_err(serde::de::Error::custom)?,
            ),
            "fix" => {
                Summary::Fix(serde_json::from_value(raw.summary).map_err(serde::de::Error::custom)?)
            }
            // convert and fetch share the outcome-counter shape.
            _ => Summary::Convert(
                serde_json::from_value(raw.summary).map_err(serde::de::Error::custom)?,
            ),
        };
        Ok(Report {
            tool: raw.tool,
            version: raw.version,
            mode: raw.mode,
            root: raw.root,
            url: raw.url,
            started_at: raw.started_at,
            duration_ms: raw.duration_ms,
            options: raw.options.unwrap_or(OptionsBlock {
                target: String::new(),
                dry_run: false,
                extensions: Vec::new(),
                exclude_dirs: Vec::new(),
                exclude_ext: Vec::new(),
                exclude_globs: Vec::new(),
                backup_dir: None,
                config_file: None,
                ignore_file: None,
            }),
            summary,
            files: raw.files,
            unknown_files: raw.unknown_files,
            error_files: raw.error_files,
            verify_failed_files: raw.verify_failed_files.unwrap_or_default(),
            warnings: raw.warnings.unwrap_or_default(),
        })
    }
}

/// Format a unix timestamp as RFC 3339 UTC (`1970-01-01T00:00:00Z` style).
/// `chrono` is deliberately not a dependency; the civil-from-days algorithm
/// is well-known and unit-tested.
pub fn rfc3339_from_secs(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Current wall-clock time as RFC 3339 UTC.
pub fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339_from_secs(secs)
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Console verbosity settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleCfg {
    /// Errors only.
    pub quiet: bool,
    /// Per-file detail lines on stdout.
    pub verbose: bool,
}

/// Print the mode header line.
pub fn print_header(cfg: ConsoleCfg, line: String) {
    if !cfg.quiet {
        println!("[utf8-forge] {line}");
    }
}

/// Print a per-file verbose line (stdout, `-v` only).
pub fn print_verbose(cfg: ConsoleCfg, line: String) {
    if cfg.verbose && !cfg.quiet {
        println!("[utf8-forge] {line}");
    }
}

/// Print a per-file problem line (stderr, always shown).
pub fn print_problem(line: String) {
    eprintln!("[utf8-forge] {line}");
}

impl<'de> Deserialize<'de> for Summary {
    // See the type-level docs: without the report's `mode`, verify and fix
    // summaries cannot be told apart (fix is a field superset).
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "summary cannot be parsed standalone; deserialize the full Report",
        ))
    }
}

/// Serialize the report and write it next to a trailing newline.
pub fn write_report(report: &Report, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create report directory {}", parent.display()))?;
    }
    // Same crash-safety contract as source writes: temp file + rename, so a
    // crash mid-write leaves the previous report intact.
    let json = serde_json::to_string_pretty(report).context("report serialization failed")?;
    crate::writer::atomic_write(path, (json + "\n").as_bytes())
        .with_context(|| format!("cannot write report {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_timestamps() {
        assert_eq!(rfc3339_from_secs(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_secs(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(rfc3339_from_secs(86_400), "1970-01-02T00:00:00Z");
        // 2024-02-29 leap day: 2024-01-01 is day 19723.
        assert_eq!(rfc3339_from_secs(19_782 * 86_400), "2024-02-29T00:00:00Z");
        // Epoch day 20_000 = 2024-10-04.
        assert_eq!(rfc3339_from_secs(20_000 * 86_400), "2024-10-04T00:00:00Z");
    }

    #[test]
    fn report_round_trips_for_every_mode() {
        // Serialize → deserialize must reproduce each mode's report exactly:
        // the mode-dispatched summary parse is the acceptance test for the
        // untagged-summary hazard (a naive untagged parse would coerce fix
        // into verify).
        let files = vec![FileRecord {
            path: "a.cpp".into(),
            detected: Some("gb18030".into()),
            action: "converted".into(),
            bytes_before: Some(10),
            bytes_after: Some(12),
            duration_us: 42,
            error: None,
        }];
        let options = OptionsBlock {
            target: "utf-8".into(),
            dry_run: false,
            extensions: vec!["cpp".into()],
            exclude_dirs: Vec::new(),
            exclude_ext: Vec::new(),
            exclude_globs: Vec::new(),
            backup_dir: None,
            config_file: None,
            ignore_file: None,
        };
        let cases: Vec<(&str, Summary)> = vec![
            (
                "scan",
                Summary::Scan(ScanSummary {
                    total_files: 1,
                    candidates: 1,
                    gb18030: 1,
                    ..ScanSummary::default()
                }),
            ),
            (
                "convert",
                Summary::Convert(ConvertSummary {
                    total_files: 1,
                    candidates: 1,
                    converted: 1,
                    ..ConvertSummary::default()
                }),
            ),
            (
                "verify",
                Summary::Verify(VerifySummary {
                    total_files: 1,
                    candidates: 1,
                    passed: 1,
                    failed: 0,
                }),
            ),
            (
                // The superset shape: must NOT deserialize as the verify
                // variant when the mode says fix.
                "fix",
                Summary::Fix(FixSummary {
                    total_files: 1,
                    candidates: 1,
                    already_utf8: 0,
                    converted: 1,
                    passed: 0,
                    failed: 1,
                    ..FixSummary::default()
                }),
            ),
        ];
        for (mode, summary) in cases {
            let report = Report {
                tool: "utf8-forge".into(),
                version: "0.2.5".into(),
                mode: mode.into(),
                root: Some("tree".into()),
                url: None,
                started_at: "2026-01-01T00:00:00Z".into(),
                duration_ms: 5,
                options: options.clone(),
                summary,
                files: files.clone(),
                unknown_files: Vec::new(),
                error_files: Vec::new(),
                verify_failed_files: Vec::new(),
                warnings: vec!["walk warning".into()],
            };
            let json = serde_json::to_string(&report).unwrap();
            let back: Report = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2, "round trip must be stable for {mode}");
        }
    }

    #[test]
    fn summary_refuses_standalone_deserialize() {
        let json = serde_json::to_string(&Summary::Verify(VerifySummary {
            total_files: 1,
            candidates: 1,
            passed: 1,
            failed: 0,
        }))
        .unwrap();
        assert!(serde_json::from_str::<Summary>(&json).is_err());
    }

    #[test]
    fn file_actions_vocabulary_is_complete() {
        for action in FILE_ACTIONS {
            let record = FileRecord {
                path: "p".into(),
                detected: None,
                action: (*action).to_string(),
                bytes_before: None,
                bytes_after: None,
                duration_us: 0,
                error: None,
            };
            // Each vocabulary entry must serialize to itself.
            let json = serde_json::to_value(&record).unwrap();
            assert_eq!(json["action"].as_str().unwrap(), *action);
        }
    }

    #[test]
    fn rfc3339_now_is_recent() {
        // Sanity bound instead of a fixed prefix: the current date must sit
        // inside this tool's era, whenever the test runs.
        let s = rfc3339_now();
        let year: u32 = s[..4].parse().expect("rfc3339 year prefix");
        assert!((2026..=2999).contains(&year), "unexpected timestamp {s}");
    }
}
