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

//! Strict decode / encode / transcode.
//!
//! ```rust
//! use utf8_forge::transcoder::{decode_strict, encode, TargetEncoding};
//!
//! let text = decode_strict(&[0xFF, 0xFE, b'h', 0], utf8_forge::detector::detect(&[0xFF, 0xFE, b'h', 0])).unwrap();
//! assert_eq!(&*text, "h");
//! assert_eq!(encode(&text, TargetEncoding::Utf8).unwrap(), b"h");
//! ```
//!
//! Rules enforced here:
//! - UTF-16 encoders never emit a BOM by themselves — [`encode`] prepends it
//!   manually.
//! - Any decode that reports `had_errors` is a hard error, never a
//!   replacement-char pass-through, so mojibake is never written.
//! - No newline normalization — `encoding_rs` preserves `\r\n` verbatim.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use encoding_rs::{GB18030, UTF_16BE, UTF_16LE};

use crate::detector::{DetectedEncoding, detect};
use crate::error::ForgeError;

/// Encodings the tool can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetEncoding {
    /// UTF-8 without BOM (the default target).
    Utf8,
    /// UTF-8 with `EF BB BF` prepended.
    Utf8Bom,
    /// GB18030 (superset of GB2312 / GBK).
    Gb18030,
    /// UTF-16 little endian with `FF FE` prepended.
    Utf16Le,
    /// UTF-16 big endian with `FE FF` prepended.
    Utf16Be,
}

impl TargetEncoding {
    /// Parse a CLI value. Case-insensitive. Only canonical tags are accepted.
    pub fn parse(s: &str) -> Option<TargetEncoding> {
        match s.to_ascii_lowercase().as_str() {
            "utf-8" => Some(TargetEncoding::Utf8),
            "utf-8-bom" => Some(TargetEncoding::Utf8Bom),
            "gb18030" => Some(TargetEncoding::Gb18030),
            "utf-16le" => Some(TargetEncoding::Utf16Le),
            "utf-16be" => Some(TargetEncoding::Utf16Be),
            _ => None,
        }
    }

    /// Stable snake-case tag used in reports and console output.
    pub fn tag(self) -> &'static str {
        match self {
            TargetEncoding::Utf8 => "utf-8",
            TargetEncoding::Utf8Bom => "utf-8-bom",
            TargetEncoding::Gb18030 => "gb18030",
            TargetEncoding::Utf16Le => "utf-16le",
            TargetEncoding::Utf16Be => "utf-16be",
        }
    }
}

impl fmt::Display for TargetEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

impl FromStr for TargetEncoding {
    type Err = String;

    /// Enables clap `value_parser` semantics; parse failure → exit code 2.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TargetEncoding::parse(s).ok_or_else(|| {
            format!(
                "invalid target encoding `{s}` (expected utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be)"
            )
        })
    }
}

/// Result of transcoding a byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscodeOutcome {
    /// Encoded output is byte-identical to the input (or the fast path hit) —
    /// the caller must skip the write to avoid needless mtime churn.
    Unchanged,
    /// New bytes to persist.
    Changed(Vec<u8>),
}

