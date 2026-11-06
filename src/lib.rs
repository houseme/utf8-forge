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

//! utf8-forge orchestration: per-file pipelines, report assembly, and the
//! `run()` entry point with exit-code mapping.
//!
//! Exit codes:
//! - `0` success
//! - `1` non-fatal per-file issues (`unknown`, `decode_error`, `encode_error`,
//!   `io_error`, `fetch_error`)
//! - `2` fatal (bad args, missing root, I/O setup failure) — also emitted by
//!   clap on parse errors
//! - `3` `verify`/`fix` gate failure (non-UTF-8 whitelisted files; fix's
//!   verify gate wins over convert-side issues)

// Public modules (lib target so integration tests can exercise the
// pipeline directly; the binary in `main.rs` stays a thin wrapper).
pub mod cli;
pub mod config;
pub mod detector;
pub mod differ;
pub mod error;
#[cfg(feature = "remote")]
pub mod fetcher;
pub mod filter;
pub mod ignorefile;
pub mod reporter;
pub mod transcoder;
pub mod walker;
pub mod writer;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, bail};
use rayon::prelude::*;

use crate::cli::{Cli, Commands, ConvertArgs, DiffArgs, FixArgs, TreeArgs};
use crate::detector::{DetectedEncoding, detect};
use crate::differ::DiffInput;
use crate::error::ForgeError;
use crate::filter::{FilterConfig, FilterOptions};
use crate::reporter::{
    ConsoleCfg, ConvertSummary, FileRecord, FixSummary, OptionsBlock, Report, ScanSummary, Summary,
    VerifySummary, print_header, print_problem, print_verbose, write_report,
};
use crate::transcoder::{TargetEncoding, TranscodeOutcome, transcode_detected};
use crate::walker::collect_files;

/// Dispatch a parsed CLI and map the outcome to a process exit code.
/// Fatal setup errors print to stderr and yield exit code 2.
pub fn run(cli: Cli) -> i32 {
    let outcome = match cli.command {
        Commands::Scan(args) => cmd_scan(&args),
        Commands::Convert(args) => cmd_convert(&args),
        Commands::Verify(args) => cmd_verify(&args),
        Commands::Fix(args) => cmd_fix(&args),
        Commands::Diff(args) => cmd_diff(&args),
    };
    outcome.unwrap_or_else(|err| {
        eprintln!("[utf8-forge] fatal: {err:#}");
        2
    })
}

// ---------------------------------------------------------------------------
// Per-file processors
// ---------------------------------------------------------------------------

