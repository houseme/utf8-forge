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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ForgeError;

/// Temp-name suffix with a process-local sequence independent of clock resolution.
/// Shared with the fetcher's streaming temp files.
pub(crate) fn unique_suffix() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", process::id())
}

/// Atomically replace `path` with `bytes`.
///
/// The temp file is created in the destination's parent directory (same
/// volume, so rename is atomic), given the original's permissions, fsynced,
/// then renamed over the destination. Temp leftovers are cleaned up on error.
/// An existing destination that is a symlink or not a regular file is
/// refused: rename would silently replace the entry itself (the link or the
/// FIFO) while the operator's target keeps stale content.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ForgeError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            return Err(ForgeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "destination `{}` is a symlink; refusing to replace the link itself \
                     (remove the link or point the output elsewhere)",
                    path.display()
                ),
            )));
        }
        if !file_type.is_file() {
            return Err(ForgeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "destination `{}` is not a regular file; refusing to replace it",
                    path.display()
                ),
            )));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Keep the temp basename short even when the destination approaches the
    // filesystem's name limit. Exclusive creation also refuses collisions
    // and symlinks; cleanup starts only after this process owns the temp.
    let tmp = parent.join(format!(".utf8-forge-tmp-{}", unique_suffix()));
    let mut f = fs::File::create_new(&tmp)?;

    let write_result = (|| -> std::io::Result<()> {
        f.write_all(bytes)?;
        // Preserve permissions before syncing, and fail without replacing
        // the original if its permissions cannot be copied.
        match fs::metadata(path) {
            Ok(meta) => f.set_permissions(meta.permissions())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        f.sync_all()?;
        drop(f);
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
/// tree structure relative to `root`, and carry the source file's permission
/// bits over to the backup (the rollback copy stays executable/private the
/// way the original was).
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
    atomic_write(&target, original)?;
    // The write above copies the *destination's* metadata, which does not
    // exist on a first-time backup — apply the source's mode afterwards so a
    // 750 original does not restore as a 644 backup. A failure here is loud:
    // bytes are already safe, but the operator must know the mode drifted.
    let perms = fs::metadata(path).map(|m| m.permissions());
    if let Ok(perms) = perms {
        fs::set_permissions(&target, perms)?;
    }
    Ok(())
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
    fn concurrent_writes_produce_complete_content_without_temp_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.cpp");
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16u8)
                .map(|value| {
                    let path = &path;
                    scope.spawn(move || atomic_write(path, &vec![value; 4096]))
                })
                .collect();
            for handle in handles {
                handle.join().unwrap().unwrap();
            }
        });
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 4096);
        assert!(bytes[0] < 16);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
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

    #[test]
    fn existing_directory_destination_is_refused_without_temp_leftovers() {
        // rename() could never replace a directory; the pre-write check now
        // refuses it with a clear message instead of a raw OS error.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("occupied");
        fs::create_dir(&destination).unwrap();
        let err = atomic_write(&destination, b"replacement").unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_refused_and_link_survives() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.cpp");
        fs::write(&target, b"original").unwrap();
        let link = dir.path().join("link.cpp");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = atomic_write(&link, b"replacement").unwrap_err();
        assert!(err.to_string().contains("is a symlink"), "{err}");
        // Neither the link nor its target may be touched.
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(fs::read(&link).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn backup_preserves_source_permission_bits() {
        let root = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let src = root.path().join("s.cpp");
        fs::write(&src, b"content").unwrap();
        let mut perms = fs::metadata(&src).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o740);
        fs::set_permissions(&src, perms).unwrap();
        backup_original(root.path(), &src, backup.path(), b"content").unwrap();
        let backed = backup.path().join("s.cpp");
        assert_eq!(fs::read(&backed).unwrap(), b"content");
        assert_eq!(
            fs::metadata(&backed).unwrap().permissions().mode() & 0o777,
            0o740
        );
    }
}