/// Strictly decode `bytes` according to a prior [`detect`] verdict.
///
/// Only the real leading BOM is stripped (via `decode_without_bom_handling` for
/// UTF-16 — `Encoding::decode` would BOM-sniff again and silently eat a
/// content U+FEFF in double-BOM files). Any `had_errors` from `encoding_rs`
/// aborts with [`ForgeError::DecodeError`] — mojibake is never written.
/// Truncated BOM payloads are a hard error, never a panic, so the pub API is
/// safe even when callers pass a verdict that does not match `bytes`.
pub fn decode_strict(bytes: &[u8], enc: DetectedEncoding) -> Result<Cow<'_, str>, ForgeError> {
    match enc {
        DetectedEncoding::Utf8 => std::str::from_utf8(bytes)
            .map(Cow::Borrowed)
            .map_err(|e| ForgeError::DecodeError(format!("invalid utf-8: {e}"))),
        DetectedEncoding::Utf8Bom => {
            let rest = bytes
                .get(3..)
                .ok_or_else(|| ForgeError::DecodeError("truncated utf-8 bom".into()))?;
            std::str::from_utf8(rest)
                .map(Cow::Borrowed)
                .map_err(|e| ForgeError::DecodeError(format!("invalid utf-8 after BOM: {e}")))
        }
        DetectedEncoding::Utf16Le => {
            let rest = bytes
                .get(2..)
                .ok_or_else(|| ForgeError::DecodeError("truncated utf-16le bom".into()))?;
            let (cow, had_errors) = UTF_16LE.decode_without_bom_handling(rest);
            if had_errors {
                return Err(ForgeError::DecodeError("invalid utf-16le sequence".into()));
            }
            Ok(cow)
        }
        DetectedEncoding::Utf16Be => {
            let rest = bytes
                .get(2..)
                .ok_or_else(|| ForgeError::DecodeError("truncated utf-16be bom".into()))?;
            let (cow, had_errors) = UTF_16BE.decode_without_bom_handling(rest);
            if had_errors {
                return Err(ForgeError::DecodeError("invalid utf-16be sequence".into()));
            }
            Ok(cow)
        }
        DetectedEncoding::Utf32Le => decode_utf32(
            bytes
                .get(4..)
                .ok_or_else(|| ForgeError::DecodeError("truncated utf-32le bom".into()))?,
            true,
        ),
        DetectedEncoding::Utf32Be => decode_utf32(
            bytes
                .get(4..)
                .ok_or_else(|| ForgeError::DecodeError("truncated utf-32be bom".into()))?,
            false,
        ),
        DetectedEncoding::Gb18030 => {
            // Skip encoding_rs' BOM sniffing: the caller's verdict came from
            // `detect`, which routes every BOM prefix to its own encoding, so
            // a UTF-16 BOM here must be a decode error, not a re-routed read.
            let (cow, had_errors) = GB18030.decode_without_bom_handling(bytes);
            if had_errors {
                return Err(ForgeError::DecodeError("invalid gb18030 sequence".into()));
            }
            Ok(cow)
        }
        DetectedEncoding::Unknown => Err(ForgeError::UnknownEncoding),
    }
}

/// Decode UTF-32 units (after the BOM). A misaligned tail, a surrogate
/// (`0xD800..=0xDFFF`) or a value above `0x10FFFF` is a hard decode error —
/// `char::from_u32` rejects surrogates and out-of-range values in one check.
fn decode_utf32(rest: &[u8], little_endian: bool) -> Result<Cow<'_, str>, ForgeError> {
    if !rest.len().is_multiple_of(4) {
        let enc = if little_endian {
            "utf-32le"
        } else {
            "utf-32be"
        };
        return Err(ForgeError::DecodeError(format!("misaligned {enc} payload")));
    }
    let mut out = String::with_capacity(rest.len() / 4);
    for unit in rest.as_chunks::<4>().0 {
        let v = if little_endian {
            u32::from_le_bytes(*unit)
        } else {
            u32::from_be_bytes(*unit)
        };
        let c = char::from_u32(v).ok_or_else(|| {
            ForgeError::DecodeError(format!("invalid utf-32 code point U+{v:04X}"))
        })?;
        out.push(c);
    }
    Ok(Cow::Owned(out))
}

/// Encode `text` into `target`. BOMs for `utf-8-bom` / `utf-16le` /
/// `utf-16be` are prepended manually — `encoding_rs` never emits them.
///
/// Fails instead of writing mojibake: a UTF-16LE target whose content begins
/// with U+0000 is refused (the output would begin `FF FE 00 00`, which the
/// detector reads back as the UTF-32LE BOM), and GB18030 refuses content
/// containing U+E5E5 — the single Unicode scalar the WHATWG gb18030 encoder
/// cannot map (encoding_rs would silently write the literal text `&#58853;`).
pub fn encode(text: &str, target: TargetEncoding) -> Result<Vec<u8>, ForgeError> {
    match target {
        TargetEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        TargetEncoding::Utf8Bom => {
            let mut v = Vec::with_capacity(3 + text.len());
            v.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            v.extend_from_slice(text.as_bytes());
            Ok(v)
        }
        TargetEncoding::Gb18030 => {
            let (cow, _, had_errors) = GB18030.encode(text);
            if had_errors {
                return Err(ForgeError::EncodeError(
                    "content contains characters unmappable in gb18030 (U+E5E5)".into(),
                ));
            }
            Ok(cow.into_owned())
        }
        TargetEncoding::Utf16Le => {
            if text.starts_with('\u{0}') {
                return Err(ForgeError::EncodeError(
                    "utf-16le output would begin with `FF FE 00 00` (indistinguishable from the \
                     utf-32le BOM) because the content begins with U+0000; the file could not be \
                     re-read as UTF-16 on the next run"
                        .into(),
                ));
            }
            // encoding_rs' generic `encode()` would silently emit UTF-8 here
            // (its `output_encoding()` maps UTF-16 targets back to UTF-8), so
            // UTF-16 bytes are produced directly from std's `encode_utf16()`,
            // which cannot yield lone surrogates for a valid `&str`.
            let mut v = Vec::with_capacity(2 + text.len() * 2);
            v.extend_from_slice(&[0xFF, 0xFE]);
            for unit in text.encode_utf16() {
                v.extend_from_slice(&unit.to_le_bytes());
            }
            Ok(v)
        }
        TargetEncoding::Utf16Be => {
            let mut v = Vec::with_capacity(2 + text.len() * 2);
            v.extend_from_slice(&[0xFE, 0xFF]);
            for unit in text.encode_utf16() {
                v.extend_from_slice(&unit.to_be_bytes());
            }
            Ok(v)
        }
    }
}