/// Read at most one byte beyond the cap so callers can distinguish an exact
/// fit from oversized content even when the file grows after the metadata
/// check. The extra byte is only used for the size verdict, never transcoded.
fn read_file_limited(path: &Path, max_file_size: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(max_file_size.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Deferred `FileRecord` builder that stamps per-file duration at completion.
struct RecBuilder<'a> {
    path: &'a Path,
    started: Instant,
}

impl<'a> RecBuilder<'a> {
    fn new(path: &'a Path) -> RecBuilder<'a> {
        RecBuilder {
            path,
            started: Instant::now(),
        }
    }

    fn done(
        self,
        detected: Option<&'static str>,
        action: &'static str,
        bytes_before: Option<u64>,
        bytes_after: Option<u64>,
        error: Option<String>,
    ) -> FileRecord {
        FileRecord {
            path: self.path.to_string_lossy().into_owned(),
            detected,
            action,
            bytes_before,
            bytes_after,
            duration_us: self.started.elapsed().as_micros() as u64,
            error,
        }
    }
}

/// Detect only. Oversized files are never read.
pub fn scan_file(path: &Path, max_file_size: u64) -> FileRecord {
    let b = RecBuilder::new(path);
    let size = fs::metadata(path).map(|m| m.len()).ok();
    if let Some(sz) = size
        && sz > max_file_size
    {
        return b.done(None, "skipped_too_large", Some(sz), None, None);
    }
    let bytes = match read_file_limited(path, max_file_size) {
        Ok(bytes) => bytes,
        Err(e) => return b.done(None, "error", size, None, Some(format!("io_error: {e}"))),
    };
    let sz = bytes.len() as u64;
    if sz > max_file_size {
        return b.done(None, "skipped_too_large", Some(sz), None, None);
    }
    let det = detect(&bytes);
    b.done(Some(det.tag()), "detected", Some(sz), None, None)
}

/// Shared context for tree-mode conversion.
#[derive(Debug, Clone)]
pub struct ConvertCtx {
    pub root: std::path::PathBuf,
    pub target: TargetEncoding,
    pub dry_run: bool,
    pub backup_dir: Option<std::path::PathBuf>,
    pub max_file_size: u64,
}

/// Detect + transcode + (unless dry-run) backup + atomic write.
///
/// Byte-identical outcomes are recorded as `unchanged` and never touch the
/// filesystem, so unchanged files keep their mtime. Every failure is
/// contained in this one record (per-file error isolation).
pub fn convert_file(path: &Path, ctx: &ConvertCtx) -> FileRecord {
    let b = RecBuilder::new(path);
    let size = fs::metadata(path).map(|m| m.len()).ok();
    if let Some(sz) = size
        && sz > ctx.max_file_size
    {
        return b.done(None, "skipped_too_large", Some(sz), None, None);
    }
    let bytes = match read_file_limited(path, ctx.max_file_size) {
        Ok(bytes) => bytes,
        Err(e) => return b.done(None, "error", size, None, Some(format!("io_error: {e}"))),
    };
    let sz = bytes.len() as u64;
    if sz > ctx.max_file_size {
        return b.done(None, "skipped_too_large", Some(sz), None, None);
    }
    let det = detect(&bytes);
    let tag = det.tag();

    match transcode_detected(&bytes, det, ctx.target) {
        Err(ForgeError::UnknownEncoding) => {
            b.done(Some(tag), "skipped_unknown", Some(sz), None, None)
        }
        Err(e) => b.done(
            Some(tag),
            "error",
            Some(sz),
            None,
            Some(format!("{}: {e}", e.category())),
        ),
        Ok(TranscodeOutcome::Unchanged) => b.done(Some(tag), "unchanged", Some(sz), Some(sz), None),
        Ok(TranscodeOutcome::Changed(new_bytes)) => {
            if ctx.dry_run {
                return b.done(
                    Some(tag),
                    "would_convert",
                    Some(sz),
                    Some(new_bytes.len() as u64),
                    None,
                );
            }
            // Refuse to clobber write-protected files: POSIX rename would
            // silently bypass the permission bits, but the operator locked
            // that file on purpose.
            let write_protected = fs::metadata(path)
                .map(|m| m.permissions().readonly())
                .unwrap_or(false);
            if write_protected {
                return b.done(
                    Some(tag),
                    "error",
                    Some(sz),
                    None,
                    Some(
                        "io_error: file is write-protected (read-only); chmod +w to convert".into(),
                    ),
                );
            }
            // Backup the original bytes before the rename, so the backup
            // always holds the pre-conversion content.
            if let Some(backup_dir) = &ctx.backup_dir
                && let Err(e) = writer::backup_original(&ctx.root, path, backup_dir, &bytes)
            {
                return b.done(
                    Some(tag),
                    "error",
                    Some(sz),
                    None,
                    Some(format!("{}: backup failed: {e}", e.category())),
                );
            }
            match writer::atomic_write(path, &new_bytes) {
                Ok(()) => b.done(
                    Some(tag),
                    "converted",
                    Some(sz),
                    Some(new_bytes.len() as u64),
                    None,
                ),
                Err(e) => b.done(
                    Some(tag),
                    "error",
                    Some(sz),
                    None,
                    Some(format!("{}: write failed: {e}", e.category())),
                ),
            }
        }
    }
}

/// Verify a file is UTF-8 without BOM. Unreadable / oversized files fail
/// closed (a gate must not wave through what it cannot inspect). The size is
/// re-checked after the read, mirroring the other processors, so a file that
/// grows between stat and read cannot slip past the cap.
pub fn verify_file(path: &Path, max_file_size: u64) -> FileRecord {
    let b = RecBuilder::new(path);
    let size = fs::metadata(path).map(|m| m.len()).ok();
    if let Some(sz) = size
        && sz > max_file_size
    {
        return b.done(
            None,
            "failed",
            Some(sz),
            None,
            Some("too_large: exceeds --max-file-size".to_string()),
        );
    }
    let bytes = match read_file_limited(path, max_file_size) {
        Ok(bytes) => bytes,
        Err(e) => return b.done(None, "failed", size, None, Some(format!("io_error: {e}"))),
    };
    let sz = bytes.len() as u64;
    if sz > max_file_size {
        return b.done(
            None,
            "failed",
            Some(sz),
            None,
            Some("too_large: exceeds --max-file-size".to_string()),
        );
    }
    let det = detect(&bytes);
    let tag = det.tag();
    if det == DetectedEncoding::Utf8 {
        b.done(Some(tag), "passed", Some(sz), None, None)
    } else {
        b.done(Some(tag), "failed", Some(sz), None, None)
    }
}

// ---------------------------------------------------------------------------
// Report assembly
// ---------------------------------------------------------------------------

/// Sort records by path (reports stay reproducible run to run), compute the
/// mode-specific summary, and assemble the final report document.
/// Run-level facts that flow into the report document unchanged.
struct RunFacts {
    root: Option<std::path::PathBuf>,
    url: Option<String>,
    options: OptionsBlock,
    started_at: String,
    verify_failed: Option<Vec<String>>,
}

fn finalize_report(
    mode: &'static str,
    facts: RunFacts,
    mut records: Vec<FileRecord>,
    elapsed: std::time::Duration,
) -> Report {
    records.sort_by(|a, b| a.path.cmp(&b.path));
    let total = records.len() as u64;

    let summary = match mode {
        "scan" => {
            let mut s = ScanSummary {
                total_files: total,
                ..ScanSummary::default()
            };
            for r in &records {
                if r.action == "skipped_too_large" {
                    s.too_large += 1;
                }
                if r.action == "error" {
                    s.errors += 1;
                }
                match r.detected {
                    Some("utf-8") => s.utf8 += 1,
                    Some("utf-8-bom") => s.utf8_bom += 1,
                    Some("utf-16le") => s.utf16le += 1,
                    Some("utf-16be") => s.utf16be += 1,
                    Some("utf-32le") => s.utf32le += 1,
                    Some("utf-32be") => s.utf32be += 1,
                    Some("gb18030") => s.gb18030 += 1,
                    Some("unknown") => s.unknown += 1,
                    _ => {}
                }
            }
            s.candidates = total - s.too_large;
            Summary::Scan(s)
        }
        "verify" => {
            let passed = records.iter().filter(|r| r.action == "passed").count() as u64;
            let failed = records.iter().filter(|r| r.action == "failed").count() as u64;
            Summary::Verify(VerifySummary {
                total_files: total,
                candidates: total,
                passed,
                failed,
            })
        }
        "fix" => {
            let mut s = FixSummary {
                total_files: total,
                ..FixSummary::default()
            };
            for r in &records {
                if r.action == "skipped_too_large" {
                    s.too_large += 1;
                }
                match r.action {
                    "unchanged" => s.already_utf8 += 1,
                    "converted" | "would_convert" => s.converted += 1,
                    "skipped_unknown" => s.unknown += 1,
                    "error" => {
                        let msg = r.error.as_deref().unwrap_or("");
                        if msg.starts_with("decode_error") {
                            s.decode_errors += 1;
                        } else if msg.starts_with("encode_error") {
                            s.encode_errors += 1;
                        } else {
                            s.io_errors += 1;
                        }
                    }
                    _ => {}
                }
            }
            s.candidates = total - s.too_large;
            Summary::Fix(s)
        }
        _ => {
            let mut s = ConvertSummary {
                total_files: total,
                ..ConvertSummary::default()
            };
            for r in &records {
                match r.action {
                    "unchanged" => s.already_utf8 += 1,
                    "converted" | "would_convert" => s.converted += 1,
                    "skipped_unknown" => s.unknown += 1,
                    "skipped_too_large" => s.too_large += 1,
                    "error" => {
                        let msg = r.error.as_deref().unwrap_or("");
                        if msg.starts_with("decode_error") {
                            s.decode_errors += 1;
                        } else if msg.starts_with("encode_error") {
                            s.encode_errors += 1;
                        } else if msg.starts_with("fetch_error") {
                            s.fetch_error += 1;
                        } else {
                            s.io_errors += 1;
                        }
                    }
                    _ => {}
                }
            }
            s.candidates = total - s.too_large;
            Summary::Convert(s)
        }
    };

    let unknown_files: Vec<String> = records
        .iter()
        .filter(|r| r.action == "skipped_unknown" || r.detected == Some("unknown"))
        .map(|r| r.path.clone())
        .collect();
    let error_files: Vec<String> = records
        .iter()
        .filter(|r| r.action == "error" || r.action == "failed")
        .map(|r| r.path.clone())
        .collect();
    // Fix mode's verify gate; empty (and omitted) in every other mode.

    Report {
        tool: "utf8-forge",
        version: env!("CARGO_PKG_VERSION"),
        mode,
        root: facts.root.map(|p| p.to_string_lossy().into_owned()),
        url: facts.url,
        started_at: facts.started_at,
        duration_ms: elapsed.as_millis() as u64,
        options: facts.options,
        summary,
        files: records,
        unknown_files,
        error_files,
        verify_failed_files: facts.verify_failed.unwrap_or_default(),
    }
}

/// Snapshot of effective options for the report's `options` block.
/// Conversion-only fields are taken from `conv` (absent in scan/verify).
fn options_block(
    args: &TreeArgs,
    conv: Option<&dyn ConvertOptions>,
    filter: &FilterConfig,
    config_file: Option<&str>,
    ignore_file: Option<&str>,
) -> OptionsBlock {
    let mut extensions: Vec<String> = filter.extensions.iter().cloned().collect();
    extensions.sort();
    let mut exclude_dirs: Vec<String> = filter.exclude_dirs.iter().cloned().collect();
    exclude_dirs.sort();
    let mut exclude_ext: Vec<String> = args.exclude_ext.clone();
    exclude_ext.sort();
    OptionsBlock {
        target: conv.map(|c| c.effective_target()).unwrap_or("utf-8"),
        dry_run: conv.is_some_and(|c| c.is_dry_run()),
        extensions,
        exclude_dirs,
        exclude_ext,
        exclude_globs: args.exclude_glob.clone(),
        backup_dir: conv.and_then(|c| c.backup_dir().map(|p| p.to_string_lossy().into_owned())),
        config_file: config_file.map(str::to_string),
        ignore_file: ignore_file.map(str::to_string),
    }
}

// ---------------------------------------------------------------------------
// Console output
// ---------------------------------------------------------------------------

/// Print problem lines (stderr, always) and verbose detail lines (stdout, -v).
///
/// `target_tag` is the destination encoding shown in converted /
/// would_convert lines (convert/fetch modes); `None` for scan/verify whose
/// verbose lines carry no target.
fn print_records(records: &[FileRecord], console: ConsoleCfg, target_tag: Option<&'static str>) {
    for r in records {
        match r.action {
            "error" => print_problem(format!(
                "error {} ({})",
                r.path,
                r.error.as_deref().unwrap_or("unspecified")
            )),
            "skipped_unknown" => print_problem(format!("unknown {}", r.path)),
            // Scan records unknown files as `detected` with the `unknown` tag;
            // they are still per-file issues (scan exits 1) and get the same
            // stderr line as `skipped_unknown` in the other modes.
            "detected" if r.detected == Some("unknown") => {
                print_problem(format!("unknown {}", r.path))
            }
            "failed" => {
                let reason = r
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("detected={}", r.detected.unwrap_or("?")));
                print_problem(format!("failed {} ({reason})", r.path));
            }
            _ => {}
        }
        match r.action {
            // Unknown files already got their stderr problem line; the
            // verbose classification line would repeat it verbatim under -v.
            "detected" if r.detected != Some("unknown") => {
                print_verbose(console, format!("{} {}", r.detected.unwrap_or("?"), r.path))
            }
            "unchanged" => print_verbose(
                console,
                format!("unchanged {} ({})", r.path, r.detected.unwrap_or("?")),
            ),
            "converted" | "would_convert" => print_verbose(
                console,
                format!(
                    "{} {} ({} → {}, {} → {} bytes)",
                    r.action,
                    r.path,
                    r.detected.unwrap_or("?"),
                    target_tag.unwrap_or("?"),
                    r.bytes_before.unwrap_or(0),
                    r.bytes_after.unwrap_or(0)
                ),
            ),
            "skipped_too_large" => print_verbose(
                console,
                format!(
                    "skipped_too_large {} ({} bytes)",
                    r.path,
                    r.bytes_before.unwrap_or(0)
                ),
            ),
            "passed" => print_verbose(console, format!("passed {}", r.path)),
            _ => {}
        }
    }
}

/// Final duration (+ report path) line.
fn print_duration(report: Option<&Path>, elapsed: std::time::Duration, console: ConsoleCfg) {
    if console.quiet {
        return;
    }
    match report {
        Some(p) => println!(
            "[utf8-forge] duration={:.1}s  report={}",
            elapsed.as_secs_f64(),
            p.display()
        ),
        None => println!("[utf8-forge] duration={:.1}s", elapsed.as_secs_f64()),
    }
}

/// `convert` / `fetch` console summary.
fn print_convert_summary(
    s: &ConvertSummary,
    report: Option<&Path>,
    elapsed: std::time::Duration,
    console: ConsoleCfg,
) {
    if !console.quiet {
        println!(
            "[utf8-forge] scanned={}  candidates={}",
            s.total_files, s.candidates
        );
        let errors = s.decode_errors + s.encode_errors + s.io_errors;
        println!(
            "[utf8-forge] already_utf8={}  converted={}  unknown={}  errors={}",
            s.already_utf8, s.converted, s.unknown, errors
        );
        if s.too_large > 0 {
            println!("[utf8-forge] too_large={}", s.too_large);
        }
        if s.fetch_error > 0 {
            println!("[utf8-forge] fetch_error={}", s.fetch_error);
        }
    }
    print_duration(report, elapsed, console);
}

/// `scan` console summary (encoding distribution).
fn print_scan_summary(
    s: &ScanSummary,
    report: Option<&Path>,
    elapsed: std::time::Duration,
    console: ConsoleCfg,
) {
    if !console.quiet {
        println!(
            "[utf8-forge] scanned={}  candidates={}",
            s.total_files, s.candidates
        );
        println!(
            "[utf8-forge] utf8={}  utf8_bom={}  utf16le={}  utf16be={}  utf32le={}  utf32be={}  gb18030={}  unknown={}",
            s.utf8, s.utf8_bom, s.utf16le, s.utf16be, s.utf32le, s.utf32be, s.gb18030, s.unknown
        );
        if s.errors > 0 {
            println!("[utf8-forge] errors={}", s.errors);
        }
        if s.too_large > 0 {
            println!("[utf8-forge] too_large={}", s.too_large);
        }
    }
    print_duration(report, elapsed, console);
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build the effective filter; conflicting flags / bad globs are fatal.
fn build_filter(args: &TreeArgs) -> anyhow::Result<FilterConfig> {
    let opts = FilterOptions {
        ext_override: args.ext.clone(),
        exclude_ext: args.exclude_ext.clone(),
        exclude_dirs: args.exclude.clone(),
        exclude_globs: args.exclude_glob.clone(),
        include_rc: args.include_rc,
        include_msbuild: args.include_msbuild,
        include_hidden: args.include_hidden,
    };
    FilterConfig::build(&opts).map_err(anyhow::Error::msg)
}

/// Load the TOML config (no-op default when `--config` absent) and merge it
/// into the shared tree args. CLI flags win over config values; lists replace
/// across layers (never concatenate) so the effective whitelist stays
/// auditable. Runs before any pre-flight so downstream `ensure_*` checks see
/// resolved values. Returns the parsed config for conversion-option merging.
fn merge_tree_config(
    common: &mut TreeArgs,
) -> anyhow::Result<(Option<String>, crate::config::ForgeConfig)> {
    let Some(path) = common.config.clone() else {
        return Ok((None, crate::config::ForgeConfig::default()));
    };
    let cfg = crate::config::load(&path)?;
    if common.ext.is_empty() {
        common.ext = cfg.filter.extensions.clone();
    }
    if common.exclude.is_empty() {
        common.exclude = cfg.filter.exclude_dirs.clone();
    }
    if common.exclude_ext.is_empty() {
        common.exclude_ext = cfg.filter.exclude_ext.clone();
    }
    if common.exclude_glob.is_empty() {
        common.exclude_glob = cfg.filter.exclude_globs.clone();
    }
    common.include_rc |= cfg.filter.include_rc;
    common.include_msbuild |= cfg.filter.include_msbuild;
    common.include_hidden |= cfg.filter.include_hidden;
    if common.max_file_size.is_none() {
        common.max_file_size = cfg.options.max_file_size;
    }
    if common.threads.is_none() {
        common.threads = cfg.options.threads;
    }
    // Validate `[convert].target` even in modes that ignore the section, so
    // an invalid value is always a loud, key-naming error instead of being
    // silently dropped by scan/verify.
    cfg.target()?;
    Ok((Some(path.to_string_lossy().into_owned()), cfg))
}

/// Conversion-option surface shared by `convert` and `fix`, so the
/// `[convert]` config section merges identically into both.
trait ConvertOptions {
    fn common(&self) -> &TreeArgs;
    fn common_mut(&mut self) -> &mut TreeArgs;
    fn set_target(&mut self, t: Option<crate::transcoder::TargetEncoding>);
    fn set_backup_dir(&mut self, p: Option<std::path::PathBuf>);
    fn target(&self) -> Option<crate::transcoder::TargetEncoding>;
    fn backup_dir(&self) -> Option<&std::path::PathBuf>;
    fn is_dry_run(&self) -> bool;

    /// The target tag after CLI/config merge (built-in default: utf-8).
    fn effective_target(&self) -> &'static str {
        self.target()
            .unwrap_or(crate::transcoder::TargetEncoding::Utf8)
            .tag()
    }
}

impl ConvertOptions for ConvertArgs {
    fn common(&self) -> &TreeArgs {
        &self.common
    }
    fn target(&self) -> Option<crate::transcoder::TargetEncoding> {
        self.target
    }
    fn common_mut(&mut self) -> &mut TreeArgs {
        &mut self.common
    }
    fn set_target(&mut self, t: Option<crate::transcoder::TargetEncoding>) {
        self.target = t;
    }
    fn set_backup_dir(&mut self, p: Option<std::path::PathBuf>) {
        self.backup_dir = p;
    }
    fn backup_dir(&self) -> Option<&std::path::PathBuf> {
        self.backup_dir.as_ref()
    }
    fn is_dry_run(&self) -> bool {
        self.dry_run
    }
}

impl ConvertOptions for FixArgs {
    fn common(&self) -> &TreeArgs {
        &self.common
    }
    fn target(&self) -> Option<crate::transcoder::TargetEncoding> {
        self.target
    }
    fn common_mut(&mut self) -> &mut TreeArgs {
        &mut self.common
    }
    fn set_target(&mut self, t: Option<crate::transcoder::TargetEncoding>) {
        self.target = t;
    }
    fn set_backup_dir(&mut self, p: Option<std::path::PathBuf>) {
        self.backup_dir = p;
    }
    fn backup_dir(&self) -> Option<&std::path::PathBuf> {
        self.backup_dir.as_ref()
    }
    fn is_dry_run(&self) -> bool {
        self.dry_run
    }
}

/// Apply the tree merge plus the `[convert]` section (convert / fix).
/// CLI flags win over config values; runs before any pre-flight. Both
/// sections use the same parsed snapshot of the file.
fn merge_convert_config(conv: &mut impl ConvertOptions) -> anyhow::Result<Option<String>> {
    let (recorded, cfg) = merge_tree_config(conv.common_mut())?;
    if conv.target().is_none()
        && let Some(t) = cfg.target()?
    {
        conv.set_target(Some(t));
    }
    if conv.backup_dir().is_none()
        && let Some(dir) = &cfg.convert.backup_dir
    {
        conv.set_backup_dir(Some(std::path::PathBuf::from(dir)));
    }
    Ok(recorded)
}

/// Root must be an existing directory, else exit code 2.
fn ensure_root(root: &Path) -> anyhow::Result<()> {
    if !root.is_dir() {
        bail!("root `{}` is not an existing directory", root.display());
    }
    Ok(())
}

/// Prepare the backup directory up front so a broken backup path aborts
/// before any source file is touched. A backup dir **inside** `--root` is
/// fatal: the next run would walk the backups and re-convert them, breaking
/// idempotence (a second run would no longer be a no-op) and silently
/// mutating the safety net. Placement is checked existence-independently
/// (from the deepest existing ancestor) *before* anything is created, so a
/// rejected configuration never leaves directories behind and a dry-run
/// never blesses what the real run would reject.
fn ensure_backup_dir(args: &impl ConvertOptions) -> anyhow::Result<()> {
    let Some(dir) = args.backup_dir() else {
        return Ok(());
    };
    let root_canonical = fs::canonicalize(&args.common().root)?;
    check_backup_placement(dir, &root_canonical)?;
    if args.is_dry_run() {
        return Ok(());
    }
    fs::create_dir_all(dir)
        .with_context(|| format!("cannot create backup dir {}", dir.display()))?;
    ensure_writable_dir(dir)?;
    Ok(())
}

/// Resolve `dir` to its would-be canonical path without requiring it to
/// exist: canonicalize the deepest existing ancestor, then re-attach the
/// non-existing tail. Fatal when the projected path lands inside `root`.
fn check_backup_placement(dir: &Path, root_canonical: &Path) -> anyhow::Result<()> {
    // A relative dir such as `backup` targets a location under the process
    // cwd; resolve it first so the ancestor walk starts from an existing
    // base instead of failing on a not-yet-created relative tail.
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .with_context(|| format!("cannot resolve relative --backup-dir `{}`", dir.display()))?
            .join(dir)
    };
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = dir.to_path_buf();
    let base = loop {
        match fs::canonicalize(&ancestor) {
            Ok(c) => break c,
            Err(_) => {
                let Some(name) = ancestor.file_name() else {
                    bail!("--backup-dir `{}` has no existing ancestor", dir.display());
                };
                let Some(parent) = ancestor.parent() else {
                    bail!("--backup-dir `{}` has no existing ancestor", dir.display());
                };
                tail.push(name.to_os_string());
                ancestor = parent.to_path_buf();
            }
        }
    };
    let projected = tail.iter().rev().fold(base, |p, c| p.join(c));
    if projected.starts_with(root_canonical) {
        bail!(
            "--backup-dir `{}` is inside `--root` `{}`; backups would be re-processed on the next run. Keep the backup directory outside the scanned tree.",
            dir.display(),
            root_canonical.display()
        );
    }
    Ok(())
}

