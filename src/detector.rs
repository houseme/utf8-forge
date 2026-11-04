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

//! Encoding detection: a frozen rule-based algorithm, deliberately without
//! statistical guessing so results are auditable and reproducible.
//!
//! Detection order is fixed: BOM sniffing first, then strict UTF-8, then
//! strict GB18030. Reordering the UTF-8/GB18030 checks would reclassify
//! ASCII-only files and break idempotence.

use encoding_rs::GB18030;

/// Encodings this tool can classify a byte stream as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectedEncoding {
    /// No BOM + strict UTF-8 valid. Pure ASCII also lands here (fast path).
    Utf8,
    /// Starts with `EF BB BF`.
    Utf8Bom,
    /// Starts with `FF FE`.
    Utf16Le,
    /// Starts with `FE FF`.
    Utf16Be,
    /// Starts with `FF FE 00 00` (UTF-32LE BOM; the 4-byte prefix wins over
    /// the 2-byte UTF-16LE check — WHATWG-style longest-BOM precedence).
    Utf32Le,
    /// Starts with `00 00 FE FF` (UTF-32BE BOM).
    Utf32Be,
    /// UTF-8 strict decode failed, GB18030 strict decode succeeded.
    /// Covers GB2312 / GBK / GB18030.
    Gb18030,
    /// All detectors failed. Reported & skipped — never guessed.
    Unknown,
}

impl DetectedEncoding {
    /// Stable snake-case tag used in JSON reports and console output.
    pub fn tag(self) -> &'static str {
        match self {
            DetectedEncoding::Utf8 => "utf-8",
            DetectedEncoding::Utf8Bom => "utf-8-bom",
            DetectedEncoding::Utf16Le => "utf-16le",
            DetectedEncoding::Utf16Be => "utf-16be",
            DetectedEncoding::Utf32Le => "utf-32le",
            DetectedEncoding::Utf32Be => "utf-32be",
            DetectedEncoding::Gb18030 => "gb18030",
            DetectedEncoding::Unknown => "unknown",
        }
    }

    /// True when the encoding was identified purely by a byte-order mark.
    pub fn has_bom(self) -> bool {
        matches!(
            self,
            DetectedEncoding::Utf8Bom
                | DetectedEncoding::Utf16Le
                | DetectedEncoding::Utf16Be
                | DetectedEncoding::Utf32Le
                | DetectedEncoding::Utf32Be
        )
    }
}

/// Classify raw bytes. Rule-based on purpose — no statistical guessing.
///
/// The UTF-32 BOMs (`FF FE 00 00` LE / `00 00 FE FF` BE) are checked before
/// the 2-byte UTF-16 BOMs (longest-prefix precedence, as in WHATWG BOM
/// sniffing). Documented consequence: a UTF-16LE file whose first character
/// is U+0000 shares its leading bytes with the UTF-32LE BOM; such a payload
/// fails the strict UTF-32 decode loudly (`decode_error`) instead of being
/// silently transcoded, so a detection ambiguity never produces mojibake.
pub fn detect(bytes: &[u8]) -> DetectedEncoding {
    if bytes.is_empty() {
        return DetectedEncoding::Utf8;
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return DetectedEncoding::Utf8Bom;
    }
    // WHATWG BOM sniffing: the 4-byte UTF-32 patterns are checked before the
    // 2-byte UTF-16 patterns. The UTF-32BE BOM is `00 00 FE FF`; a payload
    // starting `FE FF 00 00` is a UTF-16BE file whose first character is
    // U+0000 — UTF-16BE, not UTF-32BE.
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return DetectedEncoding::Utf32Be;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
            return DetectedEncoding::Utf32Le;
        }
        return DetectedEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return DetectedEncoding::Utf16Be;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return DetectedEncoding::Utf8;
    }
    let (_, _, had_errors) = GB18030.decode(bytes);
    if !had_errors {
        return DetectedEncoding::Gb18030;
    }
    DetectedEncoding::Unknown
}