/// Detect + transcode in one call. Prefer [`transcode_detected`] when the
/// detection verdict is already known (avoids a second sniff).
pub fn transcode(bytes: &[u8], target: TargetEncoding) -> Result<TranscodeOutcome, ForgeError> {
    transcode_detected(bytes, detect(bytes), target)
}

/// Transcode with a pre-computed detection verdict.
///
/// Fast path: `Utf8` → `utf-8` returns [`TranscodeOutcome::Unchanged`] with
/// zero allocation. Otherwise strict-decode, encode, and compare: a
/// byte-identical result is reported as `Unchanged` so callers skip the write
/// and a second pass over the same tree touches no file. Encoding refusals
/// ([`ForgeError::EncodeError`] — the UTF-16LE NUL guard, the GB18030
/// unmappable scalar, and the GB18030 self-misread guard below) never write
/// mojibake.
pub fn transcode_detected(
    bytes: &[u8],
    detected: DetectedEncoding,
    target: TargetEncoding,
) -> Result<TranscodeOutcome, ForgeError> {
    if detected == DetectedEncoding::Utf8 && target == TargetEncoding::Utf8 {
        return Ok(TranscodeOutcome::Unchanged);
    }
    let text = decode_strict(bytes, detected)?;
    let encoded = encode(&text, target)?;
    if encoded == bytes {
        return Ok(TranscodeOutcome::Unchanged);
    }
    // A gb18030 output that the next run would re-detect as UTF-8 is only
    // safe when reading it back as UTF-8 and re-encoding yields the same
    // bytes (then the next run sees `Unchanged`). When it does not, the next
    // run silently rewrites different characters into the file — 1920 hanzi
    // encode this way, e.g. U+4E00 一 → D2 BB, which reads back as U+04BB һ
    // and re-encodes to a 4-byte sequence. Refuse to create that drift,
    // mirroring the UTF-16LE NUL guard; outputs that cannot be valid UTF-8
    // (ASCII aside) are unambiguous and convert freely, and a file already
    // holding unambiguous gb18030 bytes re-encodes byte-identically through
    // the `Unchanged` path above.
    if target == TargetEncoding::Gb18030
        && let Some(retext) = std::str::from_utf8(&encoded).ok()
        && encode(retext, TargetEncoding::Gb18030).is_ok_and(|re_encoded| re_encoded != encoded)
    {
        return Err(ForgeError::EncodeError(
            "gb18030 output would itself be read back as different characters on the next \
             run (many common hanzi encode that way, e.g. U+4E00 → D2 BB, which re-reads as \
             U+04BB һ); refusing to write output the tool itself would silently rewrite"
                .into(),
        ));
    }
    Ok(TranscodeOutcome::Changed(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_canonical_tags_case_insensitive() {
        assert_eq!(TargetEncoding::parse("utf-8"), Some(TargetEncoding::Utf8));
        assert_eq!(
            TargetEncoding::parse("UTF-16LE"),
            Some(TargetEncoding::Utf16Le)
        );
        assert_eq!(
            TargetEncoding::parse("gb18030"),
            Some(TargetEncoding::Gb18030)
        );
        assert_eq!(
            TargetEncoding::parse("utf-8-bom"),
            Some(TargetEncoding::Utf8Bom)
        );
        assert_eq!(
            TargetEncoding::parse("utf-16be"),
            Some(TargetEncoding::Utf16Be)
        );
        assert_eq!(TargetEncoding::parse("gbk"), None);
        assert_eq!(TargetEncoding::parse(""), None);
    }

    #[test]
    fn utf16_targets_manually_prepend_bom() {
        // "hi" encoded to UTF-16LE must start with FF FE — encoding_rs alone
        // would not emit it.
        assert_eq!(
            encode("hi", TargetEncoding::Utf16Le).unwrap(),
            vec![0xFF, 0xFE, b'h', 0, b'i', 0]
        );
        assert_eq!(
            encode("hi", TargetEncoding::Utf16Be).unwrap(),
            vec![0xFE, 0xFF, 0, b'h', 0, b'i']
        );
        assert_eq!(encode("hi", TargetEncoding::Utf8Bom).unwrap(), {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(b"hi");
            v
        });
    }

    #[test]
    fn fast_path_utf8_to_utf8_is_unchanged() {
        assert_eq!(
            transcode(b"hello", TargetEncoding::Utf8).unwrap(),
            TranscodeOutcome::Unchanged
        );
    }

    #[test]
    fn empty_bom_only_file_converges_idempotently() {
        let bom = [0xEF, 0xBB, 0xBF];
        let out = match transcode(&bom, TargetEncoding::Utf8).unwrap() {
            TranscodeOutcome::Changed(v) => v,
            TranscodeOutcome::Unchanged => panic!("BOM-only file must change"),
        };
        assert!(out.is_empty());
        // Second pass: empty file detects as Utf8 → fast path.
        assert_eq!(
            transcode(&out, TargetEncoding::Utf8).unwrap(),
            TranscodeOutcome::Unchanged
        );
    }

    #[test]
    fn byte_identical_output_skips_write() {
        // ASCII is identical in GB18030 — must not be reported as Changed.
        assert_eq!(
            transcode(b"plain ascii", TargetEncoding::Gb18030).unwrap(),
            TranscodeOutcome::Unchanged
        );
    }

    #[test]
    fn gb18030_output_that_re_reads_as_utf8_is_refused() {
        // 一 encodes to D2 BB, which is valid UTF-8 (U+04BB); writing it
        // would make the next run misread the file as UTF-8 and rewrite the
        // character, so the transcode is refused instead.
        let err = transcode("一\n".as_bytes(), TargetEncoding::Gb18030).unwrap_err();
        assert!(matches!(err, ForgeError::EncodeError(_)), "{err}");
    }

    #[test]
    fn gb18030_output_that_cannot_be_utf8_still_converts() {
        // 命 encodes to C3 FC, which is not valid UTF-8, so the output is
        // unambiguous and must convert normally.
        let out = match transcode("命\n".as_bytes(), TargetEncoding::Gb18030).unwrap() {
            TranscodeOutcome::Changed(v) => v,
            other => panic!("must change, got {other:?}"),
        };
        assert_eq!(out, b"\xC3\xFC\x0A");
    }

    #[test]
    fn existing_4_byte_gb18030_stays_unchanged() {
        // һ (U+04BB) already sits in its 4-byte gb18030 spelling; the
        // re-encode is byte-identical and must not trip the ambiguity
        // refusal (which only guards *changed* output).
        let bytes = [0x81, 0x30, 0xDD, 0x35, b'\n'];
        assert_eq!(
            transcode(&bytes, TargetEncoding::Gb18030).unwrap(),
            TranscodeOutcome::Unchanged
        );
    }

    #[test]
    fn gb18030_ambiguity_policy_matches_full_scalar_oracle() {
        // For every scalar: byte-identical output stays Unchanged; output
        // the next run would read back identically converts; output the next
        // run would silently rewrite must be refused; everything else
        // converts. U+E5E5 is the single unmappable scalar, and U+FEFF's
        // UTF-8 spelling is the BOM, so its input detection differs.
        for cp in 0x20u32..0x11_0000 {
            if (0xD800..=0xDFFF).contains(&cp) {
                continue; // Surrogates are not scalars.
            }
            let text = format!("{}\n", char::from_u32(cp).unwrap());
            let Ok(encoded) = encode(&text, TargetEncoding::Gb18030) else {
                assert_eq!(cp, 0xE5E5, "only U+E5E5 is unmappable");
                continue;
            };
            // The next-run simulation: read the output back as UTF-8 (as
            // the detector would) and re-encode.
            let reread_stable = std::str::from_utf8(&encoded).is_ok_and(|retext| {
                encode(retext, TargetEncoding::Gb18030).is_ok_and(|re| re == encoded)
            });
            let outcome = transcode(text.as_bytes(), TargetEncoding::Gb18030);
            if encoded == text.as_bytes() {
                assert_eq!(outcome.unwrap(), TranscodeOutcome::Unchanged, "U+{cp:04X}");
            } else if reread_stable {
                assert!(
                    matches!(outcome, Ok(TranscodeOutcome::Changed(_))),
                    "U+{cp:04X} must convert (stable reread)"
                );
            } else if std::str::from_utf8(&encoded).is_ok() {
                assert!(
                    matches!(outcome, Err(ForgeError::EncodeError(_))),
                    "U+{cp:04X} must be refused"
                );
            } else {
                assert!(
                    matches!(outcome, Ok(TranscodeOutcome::Changed(_))),
                    "U+{cp:04X} must convert"
                );
            }
        }
    }
}