/// Pre-flight the report path so an unwritable report aborts (exit 2) before
/// any file is converted, and collect the canonical report path for
/// traversal exclusion (`--report` inside the tree must not pollute later
/// runs). A bare filename (no directory component) is probed against the
/// current directory.
fn ensure_report_dir(report: &Path) -> anyhow::Result<()> {
    if report.is_dir() {
        bail!("--report `{}` is an existing directory", report.display());
    }
    let parent = match report.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    fs::create_dir_all(&parent)
        .with_context(|| format!("cannot create report directory {}", parent.display()))?;
    // create_dir_all is a no-op on an existing directory regardless of
    // permissions — probe writability so an existing read-only report
    // directory also aborts before the tree is touched.
    ensure_writable_dir(&parent)?;
    Ok(())
}

/// Verify `dir` accepts new files by creating and immediately removing a
/// unique probe file. Catches read-only directories that `create_dir_all`
/// silently accepts because they already exist.
fn ensure_writable_dir(dir: &Path) -> anyhow::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(
        ".utf8-forge-probe-{}-{}",
        std::process::id(),
        nanos
    ));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => bail!("directory `{}` is not writable: {e}", dir.display()),
    }
}

/// Traversal skip list: the report path and the ignore file (so neither is
/// ever scanned as content) — canonicalized downstream by `collect_files`.
fn skip_paths(report: Option<&Path>, ignore: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = report {
        v.push(p.to_path_buf());
    }
    if let Some(p) = ignore {
        v.push(p.to_path_buf());
    }
    v
}

