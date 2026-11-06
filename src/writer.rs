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

//! Crash-safe persistence.
//!
//! Never truncate-then-write: every overwrite goes through a temp file in the
//! same directory + `fs::rename`, so a crash mid-write leaves either the old
//! or the new content, never neither. Backups copy the original bytes
//! *before* the rename, mirroring the tree structure under `--backup-dir`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ForgeError;

/// Monotonic-enough uniqueness source for temp names (pid + wall nanos).
/// Shared with the fetcher's streaming temp files.
pub(crate) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", process::id(), nanos)
}

/// Atomically replace `path` with `bytes`.
///
/// The temp file is created in the destination's parent directory (same
/// volume, so rename is atomic), fsynced, given the original's permissions,
/// then renamed over the destination. Temp leftovers are cleaned up on error.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ForgeError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let tmp = parent.join(format!(".utf8-forge-tmp-{}-{}", unique_suffix(), file_name));

    let write_result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // Preserve the original file's permissions when replacing it.
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, path)?;
        // Best-effort directory fsync: without it a machine crash right after
        // the rename can lose the rename itself. Ignored where the platform
        // cannot open a directory as a file (e.g. Windows).
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    match write_result {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup of the temp file; the original is intact.
            let _ = fs::remove_file(&tmp);
            Err(ForgeError::Io(e))
        }
    }
}

/// Copy the original bytes of `path` into `backup_dir`, mirroring the
/// tree structure relative to `root`.
///
/// Called *before* the converted bytes are renamed into place, so the backup
/// always holds the pre-conversion content and stays replayable.
pub fn backup_original(
    root: &Path,
    path: &Path,
    backup_dir: &Path,
    original: &[u8],
) -> Result<(), ForgeError> {
    let rel = path.strip_prefix(root).map_err(|_| {
        ForgeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "file {} is not under root {}",
                path.display(),
                root.display()
            ),
        ))
    })?;
    let target: PathBuf = backup_dir.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&target, original)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn atomic_write_replaces_content_and_cleans_up_temps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, b"old").unwrap();

        atomic_write(&p, b"new-bytes").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new-bytes");

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".utf8-forge-tmp")
            })
            .collect();
        assert!(leftovers.is_empty(), "temp files must be renamed away");
    }

    #[test]
    fn atomic_write_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("b.txt");
        // Parent does not exist → io error, original "missing" state preserved.
        assert!(atomic_write(&p, b"x").is_err());
        let p2 = dir.path().join("c.txt");
        atomic_write(&p2, b"x").unwrap();
        assert_eq!(fs::read(&p2).unwrap(), b"x");
    }

    #[test]
    fn backup_mirrors_relative_structure() {
        let root = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let src = root.path().join("GameServer").join("Channel.cpp");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"original").unwrap();

        backup_original(root.path(), &src, backup.path(), b"original").unwrap();
        let backed = backup.path().join("GameServer").join("Channel.cpp");
        assert_eq!(fs::read(&backed).unwrap(), b"original");
    }

    #[test]
    fn backup_rejects_path_outside_root() {
        let root = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let src = outside.path().join("x.cpp");
        fs::write(&src, b"x").unwrap();
        let res = backup_original(root.path(), &src, backup.path(), b"x");
        assert!(res.is_err());
    }
}
