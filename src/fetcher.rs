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
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::ForgeError;
use crate::writer::unique_suffix;

/// Render an error with its root cause appended: in CI the fetch error line
/// is the only diagnostic, so `connection refused` or `dns error` must
/// survive alongside the reqwest wrapper text.
fn err_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        text.push_str(": ");
        text.push_str(&c.to_string());
        cause = c.source();
    }
    text
}

/// Wall-clock budget for the whole download body. The client timeout fires
/// per read, so a slow-but-steady trickle would otherwise never trip it.
const DOWNLOAD_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// Fetch `url`, capped by `max_size`, streaming the body to a temp file.
///
/// `Content-Length` is checked before reading when present; the cap is then
/// enforced per chunk while streaming, so an oversized body is aborted
/// mid-download (temp deleted) instead of being buffered in full. At most
/// `max_size` bytes are ever held in memory (the final read-back). Redirects
/// are limited to 5, and an `https://` request never follows a downgrade
/// redirect to plain `http://` (the client is built with `https_only` for
/// https origins; plain-http requests keep the lenient policy). The connect
/// timeout is 30 s and the whole download must finish within a 120 s
/// wall-clock budget (enforced here, because the client's timeout applies
/// per read). Bodyless success responses (204/304/3xx without a location)
/// and partial-content responses (206) are refused.
pub fn fetch_bytes(url: &str, max_size: u64) -> Result<Vec<u8>, ForgeError> {
    let https_origin = reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https");
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::limited(5));
    if https_origin {
        builder = builder.https_only(true);
    }
    let client = builder
        .build()
        .map_err(|e| ForgeError::Fetch(format!("client build failed: {e}")))?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| {
            ForgeError::Fetch(format!(
                "request failed: {} (network down, DNS, or proxy? check HTTP_PROXY/NO_PROXY)",
                err_chain(&e)
            ))
        })?
        .error_for_status()
        .map_err(|e| ForgeError::Fetch(format!("http status: {}", err_chain(&e))))?;

    if let Some(len) = resp.content_length()
        && len > max_size
    {
        return Err(ForgeError::TooLarge {
            size: len,
            max: max_size,
        });
    }

    // A bodyless success carries no content to transcode — materializing it
    // would silently truncate the output with a zero-byte file. reqwest only
    // rejects 4xx/5xx, and a 3xx without a Location header lands here as a
    // final response.
    let status = resp.status();
    if status.is_redirection()
        || status == reqwest::StatusCode::NO_CONTENT
        || status == reqwest::StatusCode::NOT_MODIFIED
    {
        return Err(ForgeError::Fetch(format!(
            "response has no body (HTTP {status}); nothing to fetch"
        )));
    }
    // A 206 self-declares that it carries only *part* of the resource; this
    // tool sends no Range header, so a terminal 206 means a proxy or CDN
    // truncated the body — refusing it beats silently transcoding half a
    // file.
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(ForgeError::Fetch(
            "server returned partial content (HTTP 206) for a full-body request; \
             refusing a truncated download"
                .into(),
        ));
    }

    // Stream the body into a unique temp file, enforcing the cap per chunk:
    // an oversized payload never accumulates in memory.
    let tmp: PathBuf = std::env::temp_dir().join(format!(".utf8-forge-fetch-{}", unique_suffix()));
    read_body_to_temp(&mut resp, max_size, DOWNLOAD_BUDGET, &tmp)
}

