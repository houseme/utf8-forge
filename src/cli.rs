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

//! clap definitions for the CLI surface.
//!
//! All flags carry `///` doc comments in English.
//! clap parse failures exit with code 2, matching the documented fatal-code
//! contract; semantic conflicts (opt-in vs `--exclude-ext`) are validated in
//! [`FilterConfig::build`](crate::filter::FilterConfig::build).
//!
//! The flag surface is normalized per subcommand: `scan` / `verify` accept
//! only detection + filtering + reporting options; conversion-only options
//! (`--target`, `--dry-run`, `--backup-dir`, `--url`, `--output`) exist solely
//! on `convert`, so misuse fails fast with exit 2 instead of being ignored.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::transcoder::TargetEncoding;

/// Built-in per-file size cap (200 MiB) used when neither the CLI nor the
/// config file supplies `--max-file-size`.
pub(crate) const DEFAULT_MAX_FILE_SIZE: u64 = 200 * 1024 * 1024;

/// utf8-forge — a focused, idempotent, bidirectional charset transcoder.
#[derive(Debug, Parser)]
#[command(
    name = "utf8-forge",
    version,
    about = "Idempotent charset transcoder for source trees (default target: UTF-8 without BOM)"
)]
pub struct Cli {
    /// Selected subcommand.
    #[command(subcommand)]
    pub command: Commands,
}

/// Subcommands. Named `Commands` to avoid colliding with
/// the imported `clap::Subcommand` derive trait.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Detect only; report distribution (no writes).
    Scan(TreeArgs),
    /// Detect + transcode + atomic write (supports --dry-run, --url).
    Convert(ConvertArgs),
    /// Assert all whitelisted files are UTF-8 without BOM.
    Verify(TreeArgs),
    /// Detect + transcode + verify in one pass (exit 3 when the gate fails).
    Fix(FixArgs),
    /// Compare two utf8-forge JSON reports per path (any mode with files[]).
    Diff(DiffArgs),
}

/// Detection / filtering / reporting options shared by `scan`, `verify`, and
/// `convert` (tree mode). Conversion-only options live on [`ConvertArgs`].
#[derive(Debug, Args, Clone)]
pub struct TreeArgs {
    /// Root directory to process recursively.
    #[arg(short = 'r', long = "root", default_value = ".")]
    pub root: PathBuf,

    /// Override the default extension whitelist (comma-separated).
    #[arg(short = 'e', long = "ext", value_delimiter = ',')]
    pub ext: Vec<String>,

    /// Extra excluded directory names (comma-separated, appended to defaults).
    #[arg(short = 'x', long = "exclude", value_delimiter = ',')]
    pub exclude: Vec<String>,

    /// Extra excluded extensions (comma-separated, appended).
    #[arg(short = 'X', long = "exclude-ext", value_delimiter = ',')]
    pub exclude_ext: Vec<String>,

    /// Extra glob pattern excluding root-relative paths (repeatable).
    #[arg(long = "exclude-glob")]
    pub exclude_glob: Vec<String>,

    /// Include .rc/.rc2/.dlg (rc.exe has limited UTF-8 support).
    #[arg(long = "include-rc")]
    pub include_rc: bool,

    /// Include .sln/.vcxproj/... (VS re-saves these with a BOM).
    #[arg(long = "include-msbuild")]
    pub include_msbuild: bool,

    /// Process dot-prefixed files and directories.
    #[arg(long = "include-hidden")]
    pub include_hidden: bool,

    /// Skip files above this size, recorded as too_large (bytes; default
    /// 200 MiB). CLI value overrides the config file.
    #[arg(long = "max-file-size")]
    pub max_file_size: Option<u64>,

    /// Load defaults from this TOML config file (CLI flags override config
    /// values; missing/malformed file or unknown keys are fatal).
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// Load gitignore-style exclusion patterns from this file (fatal when
    /// missing; auto-discovery of `<root>/.utf8forgeignore` is replaced).
    #[arg(long = "ignore-file", conflicts_with = "no_ignore_file")]
    pub ignore_file: Option<PathBuf>,

    /// Disable ignore-file handling entirely, including auto-discovery of
    /// `<root>/.utf8forgeignore`.
    #[arg(long = "no-ignore-file")]
    pub no_ignore_file: bool,

    /// Rayon worker thread count (default: CPU count; values below 1 are
    /// treated as 1).
    #[arg(long = "threads")]
    pub threads: Option<usize>,

    /// Write a JSON report to this path.
    #[arg(long = "report")]
    pub report: Option<PathBuf>,

