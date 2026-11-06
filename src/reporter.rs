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
//! Report guarantees:
//! - `files[]` is sorted by `path`, so audits are reproducible run to run.
//! - Each mode serializes exactly its own summary fields via an untagged enum
//!   (`scan` distribution, `convert`/`fetch` counters, `verify` gate).

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;

/// A single processed file in the report.
#[derive(Debug, Clone, Serialize)]
pub struct FileRecord {
    pub path: String,
    /// Detection tag (`utf-8`, `gb18030`, ...); `None` when unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected: Option<&'static str>,
    /// `would_convert` | `converted` | `unchanged` | `skipped_unknown` |
    /// `skipped_too_large` | `error` | `detected` | `passed` | `failed`.
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_after: Option<u64>,
    pub duration_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Effective CLI options snapshot, embedded in every JSON report.
#[derive(Debug, Serialize)]
pub struct OptionsBlock {
    pub target: &'static str,
    pub dry_run: bool,
    pub extensions: Vec<String>,
    pub exclude_dirs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_ext: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_globs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<String>,
    /// Config file in effect (`--config` value verbatim); omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_file: Option<String>,
    /// Ignore rules in effect (explicit path or auto-discovered
    /// `<root>/.utf8forgeignore`); omitted when none are active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_file: Option<String>,
}

/// `scan` summary: per-encoding distribution counts.
#[derive(Debug, Clone, Serialize, Default)]
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
#[derive(Debug, Clone, Serialize, Default)]
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
#[derive(Debug, Clone, Serialize, Default)]
pub struct VerifySummary {
    pub total_files: u64,
    pub candidates: u64,
    pub passed: u64,
    pub failed: u64,
}

/// `fix` summary: convert counters plus the verify gate result on the same
/// tree. `fetch_error` is impossible in fix mode and is omitted.
#[derive(Debug, Clone, Serialize, Default)]
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
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Summary {
    Scan(ScanSummary),
    Convert(ConvertSummary),
    Verify(VerifySummary),
    Fix(FixSummary),
}

/// Full JSON report document.
#[derive(Debug, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub version: &'static str,
    pub mode: &'static str,
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
    fn rfc3339_now_is_recent() {
        // Sanity: current date must be after 2026-01-01 (this tool's era).
        let s = rfc3339_now();
        assert!(s.starts_with("202"), "unexpected timestamp {s}");
    }
}
