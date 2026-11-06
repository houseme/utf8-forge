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

//! Optional remote download.
//!
//! Compiled only with the `remote` feature (default). Reuses the exact same
//! `detect()` / `transcode()` pipeline as the local path — only the source of
//! `raw_bytes` changes. Downloaded content is treated as untrusted text:
//! strict decoding rejects malformed byte sequences, no shell is executed,
//! no recursive link following.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::ForgeError;
use crate::writer::unique_suffix;

/// Fetch `url`, capped by `max_size`, streaming the body to a temp file.
///
/// `Content-Length` is checked before reading when present; the cap is then
/// enforced per chunk while streaming, so an oversized body is aborted
/// mid-download (temp deleted) instead of being buffered in full. At most
/// `max_size` bytes are ever held in memory (the final read-back). Redirects
/// are limited to 5; connect timeout is 30 s and the total request timeout
/// is 120 s.
pub fn fetch_bytes(url: &str, max_size: u64) -> Result<Vec<u8>, ForgeError> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| ForgeError::Fetch(format!("client build failed: {e}")))?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| ForgeError::Fetch(format!("request failed: {e}")))?
        .error_for_status()
        .map_err(|e| ForgeError::Fetch(format!("http status: {e}")))?;

    if let Some(len) = resp.content_length()
        && len > max_size
    {
        return Err(ForgeError::TooLarge {
            size: len,
            max: max_size,
        });
    }

    // Stream the body into a unique temp file, enforcing the cap per chunk:
    // an oversized payload never accumulates in memory.
    let tmp: PathBuf = std::env::temp_dir().join(format!(".utf8-forge-fetch-{}", unique_suffix()));
    let cleanup = |p: &Path| {
        let _ = fs::remove_file(p);
    };
    let mut file = fs::File::create(&tmp)
        .map_err(|e| ForgeError::Fetch(format!("create temp file failed: {e}")))?;
    let mut total: u64 = 0;
    let mut buf = [0u8; 8192];
    loop {
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > max_size {
                    cleanup(&tmp);
                    return Err(ForgeError::TooLarge {
                        size: total,
                        max: max_size,
                    });
                }
                if let Err(e) = file.write_all(&buf[..n]) {
                    cleanup(&tmp);
                    return Err(ForgeError::Fetch(format!("write temp file failed: {e}")));
                }
            }
            Err(e) => {
                cleanup(&tmp);
                return Err(ForgeError::Fetch(format!("read body failed: {e}")));
            }
        }
    }
    // Defense-in-depth: the per-chunk check already guarantees this.
    if total > max_size {
        cleanup(&tmp);
        return Err(ForgeError::TooLarge {
            size: total,
            max: max_size,
        });
    }
    let body = match fs::read(&tmp) {
        Ok(bytes) => bytes,
        Err(e) => {
            cleanup(&tmp);
            return Err(ForgeError::Fetch(format!("read temp file failed: {e}")));
        }
    };
    cleanup(&tmp);
    Ok(body)
}

/// Derive the output path from the URL when `--output` is omitted:
///
/// - last non-empty path segment with an extension is kept verbatim
///   (`.../legacy.cpp` → `legacy.cpp`);
/// - a segment without an extension gets `.utf8` appended (`.../foo` → `foo.utf8`);
/// - no usable segment → `download.utf8`.
pub fn derive_output_path(url: &str) -> PathBuf {
    // Scan backwards so a trailing slash (`.../dir/`) cannot shadow the real
    // last segment with an empty one.
    let segment = reqwest::Url::parse(url).ok().and_then(|u| {
        u.path_segments()?
            .rev()
            .find(|s| !s.is_empty())
            .map(str::to_owned)
    });

    match segment {
        Some(seg) if has_extension(&seg) => PathBuf::from(seg),
        Some(seg) => PathBuf::from(format!("{seg}.utf8")),
        None => PathBuf::from("download.utf8"),
    }
}

/// True when the last URL segment looks like a filename with an extension.
fn has_extension(segment: &str) -> bool {
    std::path::Path::new(segment)
        .extension()
        .map(|e| !e.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_keeps_extensioned_segments() {
        assert_eq!(
            derive_output_path("https://example.com/data/legacy.cpp"),
            PathBuf::from("legacy.cpp")
        );
    }

    #[test]
    fn derive_appends_utf8_without_extension() {
        assert_eq!(
            derive_output_path("https://example.com/foo"),
            PathBuf::from("foo.utf8")
        );
    }

    #[test]
    fn derive_falls_back_when_no_segment() {
        assert_eq!(
            derive_output_path("https://example.com/"),
            PathBuf::from("download.utf8")
        );
        assert_eq!(
            derive_output_path("not a url"),
            PathBuf::from("download.utf8")
        );
    }

    #[test]
    fn derive_ignores_query_string() {
        assert_eq!(
            derive_output_path("https://example.com/a.cpp?raw=1"),
            PathBuf::from("a.cpp")
        );
    }

    #[test]
    fn derive_treats_trailing_dot_as_missing_extension() {
        assert_eq!(
            derive_output_path("https://example.com/foo."),
            PathBuf::from("foo..utf8")
        );
    }

    #[test]
    fn derive_skips_trailing_slash_segments() {
        // A trailing slash yields an empty last segment; the real last
        // non-empty segment wins instead of falling through to the fallback.
        assert_eq!(
            derive_output_path("https://example.com/a/dir.txt/"),
            PathBuf::from("dir.txt")
        );
        assert_eq!(
            derive_output_path("https://example.com/a/noext/"),
            PathBuf::from("noext.utf8")
        );
    }
}