/// Spool a bounded body through an exclusively created temporary file.
/// Creation failures preserve any existing path. Once created, the file is
/// closed before cleanup on both success and error, including on Windows.
/// `budget` bounds the whole body's wall-clock time (the client timeout
/// fires per read, so a slow trickle would otherwise never trip it); the
/// public fetch path passes [`DOWNLOAD_BUDGET`] and tests inject smaller
/// values.
fn read_body_to_temp(
    reader: &mut impl Read,
    max_size: u64,
    budget: Duration,
    tmp: &Path,
) -> Result<Vec<u8>, ForgeError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(|e| ForgeError::Fetch(format!("create temp file failed: {e}")))?;

    let result = (|| {
        // Move the handle into this scope so every return closes it before
        // removal. Reading back through this handle avoids reopening a path
        // that another process could have replaced after creation.
        let mut file = file;
        let started = std::time::Instant::now();
        let mut total: u64 = 0;
        let mut buf = [0u8; 8192];
        loop {
            // The client timeout fires per read (a slow-but-steady trickle
            // never trips it), so the tool enforces its own wall-clock
            // budget on the whole body.
            if started.elapsed() > budget {
                return Err(ForgeError::Fetch(format!(
                    "download exceeded the {} s budget",
                    budget.as_secs()
                )));
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| ForgeError::Fetch(format!("read body failed: {}", err_chain(&e))))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max_size {
                return Err(ForgeError::TooLarge {
                    size: total,
                    max: max_size,
                });
            }
            file.write_all(&buf[..n])
                .map_err(|e| ForgeError::Fetch(format!("write temp file failed: {e}")))?;
        }
        file.rewind()
            .map_err(|e| ForgeError::Fetch(format!("read temp file failed: {e}")))?;
        let mut body = Vec::new();
        file.take(total)
            .read_to_end(&mut body)
            .map_err(|e| ForgeError::Fetch(format!("read temp file failed: {e}")))?;
        Ok(body)
    })();
    let _ = fs::remove_file(tmp);
    result
}

/// Derive the output path from the URL when `--output` is omitted:
///
/// - last non-empty path segment with an extension is kept, percent-decoded
///   (`.../legacy.cpp` → `legacy.cpp`, `.../my%20file.cpp` → `my file.cpp`,
///   `…/%E6%BA%90%E7%A0%81.cpp` → `源码.cpp`);
/// - a segment without an extension gets `.utf8` appended (`.../foo` → `foo.utf8`);
/// - no usable segment → `download.utf8`.
///
/// A segment whose decoding would introduce a path separator or NUL (e.g.
/// `%2F`), or that is not valid UTF-8 when decoded, keeps its percent-encoded
/// spelling — the encoded form is always a single safe path component.
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
        Some(seg) => {
            let name = usable_filename(&seg);
            if has_extension(&name) {
                PathBuf::from(name)
            } else {
                PathBuf::from(format!("{name}.utf8"))
            }
        }
        None => PathBuf::from("download.utf8"),
    }
}

/// Percent-decode a URL path segment into a usable filename. Falls back to
/// the raw (encoded) spelling whenever decoding would produce an empty
/// string, a path separator, a backslash, or NUL — the encoded form is
/// always a single path component, so it is the safe fallback.
fn usable_filename(raw: &str) -> String {
    let decoded = percent_decode(raw);
    match decoded {
        Some(name)
            if !name.is_empty()
                && !name.contains('/')
                && !name.contains('\\')
                && !name.contains('\0') =>
        {
            name
        }
        _ => raw.to_string(),
    }
}