/// Resolve + load ignore rules per the flag precedence: `--no-ignore-file`
/// disables everything; an explicit `--ignore-file` is fatal when missing;
/// auto-discovery of `<root>/.utf8forgeignore` is silent when absent and a
/// warning when unreadable. Returns the rules plus the path to record in the
/// report (None = no ignore layer active).
fn load_ignore(
    args: &TreeArgs,
) -> anyhow::Result<(Option<ignorefile::IgnoreFile>, Option<String>, Vec<String>)> {
    if args.no_ignore_file {
        return Ok((None, None, Vec::new()));
    }
    match &args.ignore_file {
        Some(path) => {
            if !path.is_file() {
                bail!("ignore file `{}` not found", path.display());
            }
            let rules = ignorefile::IgnoreFile::load(path).map_err(anyhow::Error::msg)?;
            let recorded = path.to_string_lossy().into_owned();
            Ok((Some(rules), Some(recorded), Vec::new()))
        }
        None => {
            let path = args.root.join(".utf8forgeignore");
            if !path.is_file() {
                return Ok((None, None, Vec::new()));
            }
            match ignorefile::IgnoreFile::load(&path) {
                Ok(rules) => {
                    let recorded = path.to_string_lossy().into_owned();
                    Ok((Some(rules), Some(recorded), Vec::new()))
                }
                Err(e) => Ok((
                    None,
                    None,
                    vec![format!(
                        "warning: ignoring unreadable .utf8forgeignore: {e}"
                    )],
                )),
            }
        }
    }
}