    /// Print errors only.
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Print one line per file.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

impl Default for TreeArgs {
    /// Mirrors the clap defaults (`root = "."`, `max_file_size = 200 MiB`) so
    /// programmatic/test invocation behaves exactly like the CLI.
    fn default() -> Self {
        TreeArgs {
            root: PathBuf::from("."),
            ext: Vec::new(),
            exclude: Vec::new(),
            exclude_ext: Vec::new(),
            exclude_glob: Vec::new(),
            include_rc: false,
            include_msbuild: false,
            include_hidden: false,
            max_file_size: Some(DEFAULT_MAX_FILE_SIZE),
            config: None,
            ignore_file: None,
            no_ignore_file: false,
            threads: None,
            report: None,
            quiet: false,
            verbose: false,
        }
    }
}

/// `convert` additions: conversion options + remote fetch. Extends the shared
/// [`TreeArgs`] (whose `--root` is ignored when `--url` is given).
#[derive(Debug, Args, Clone)]
pub struct ConvertArgs {
    /// Shared detection / filtering / reporting options.
    #[command(flatten)]
    pub common: TreeArgs,

    /// Target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be
    /// (default utf-8). CLI value overrides the config file.
    #[arg(short = 't', long = "target")]
    pub target: Option<TargetEncoding>,

    /// Report what would change without writing anything.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Mirror original bytes under this directory before overwriting.
    #[arg(long = "backup-dir")]
    pub backup_dir: Option<PathBuf>,

    /// Fetch this remote file and transcode it (convert only).
    #[arg(long = "url")]
    pub url: Option<String>,

    /// Destination path for --url (derived from the URL when omitted).
    #[arg(long = "output", requires = "url")]
    pub output: Option<PathBuf>,
}

impl Default for ConvertArgs {
    /// Tree-mode defaults with `utf-8` target and no remote fetch.
    fn default() -> Self {
        ConvertArgs {
            common: TreeArgs::default(),
            target: Some(TargetEncoding::Utf8),
            dry_run: false,
            backup_dir: None,
            url: None,
            output: None,
        }
    }
}

impl TreeArgs {
    /// The size cap after config merge (built-in default: 200 MiB).
    pub fn effective_max_file_size(&self) -> u64 {
        self.max_file_size.unwrap_or(DEFAULT_MAX_FILE_SIZE)
    }
}

impl ConvertArgs {
    /// The target encoding after config merge (built-in default: utf-8).
    pub fn target_or_default(&self) -> TargetEncoding {
        self.target.unwrap_or(TargetEncoding::Utf8)
    }
}

/// `fix` args: shared tree options plus conversion options. No `--url` /
/// `--output` — fix is a tree operation only.
#[derive(Debug, Args, Clone)]
pub struct FixArgs {
    /// Shared detection / filtering / reporting options.
    #[command(flatten)]
    pub common: TreeArgs,

    /// Target encoding (default utf-8). CLI value overrides the config file.
    #[arg(short = 't', long = "target")]
    pub target: Option<TargetEncoding>,

    /// Report what would change without writing anything (the verify pass
    /// still runs read-only on the current state).
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Mirror original bytes under this directory before overwriting.
    #[arg(long = "backup-dir")]
    pub backup_dir: Option<PathBuf>,
}

impl Default for FixArgs {
    /// Tree-mode defaults with `utf-8` target.
    fn default() -> Self {
        FixArgs {
            common: TreeArgs::default(),
            target: Some(TargetEncoding::Utf8),
            dry_run: false,
            backup_dir: None,
        }
    }
}

impl FixArgs {
    /// The target encoding after config merge (built-in default: utf-8).
    pub fn target_or_default(&self) -> TargetEncoding {
        self.target.unwrap_or(TargetEncoding::Utf8)
    }
}

/// `diff` args: two report files to compare. No tree options.
#[derive(Debug, Args, Clone)]
pub struct DiffArgs {
    /// The earlier report (JSON, any mode that has files[]).
    #[arg(long = "before")]
    pub before: PathBuf,

    /// The later report.
    #[arg(long = "after")]
    pub after: PathBuf,

    /// Write the diff JSON here; otherwise pretty-print to stdout.
    #[arg(long = "report")]
    pub report: Option<PathBuf>,

    /// Exit 1 when the diff is non-empty (added + removed + changed > 0).
    #[arg(long = "fail-on-change")]
    pub fail_on_change: bool,
}

/// Wrap a subcommand into a full [`Cli`], mirroring what the parser would
/// produce. Handy for tests and programmatic invocation.
impl From<Commands> for Cli {
    fn from(command: Commands) -> Cli {
        Cli { command }
    }
}