/// Decode `%XX` escapes into a String. Malformed escapes (a `%` not followed
/// by two hex digits) are kept literally; a byte sequence that is not valid
/// UTF-8 yields `None` (the caller then keeps the encoded spelling).
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex_high = (bytes[index + 1] as char).to_digit(16);
            let hex_low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex_high, hex_low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).ok()
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
    use std::io;

    #[test]
    fn streaming_temp_is_removed_after_exact_limit_readback() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        let body = vec![b'x'; 8192 + 13];
        assert_eq!(
            read_body_to_temp(
                &mut body.as_slice(),
                body.len() as u64,
                DOWNLOAD_BUDGET,
                &tmp
            )
            .unwrap(),
            body
        );
        assert!(!tmp.exists());
    }

    #[test]
    fn streaming_temp_is_removed_after_size_limit_failure() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        // The first chunk is persisted before the second exceeds the cap.
        let body = vec![b'x'; 8193];
        assert!(matches!(
            read_body_to_temp(&mut body.as_slice(), 8192, DOWNLOAD_BUDGET, &tmp),
            Err(ForgeError::TooLarge {
                size: 8193,
                max: 8192
            })
        ));
        assert!(!tmp.exists());
    }

    #[test]
    fn streaming_temp_accepts_empty_body_with_zero_limit() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        assert!(
            read_body_to_temp(&mut b"".as_slice(), 0, DOWNLOAD_BUDGET, &tmp)
                .unwrap()
                .is_empty()
        );
        assert!(!tmp.exists());
        assert!(matches!(
            read_body_to_temp(&mut b"x".as_slice(), 0, DOWNLOAD_BUDGET, &tmp),
            Err(ForgeError::TooLarge { size: 1, max: 0 })
        ));
        assert!(!tmp.exists());
    }

    #[test]
    fn streaming_temp_is_removed_after_body_read_failure() {
        struct FailingReader;

        impl Read for FailingReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected body read failure"))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        let mut reader = b"partial body".as_slice().chain(FailingReader);
        let error = read_body_to_temp(&mut reader, 1024, DOWNLOAD_BUDGET, &tmp).unwrap_err();
        assert!(
            matches!(error, ForgeError::Fetch(ref message) if message.starts_with("read body failed:"))
        );
        assert!(!tmp.exists());
    }

    #[test]
    fn streaming_temp_collision_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        fs::write(&tmp, b"original").unwrap();
        let mut reader = b"download".as_slice();
        let error = read_body_to_temp(&mut reader, 1024, DOWNLOAD_BUDGET, &tmp).unwrap_err();
        assert!(
            matches!(error, ForgeError::Fetch(ref message) if message.starts_with("create temp file failed:"))
        );
        assert_eq!(fs::read(&tmp).unwrap(), b"original");
        assert_eq!(
            reader, b"download",
            "the body must not be read after creation fails"
        );
    }

    #[cfg(unix)]
    #[test]
    fn streaming_temp_collision_preserves_symlink_and_target() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        let target = dir.path().join("original.cpp");
        fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, &tmp).unwrap();
        assert!(
            read_body_to_temp(&mut b"download".as_slice(), 1024, DOWNLOAD_BUDGET, &tmp).is_err()
        );
        assert!(fs::symlink_metadata(&tmp).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

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

    #[test]
    fn derive_percent_decodes_the_segment() {
        assert_eq!(
            derive_output_path("https://example.com/my%20file.cpp"),
            PathBuf::from("my file.cpp")
        );
        assert_eq!(
            derive_output_path("https://example.com/%E6%BA%90%E7%A0%81.cpp"),
            PathBuf::from("源码.cpp")
        );
        // Malformed escapes are kept literally, never dropped.
        assert_eq!(
            derive_output_path("https://example.com/%zz.cpp"),
            PathBuf::from("%zz.cpp")
        );
    }

    #[test]
    fn derive_keeps_encoding_when_decoding_would_be_unsafe() {
        // %2F would decode into a path separator; the encoded spelling must
        // stay one component instead of splitting the path.
        assert_eq!(
            derive_output_path("https://example.com/a%2Fb.cpp"),
            PathBuf::from("a%2Fb.cpp")
        );
        // Bytes that are not valid UTF-8 when decoded keep the encoding too.
        assert_eq!(
            derive_output_path("https://example.com/%FF.cpp"),
            PathBuf::from("%FF.cpp")
        );
    }

    #[test]
    fn wall_clock_budget_aborts_a_slow_reader() {
        struct SlowReader;

        impl Read for SlowReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_millis(30));
                buf[0] = b'x';
                Ok(1)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("fetch.tmp");
        let mut reader = SlowReader;
        let error =
            read_body_to_temp(&mut reader, u64::MAX, Duration::from_millis(100), &tmp).unwrap_err();
        assert!(
            matches!(error, ForgeError::Fetch(ref message) if message.contains("budget")),
            "{error}"
        );
        assert!(
            !tmp.exists(),
            "temp must be cleaned up after the budget trip"
        );
    }
}