/// RFC 3339 timestamp captured when the run starts, so `started_at` +
/// `duration_ms` line up even for long runs.
fn wall_now() -> String {
    crate::reporter::rfc3339_now()
}

/// Run `f` on the configured rayon pool (default pool when `--threads` unset).
fn with_pool<R: Send>(threads: Option<usize>, f: impl FnOnce() -> R + Send) -> anyhow::Result<R> {
    match threads {
        Some(n) => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(n.max(1))
                .build()
                .context("failed to build rayon thread pool")?;
            Ok(pool.install(f))
        }
        None => Ok(f()),
    }
}

fn console_cfg(args: &TreeArgs) -> ConsoleCfg {
    ConsoleCfg {
        quiet: args.quiet,
        verbose: args.verbose,
    }
}

/// Print non-fatal traversal warnings (suppressed by `--quiet`).
fn print_warnings(warnings: &[String], console: ConsoleCfg) {
    if !console.quiet {
        for w in warnings {
            print_problem(w.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

/// `scan`: detect only, no writes. Exit 1 when any file is `unknown` or
/// errored (I/O), matching the documented per-file issue contract.
fn cmd_scan(args: &TreeArgs) -> anyhow::Result<i32> {
    let mut args = args.clone();
    let (config_file, _) = merge_tree_config(&mut args)?;
    ensure_root(&args.root)?;
    let filter = build_filter(&args)?;
    if let Some(path) = &args.report {
        ensure_report_dir(path)?;
    }
    let console = console_cfg(&args);
    print_header(console, format!("mode=scan root={}", args.root.display()));

    let (ignore, ignore_recorded, ignore_warnings) = load_ignore(&args)?;
    let mut warnings = Vec::new();
    let walk = {
        let skip = skip_paths(
            args.report.as_deref(),
            ignore_recorded
                .as_deref()
                .map(std::path::PathBuf::from)
                .as_deref(),
        );
        let walk = collect_files(&args.root, &filter, &skip, ignore.as_ref());
        warnings.extend(ignore_warnings);
        walk
    };
    warnings.extend(walk.warnings);
    print_warnings(&warnings, console);
    let started_at = wall_now();
    let started = Instant::now();
    let records = with_pool(args.threads, || {
        walk.files
            .par_iter()
            .map(|p| scan_file(p, args.effective_max_file_size()))
            .collect::<Vec<_>>()
    })?;
    print_records(&records, console, None);
    let elapsed = started.elapsed();

    let report = finalize_report(
        "scan",
        RunFacts {
            root: Some(args.root.clone()),
            url: None,
            options: options_block(
                &args,
                None,
                &filter,
                config_file.as_deref(),
                ignore_recorded.as_deref(),
            ),
            started_at,
            verify_failed: None,
        },
        records,
        elapsed,
    );
    let unknown = match &report.summary {
        Summary::Scan(s) => s.unknown,
        _ => 0,
    };
    // Per-file I/O failures also count as issues: the tree was not fully
    // inventoried, so "all classified" would be a lie.
    let file_errors = report.files.iter().filter(|r| r.action == "error").count() as u64;
    if let Some(path) = &args.report {
        write_report(&report, path)?;
    }
    if let Summary::Scan(s) = &report.summary {
        print_scan_summary(s, args.report.as_deref(), elapsed, console);
    }
    Ok(if unknown > 0 || file_errors > 0 { 1 } else { 0 })
}

/// `convert`: tree mode or remote fetch.
fn cmd_convert(args: &ConvertArgs) -> anyhow::Result<i32> {
    #[cfg(feature = "remote")]
    {
        if let Some(url) = &args.url {
            return cmd_convert_url(args, url);
        }
    }
    #[cfg(not(feature = "remote"))]
    {
        if args.url.is_some() {
            bail!("--url requires the `remote` feature; rebuild with default features");
        }
    }
    cmd_convert_tree(args)
}

/// `convert` tree mode: detect + transcode + atomic write. Exit 1 when any
/// per-file issue (`unknown` / decode / io) remains.
fn cmd_convert_tree(args: &ConvertArgs) -> anyhow::Result<i32> {
    let mut args = args.clone();
    let config_file = merge_convert_config(&mut args)?;
    let common = &args.common;
    let target = args.target_or_default();
    ensure_root(&common.root)?;
    let filter = build_filter(common)?;
    // The report pre-flight is side-effect-free, so it runs before the backup
    // directory is created — a rejected report path must not leave a fresh
    // backup dir behind.
    if let Some(path) = &common.report {
        ensure_report_dir(path)?;
    }
    ensure_backup_dir(&args)?;
    let console = console_cfg(common);
    let dry_tag = if args.dry_run { " dry_run=true" } else { "" };
    print_header(
        console,
        format!(
            "mode=convert target={} root={}{}",
            target,
            common.root.display(),
            dry_tag
        ),
    );

    let (ignore, ignore_recorded, ignore_warnings) = load_ignore(common)?;
    let mut warnings = ignore_warnings;
    let walk = {
        let skip = skip_paths(
            common.report.as_deref(),
            ignore_recorded
                .as_deref()
                .map(std::path::PathBuf::from)
                .as_deref(),
        );
        collect_files(&common.root, &filter, &skip, ignore.as_ref())
    };
    warnings.extend(walk.warnings);
    print_warnings(&warnings, console);
    let ctx = ConvertCtx {
        root: common.root.clone(),
        target,
        dry_run: args.dry_run,
        backup_dir: args.backup_dir.clone(),
        max_file_size: common.effective_max_file_size(),
    };
    let started_at = wall_now();
    let started = Instant::now();
    let records = with_pool(common.threads, || {
        walk.files
            .par_iter()
            .map(|p| convert_file(p, &ctx))
            .collect::<Vec<_>>()
    })?;
    print_records(&records, console, Some(target.tag()));
    let elapsed = started.elapsed();

    let report = finalize_report(
        "convert",
        RunFacts {
            root: Some(common.root.clone()),
            url: None,
            options: options_block(
                common,
                Some(&args as &dyn ConvertOptions),
                &filter,
                config_file.as_deref(),
                ignore_recorded.as_deref(),
            ),
            started_at,
            verify_failed: None,
        },
        records,
        elapsed,
    );
    let summary = match &report.summary {
        Summary::Convert(s) => s.clone(),
        _ => ConvertSummary::default(),
    };
    if let Some(path) = &common.report {
        write_report(&report, path)?;
    }
    print_convert_summary(&summary, common.report.as_deref(), elapsed, console);
    Ok(
        if summary.unknown > 0
            || summary.decode_errors > 0
            || summary.encode_errors > 0
            || summary.io_errors > 0
        {
            1
        } else {
            0
        },
    )
}

/// `verify`: assert all whitelisted files are UTF-8 without BOM.
/// Exit 0 on success, 3 on any failure.
fn cmd_verify(args: &TreeArgs) -> anyhow::Result<i32> {
    let mut args = args.clone();
    let (config_file, _) = merge_tree_config(&mut args)?;
    ensure_root(&args.root)?;
    let filter = build_filter(&args)?;
    if let Some(path) = &args.report {
        ensure_report_dir(path)?;
    }
    let console = console_cfg(&args);
    print_header(console, format!("mode=verify root={}", args.root.display()));

    let (ignore, ignore_recorded, ignore_warnings) = load_ignore(&args)?;
    let mut warnings = Vec::new();
    let walk = {
        let skip = skip_paths(
            args.report.as_deref(),
            ignore_recorded
                .as_deref()
                .map(std::path::PathBuf::from)
                .as_deref(),
        );
        let walk = collect_files(&args.root, &filter, &skip, ignore.as_ref());
        warnings.extend(ignore_warnings);
        walk
    };
    warnings.extend(walk.warnings);
    print_warnings(&warnings, console);
    let started_at = wall_now();
    let started = Instant::now();
    let records = with_pool(args.threads, || {
        walk.files
            .par_iter()
            .map(|p| verify_file(p, args.effective_max_file_size()))
            .collect::<Vec<_>>()
    })?;
    print_records(&records, console, None);
    let elapsed = started.elapsed();

    let report = finalize_report(
        "verify",
        RunFacts {
            root: Some(args.root.clone()),
            url: None,
            options: options_block(
                &args,
                None,
                &filter,
                config_file.as_deref(),
                ignore_recorded.as_deref(),
            ),
            started_at,
            verify_failed: None,
        },
        records,
        elapsed,
    );
    let summary = match &report.summary {
        Summary::Verify(s) => s.clone(),
        _ => VerifySummary::default(),
    };
    if let Some(path) = &args.report {
        write_report(&report, path)?;
    }
    if !console.quiet {
        println!(
            "[utf8-forge] checked={}  passed={}  failed={}",
            summary.total_files, summary.passed, summary.failed
        );
    }
    print_duration(args.report.as_deref(), elapsed, console);
    Ok(if summary.failed > 0 { 3 } else { 0 })
}

/// `convert --url`: fetch → detect → transcode → save.
///
/// The transcode result is always materialized unless the destination already
/// holds the identical bytes — in that case the record is `unchanged` and no
/// write happens (same no-churn contract as tree mode; a
/// missing destination is still written, so downloads always materialize).
/// Exit 1 on fetch/decode/unknown/too-large issues; the report is still
/// written for post-mortem.
#[cfg(feature = "remote")]
fn cmd_convert_url(args: &ConvertArgs, url: &str) -> anyhow::Result<i32> {
    let common = &args.common;
    if args.backup_dir.is_some() {
        bail!("--backup-dir is not supported with --url");
    }
    // Fetch reads no config file: the only mergeable key ([convert].target)
    // would silently change nothing here, so combining them is refused.
    if common.config.is_some() {
        bail!("--config is not supported with --url");
    }
    // Same report pre-flight as the tree modes: an unwritable report aborts
    // before the download, so no output is produced for a run that cannot be
    // audited.
    if let Some(path) = &common.report {
        ensure_report_dir(path)?;
    }
    let console = console_cfg(common);
    print_header(
        console,
        format!("mode=fetch target={} url={url}", args.target_or_default()),
    );
    let started_at = wall_now();
    let started = Instant::now();
    let out_path = args
        .output
        .clone()
        .unwrap_or_else(|| fetcher::derive_output_path(url));

    let record = match fetcher::fetch_bytes(url, common.effective_max_file_size()) {
        Err(e) => {
            // An oversized payload is the documented `too_large` bucket; any
            // other fetch failure is a generic error row. The console line
            // must match the report action.
            let action = match e {
                ForgeError::TooLarge { .. } => "skipped_too_large",
                _ => "error",
            };
            // The oversized bucket keeps its immediate console line (its
            // report action has no problem print); generic errors are printed
            // exactly once, by `print_records`, against the output path.
            if action == "skipped_too_large" {
                print_problem(format!("skipped_too_large {url} ({}: {e})", e.category()));
            }
            RecBuilder::new(&out_path).done(
                None,
                action,
                None,
                None,
                Some(format!("{}: {e}", e.category())),
            )
        }
        Ok(bytes) => {
            let raw_len = bytes.len() as u64;
            let det = detect(&bytes);
            let tag = det.tag();
            match transcode_detected(&bytes, det, args.target_or_default()) {
                Err(e) => {
                    // Printed exactly once, by `print_records`, against the
                    // output path — the same single-line contract as the
                    // generic fetch-error branch.
                    RecBuilder::new(&out_path).done(
                        Some(tag),
                        "error",
                        Some(raw_len),
                        None,
                        Some(format!("{}: {e}", e.category())),
                    )
                }
                Ok(outcome) => {
                    let new_bytes = match outcome {
                        TranscodeOutcome::Unchanged => bytes,
                        TranscodeOutcome::Changed(v) => v,
                    };
                    // Destination already holds the identical bytes → same
                    // no-churn contract as tree mode: `unchanged`, no write.
                    if read_file_limited(&out_path, new_bytes.len() as u64)
                        .is_ok_and(|existing| existing == new_bytes)
                    {
                        RecBuilder::new(&out_path).done(
                            Some(tag),
                            "unchanged",
                            Some(raw_len),
                            Some(new_bytes.len() as u64),
                            None,
                        )
                    } else if args.dry_run {
                        RecBuilder::new(&out_path).done(
                            Some(tag),
                            "would_convert",
                            Some(raw_len),
                            Some(new_bytes.len() as u64),
                            None,
                        )
                    } else {
                        if let Some(parent) = out_path.parent()
                            && !parent.as_os_str().is_empty()
                        {
                            fs::create_dir_all(parent).with_context(|| {
                                format!("cannot create output dir {}", parent.display())
                            })?;
                        }
                        match writer::atomic_write(&out_path, &new_bytes) {
                            Ok(()) => RecBuilder::new(&out_path).done(
                                Some(tag),
                                "converted",
                                Some(raw_len),
                                Some(new_bytes.len() as u64),
                                None,
                            ),
                            Err(e) => RecBuilder::new(&out_path).done(
                                Some(tag),
                                "error",
                                Some(raw_len),
                                None,
                                Some(format!("{}: write failed: {e}", e.category())),
                            ),
                        }
                    }
                }
            }
        }
    };
    let elapsed = started.elapsed();

    let options = OptionsBlock {
        target: args.target_or_default().tag(),
        dry_run: args.dry_run,
        extensions: Vec::new(),
        exclude_dirs: Vec::new(),
        exclude_ext: Vec::new(),
        exclude_globs: Vec::new(),
        backup_dir: None,
        config_file: None,
        ignore_file: None,
    };
    let report = finalize_report(
        "fetch",
        RunFacts {
            root: None,
            url: Some(url.to_string()),
            options,
            started_at,
            verify_failed: None,
        },
        vec![record],
        elapsed,
    );
    let summary = match &report.summary {
        Summary::Convert(s) => s.clone(),
        _ => ConvertSummary::default(),
    };
    print_records(
        std::slice::from_ref(&report.files[0]),
        console,
        Some(args.target_or_default().tag()),
    );
    if let Some(path) = &common.report {
        write_report(&report, path)?;
    }
    print_convert_summary(&summary, common.report.as_deref(), elapsed, console);
    // Unlike tree mode, an oversized payload fails the run: the single file
    // the user asked to save was not written.
    Ok(
        if summary.unknown > 0
            || summary.decode_errors > 0
            || summary.encode_errors > 0
            || summary.io_errors > 0
            || summary.fetch_error > 0
            || summary.too_large > 0
        {
            1
        } else {
            0
        },
    )
}

// ---------------------------------------------------------------------------
// fix / diff subcommands
// ---------------------------------------------------------------------------

/// `fix`: convert + verify in one pass over a single traversal.
///
/// Exit code describes the END state of the tree — the gate wins: `3` when
/// any file fails the verify pass (unknown files, oversized files, and I/O
/// errors all fail closed), else `1` for convert-side issues, else `0`.
/// With `--dry-run` the convert pass writes nothing but the verify pass still
/// runs read-only, so the preview reports the tree's current end state.
fn cmd_fix(args: &FixArgs) -> anyhow::Result<i32> {
    let mut args = args.clone();
    let config_file = merge_convert_config(&mut args)?;
    let common = &args.common;
    let target = args.target_or_default();
    // The verify gate asserts UTF-8 without BOM no matter what the convert
    // pass targeted, so any other target would guarantee exit 3 while having
    // rewritten the tree into a state the gate itself rejects. Refuse it up
    // front, covering both the CLI flag and the config file.
    if target != TargetEncoding::Utf8 {
        bail!(
            "fix requires --target utf-8 (the verify gate asserts UTF-8 without BOM); got `{}`",
            target
        );
    }
    ensure_root(&common.root)?;
    let filter = build_filter(common)?;
    // Same ordering as convert: probe the report path before creating the
    // backup directory, so a rejected report leaves nothing behind.
    if let Some(path) = &common.report {
        ensure_report_dir(path)?;
    }
    ensure_backup_dir(&args)?;
    let console = console_cfg(common);
    let dry_tag = if args.dry_run { " dry_run=true" } else { "" };
    print_header(
        console,
        format!(
            "mode=fix target={} root={}{}",
            target,
            common.root.display(),
            dry_tag
        ),
    );

    let (ignore, ignore_recorded, ignore_warnings) = load_ignore(common)?;
    let mut warnings = ignore_warnings;
    let walk = {
        let skip = skip_paths(
            common.report.as_deref(),
            ignore_recorded.as_deref().map(PathBuf::from).as_deref(),
        );
        collect_files(&common.root, &filter, &skip, ignore.as_ref())
    };
    warnings.extend(walk.warnings);
    print_warnings(&warnings, console);

    let started_at = wall_now();
    let started = Instant::now();
    let ctx = ConvertCtx {
        root: common.root.clone(),
        target,
        dry_run: args.dry_run,
        backup_dir: args.backup_dir.clone(),
        max_file_size: common.effective_max_file_size(),
    };
    let convert_records = with_pool(common.threads, || {
        walk.files
            .par_iter()
            .map(|p| convert_file(p, &ctx))
            .collect::<Vec<_>>()
    })?;
    print_records(&convert_records, console, Some(target.tag()));

    // Verify pass over the SAME traversal — no re-walk, so both passes see an
    // identical candidate list.
    let verify_records = with_pool(common.threads, || {
        walk.files
            .par_iter()
            .map(|p| verify_file(p, common.effective_max_file_size()))
            .collect::<Vec<_>>()
    })?;
    let mut verify_failed: Vec<String> = verify_records
        .iter()
        .filter(|r| r.action == "failed")
        .map(|r| r.path.clone())
        .collect();
    verify_failed.sort();
    for r in &verify_records {
        if r.action == "failed" {
            let reason = r
                .error
                .clone()
                .unwrap_or_else(|| format!("detected={}", r.detected.unwrap_or("?")));
            print_problem(format!("failed {} ({reason})", r.path));
        }
    }
    let elapsed = started.elapsed();

    let mut report = finalize_report(
        "fix",
        RunFacts {
            root: Some(common.root.clone()),
            url: None,
            options: options_block(
                common,
                Some(&args as &dyn ConvertOptions),
                &filter,
                config_file.as_deref(),
                ignore_recorded.as_deref(),
            ),
            started_at,
            verify_failed: Some(verify_failed.clone()),
        },
        convert_records,
        elapsed,
    );
    // The verify pass counts ride on the fix summary; they are computed from
    // the gate records, not the convert records.
    let verify_failed_count = verify_failed.len() as u64;
    if let Summary::Fix(f) = &mut report.summary {
        f.passed = verify_records
            .iter()
            .filter(|r| r.action == "passed")
            .count() as u64;
        f.failed = verify_failed_count;
    }
    let convert_issues = match &report.summary {
        Summary::Fix(f) => f.unknown + f.decode_errors + f.encode_errors + f.io_errors,
        _ => 0,
    };
    if let Some(path) = &common.report {
        write_report(&report, path)?;
    }
    if let Summary::Fix(f) = &report.summary
        && !console.quiet
    {
        println!(
            "[utf8-forge] scanned={}  candidates={}",
            f.total_files, f.candidates
        );
        println!(
            "[utf8-forge] already_utf8={}  converted={}  unknown={}  errors={}",
            f.already_utf8,
            f.converted,
            f.unknown,
            f.decode_errors + f.encode_errors + f.io_errors
        );
        if f.too_large > 0 {
            println!("[utf8-forge] too_large={}", f.too_large);
        }
        println!(
            "[utf8-forge] verify: checked={}  passed={}  failed={}",
            f.passed + f.failed,
            f.passed,
            f.failed
        );
    }
    print_duration(common.report.as_deref(), elapsed, console);
    // Gate wins: the exit code describes the end state of the tree.
    Ok(if verify_failed_count > 0 {
        3
    } else if convert_issues > 0 {
        1
    } else {
        0
    })
}

/// `diff`: compare two report JSONs per path. Inspection tool — exit 0 by
/// default even when differences exist; `--fail-on-change` turns a non-empty
/// diff into exit 1 for CI gates.
fn cmd_diff(args: &DiffArgs) -> anyhow::Result<i32> {
    let load = |p: &Path, flag: &str| -> anyhow::Result<DiffInput> {
        let text = fs::read_to_string(p)
            .with_context(|| format!("cannot load {flag} report `{}`", p.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("cannot load {flag} report `{}`", p.display()))
    };
    let before_raw = load(&args.before, "--before")?;
    let after_raw = load(&args.after, "--after")?;
    if let (Some(r1), Some(r2)) = (&before_raw.root, &after_raw.root)
        && r1 != r2
    {
        print_problem(format!(
            "warning: roots differ (`{r1}` vs `{r2}`); paths are compared relative to each root"
        ));
    }

    // Records are compared relative to each report's own root, so the same
    // tree scanned under two root directories diffs clean.
    let before = DiffInput {
        mode: before_raw.mode.clone(),
        root: before_raw.root.clone(),
        files: differ::relativize_files(before_raw.files, before_raw.root.as_deref()),
    };
    let after = DiffInput {
        mode: after_raw.mode.clone(),
        root: after_raw.root.clone(),
        files: differ::relativize_files(after_raw.files, after_raw.root.as_deref()),
    };
    let mut doc = differ::compare(&before, &after);
    doc.before = args.before.to_string_lossy().into_owned();
    doc.after = args.after.to_string_lossy().into_owned();
    let summary = &doc.summary;

    // The summary line goes to stderr; stdout carries only the JSON document
    // (when no --report), so piping the diff into other tools stays clean.
    if let Some(path) = &args.report {
        ensure_report_dir(path)?;
        let json = serde_json::to_string_pretty(&doc).context("diff serialization failed")?;
        writer::atomic_write(path, (json + "\n").as_bytes())
            .with_context(|| format!("cannot write diff report {}", path.display()))?;
        eprintln!(
            "[utf8-forge] diff: added={}  removed={}  changed={}  unchanged={}  report={}",
            summary.added,
            summary.removed,
            summary.action_changed,
            summary.unchanged,
            path.display()
        );
    } else {
        eprintln!(
            "[utf8-forge] diff: added={}  removed={}  changed={}  unchanged={}",
            summary.added, summary.removed, summary.action_changed, summary.unchanged
        );
        let json = serde_json::to_string_pretty(&doc).context("diff serialization failed")?;
        println!("{json}");
    }
    let drifted = summary.added + summary.removed + summary.action_changed;
    Ok(if args.fail_on_change && drifted > 0 {
        1
    } else {
        0
    })
}

#[cfg(test)]
mod bounded_read_tests {
    use super::read_file_limited;
    use std::fs;

    #[test]
    fn reads_only_cap_plus_one_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing.cpp");
        // Exercise the read independently of the stat pre-check, as when a
        // file grows between the pre-check and opening it.
        fs::write(&path, vec![b'x'; 65536]).unwrap();
        let bytes = read_file_limited(&path, 4096).unwrap();
        assert_eq!(bytes, vec![b'x'; 4097]);
        assert_eq!(read_file_limited(&path, 0).unwrap(), b"x");
    }

    #[test]
    fn handles_exact_limit_empty_files_and_unlimited_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cpp");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(read_file_limited(&path, 3).unwrap(), b"abc");
        assert_eq!(read_file_limited(&path, u64::MAX).unwrap(), b"abc");
        fs::write(&path, b"").unwrap();
        assert!(read_file_limited(&path, 0).unwrap().is_empty());
        assert_eq!(
            read_file_limited(&dir.path().join("missing.cpp"), 3)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }
}

#[cfg(test)]
mod backup_placement_tests {
    use super::check_backup_placement;
    use std::path::Path;

    #[test]
    fn relative_backup_dir_resolves_against_cwd() {
        // The root argument must be canonicalized, exactly as the real
        // caller passes it: on Windows canonicalize yields a `\\?\`
        // verbatim prefix, so a plain current_dir() would never compare
        // equal to the projected path.
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        // A not-yet-created relative dir lands under the cwd, outside a root
        // elsewhere.
        check_backup_placement(Path::new("backup"), &cwd.join("elsewhere-root")).unwrap();
        // Deep non-existing relative tails resolve through the cwd too.
        check_backup_placement(Path::new("a/b/c"), &cwd.join("elsewhere-root")).unwrap();
        // The same relative dir is fatal when the cwd itself is the root.
        let err = check_backup_placement(Path::new("backup"), &cwd).unwrap_err();
        assert!(err.to_string().contains("inside"), "{err}");
    }
}
