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

//! Recursive candidate collection on top of `walkdir`.
//!
//! - Symlinks are never followed, neither directories nor files: a symlinked
//!   directory is not descended into and a symlinked file is skipped.
//! - Directory-name / prefix exclusions prune directories during traversal
//!   (never descended into); the same rule deliberately does **not** drop
//!   files — a file named `outputfiles_config.md` is legitimate content, and
//!   files are gated by the extension whitelist anyway.
//! - The root entry itself is always kept: an explicitly given `--root` must
//!   be honored even when its basename collides with an exclusion rule.
//! - `skip` paths (e.g. the `--report` output) are excluded from collection so
//!   a report written inside the tree never pollutes later runs.
//! - Walk errors (permission, ...) are collected as warnings, never fatal.

use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

use crate::filter::{FilterConfig, extension_of};
use crate::ignorefile::IgnoreFile;

/// Collected candidates plus non-fatal traversal warnings.
#[derive(Debug, Default)]
pub struct WalkResult {
    /// Whitelisted files, sorted by path for deterministic processing.
    pub files: Vec<PathBuf>,
    /// Human-readable traversal warnings (unreadable directories etc.).
    pub warnings: Vec<String>,
}

/// Collect whitelisted files under `root`, sorted by path so every run
/// processes and reports files in the same deterministic order.
///
/// `skip` holds absolute/canonical paths that must never be collected (the
/// `--report` output; currently-existing paths are canonicalized, and paths
/// whose parent exists are canonicalized through the parent so not-yet-written
/// reports resolve correctly).
pub fn collect_files(
    root: &Path,
    cfg: &FilterConfig,
    skip: &[PathBuf],
    ignore: Option<&IgnoreFile>,
) -> WalkResult {
    let mut result = WalkResult::default();
    let skip_set = canonical_skip_set(skip);
    let root_canonical = fs_canonicalize_or(root, root);
    let use_skip = !skip_set.is_empty();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| keep_entry(e, cfg, root, ignore))
    {
        match entry {
            Ok(e) => {
                if !e.file_type().is_file() {
                    continue;
                }
                let path = e.path();
                if use_skip && is_skipped(path, root, &root_canonical, &skip_set) {
                    continue;
                }
                let Some(ext) = extension_of(path) else {
                    continue;
                };
                if !cfg.extensions.contains(&ext) {
                    continue;
                }
                if is_glob_excluded(path, root, cfg) {
                    continue;
                }
                result.files.push(path.to_path_buf());
            }
            Err(err) => {
                result.warnings.push(format!(
                    "walk warning at {}: {err}",
                    err.path().unwrap_or(root).display()
                ));
            }
        }
    }

    result.files.sort();
    result
}

/// Canonicalize every existing skip path (best effort); skip paths whose
/// parent exists resolve through the parent + file name.
fn canonical_skip_set(skip: &[PathBuf]) -> Vec<PathBuf> {
    skip.iter()
        .filter_map(|p| {
            if let Ok(c) = std::fs::canonicalize(p) {
                return Some(c);
            }
            let parent = p.parent()?;
            let parent_c = std::fs::canonicalize(parent).ok()?;
            let name = p.file_name()?;
            Some(parent_c.join(name))
        })
        .collect()
}

/// True when the walked entry resolves to a skipped path. Entry paths are
/// rebuilt against the canonical root (no per-entry syscalls — traversal does
/// not follow symlinks, so the entries are real).
fn is_skipped(path: &Path, root: &Path, root_canonical: &Path, skip_set: &[PathBuf]) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let candidate = root_canonical.join(rel);
    skip_set.iter().any(|s| s == &candidate)
}

/// Entry gate for traversal: keep the root, drop excluded/hidden dirs and
/// dot-files, drop glob-excluded paths.
fn keep_entry(e: &DirEntry, cfg: &FilterConfig, root: &Path, ignore: Option<&IgnoreFile>) -> bool {
    // Always process the explicit root itself; ignore patterns never prune it.
    if e.depth() == 0 {
        return true;
    }
    // Directory-name / prefix exclusion applies to directories only; files
    // with excluded-looking names are gated by the extension whitelist.
    if e.file_type().is_dir() && cfg.is_excluded_dir_name(e.file_name()) {
        return false;
    }
    if cfg.is_hidden_name(e.file_name()) {
        return false;
    }
    if let Some(rules) = ignore {
        let rel = match e.path().strip_prefix(root) {
            Ok(r) => r,
            Err(_) => return true,
        };
        let components: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let refs: Vec<&str> = components.iter().map(String::as_str).collect();
        if rules.ignores(&refs, e.file_type().is_dir()) {
            return false;
        }
    }
    !is_glob_excluded(e.path(), root, cfg)
}

/// Glob check against the root-relative path (safety net on top of dir
/// pruning, and the primary gate for file paths).
fn is_glob_excluded(path: &Path, root: &Path, cfg: &FilterConfig) -> bool {
    if cfg.exclude_globs.is_empty() {
        return false;
    }
    let rel = path.strip_prefix(root).unwrap_or(path);
    let components: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let refs: Vec<&str> = components.iter().map(String::as_str).collect();
    cfg.is_excluded_by_glob(&refs)
}

/// `fs::canonicalize` with a fallback that keeps the input untouched.
fn fs_canonicalize_or(path: &Path, fallback: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| fallback.to_path_buf())
}
