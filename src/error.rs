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

//! Error types shared across the transcoding pipeline.
//!
//! `ForgeError` is the per-file domain error. Each variant maps 1:1 to a
//! stable report category, which keeps report aggregation deterministic.
//! Fatal / setup errors bubble up as `anyhow::Error` instead.

use std::fmt;
use std::io;

/// Domain error for a single file operation.
#[derive(Debug)]
pub enum ForgeError {
    /// Detection could not classify the bytes — never guessed, always reported.
    UnknownEncoding,
    /// A legal encoding was detected but the byte stream is malformed
    /// (`had_errors == true` from `encoding_rs`). Never half-decoded.
    DecodeError(String),
    /// The decoded text cannot be encoded into the requested target without
    /// producing bytes the tool could not re-read on the next run.
    EncodeError(String),
    /// File (or remote payload) exceeds `--max-file-size`.
    TooLarge { size: u64, max: u64 },
    /// Local filesystem failure (permissions, locked file, ...).
    Io(io::Error),
    /// `--url` network / TLS / status failure.
    Fetch(String),
}

impl ForgeError {
    /// Stable report category string for this error (used by the JSON report
    /// summary buckets and `error_files` lists).
    pub fn category(&self) -> &'static str {
        match self {
            ForgeError::UnknownEncoding => "unknown",
            ForgeError::DecodeError(_) => "decode_error",
            ForgeError::EncodeError(_) => "encode_error",
            ForgeError::TooLarge { .. } => "too_large",
            ForgeError::Io(_) => "io_error",
            ForgeError::Fetch(_) => "fetch_error",
        }
    }
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::UnknownEncoding => write!(f, "unknown encoding (never guessed)"),
            ForgeError::DecodeError(detail) => write!(f, "strict decode failed: {detail}"),
            ForgeError::EncodeError(detail) => write!(f, "strict encode failed: {detail}"),
            ForgeError::TooLarge { size, max } => {
                write!(f, "file too large: {size} bytes > --max-file-size {max}")
            }
            ForgeError::Io(e) => write!(f, "io error: {e}"),
            ForgeError::Fetch(e) => write!(f, "fetch error: {e}"),
        }
    }
}

impl std::error::Error for ForgeError {
    // The io cause is already spelled out by `Display`; re-exposing it as
    // `source()` would make anyhow-style `{err:#}` chains print it twice.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl From<io::Error> for ForgeError {
    fn from(e: io::Error) -> Self {
        ForgeError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_match_report_schema() {
        assert_eq!(ForgeError::UnknownEncoding.category(), "unknown");
        assert_eq!(
            ForgeError::DecodeError("x".into()).category(),
            "decode_error"
        );
        assert_eq!(
            ForgeError::EncodeError("x".into()).category(),
            "encode_error"
        );
        assert_eq!(
            ForgeError::TooLarge { size: 10, max: 5 }.category(),
            "too_large"
        );
        assert_eq!(ForgeError::Io(io::Error::other("x")).category(), "io_error");
        assert_eq!(ForgeError::Fetch("x".into()).category(), "fetch_error");
    }
}
