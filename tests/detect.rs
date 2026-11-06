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

//! Detection tests covering every encoding the tool classifies.

use utf8_forge::detector::{DetectedEncoding, detect};

#[test]
fn ascii_is_utf8() {
    assert_eq!(detect(b"hello"), DetectedEncoding::Utf8);
}

#[test]
fn utf8_bom_is_detected() {
    assert_eq!(detect(b"\xEF\xBB\xBFhello"), DetectedEncoding::Utf8Bom);
}

#[test]
fn utf16le_bom_is_detected() {
    assert_eq!(detect(b"\xFF\xFEh\0i\0"), DetectedEncoding::Utf16Le);
}

#[test]
fn utf16be_bom_is_detected() {
    assert_eq!(detect(b"\xFE\xFF\0h\0i"), DetectedEncoding::Utf16Be);
}

#[test]
fn gbk_bytes_are_gb18030() {
    // "你好" in GBK / GB18030.
    assert_eq!(detect(&[0xC4, 0xE3, 0xBA, 0xC3]), DetectedEncoding::Gb18030);
}

#[test]
fn invalid_bytes_are_unknown() {
    // A byte sequence no rule can classify.
    assert_eq!(detect(b"\xFF\xFF\xFF"), DetectedEncoding::Unknown);
}

#[test]
fn empty_input_is_utf8() {
    assert_eq!(detect(&[]), DetectedEncoding::Utf8);
}

#[test]
fn utf8_multibyte_is_utf8_not_gb18030() {
    // "你好" in UTF-8. Detection order is frozen: strict UTF-8 wins over
    // GB18030 even though some UTF-8 sequences may re-decode as GB18030.
    let utf8 = "你好".as_bytes();
    assert_eq!(detect(utf8), DetectedEncoding::Utf8);
}

#[test]
fn utf8_bom_only_file() {
    assert_eq!(detect(&[0xEF, 0xBB, 0xBF]), DetectedEncoding::Utf8Bom);
}

#[test]
fn lone_utf16_boms() {
    // Exactly a BOM and nothing else still counts as the BOM encoding.
    assert_eq!(detect(&[0xFF, 0xFE]), DetectedEncoding::Utf16Le);
    assert_eq!(detect(&[0xFE, 0xFF]), DetectedEncoding::Utf16Be);
}

#[test]
fn tags_are_stable() {
    assert_eq!(DetectedEncoding::Utf8.tag(), "utf-8");
    assert_eq!(DetectedEncoding::Utf8Bom.tag(), "utf-8-bom");
    assert_eq!(DetectedEncoding::Utf16Le.tag(), "utf-16le");
    assert_eq!(DetectedEncoding::Utf16Be.tag(), "utf-16be");
    assert_eq!(DetectedEncoding::Utf32Le.tag(), "utf-32le");
    assert_eq!(DetectedEncoding::Utf32Be.tag(), "utf-32be");
    assert_eq!(DetectedEncoding::Gb18030.tag(), "gb18030");
    assert_eq!(DetectedEncoding::Unknown.tag(), "unknown");
}

#[test]
fn bom_flag_tracks_bom_based_detections() {
    assert!(!DetectedEncoding::Utf8.has_bom());
    assert!(!DetectedEncoding::Gb18030.has_bom());
    assert!(!DetectedEncoding::Unknown.has_bom());
    assert!(DetectedEncoding::Utf8Bom.has_bom());
    assert!(DetectedEncoding::Utf16Le.has_bom());
    assert!(DetectedEncoding::Utf16Be.has_bom());
    assert!(DetectedEncoding::Utf32Le.has_bom());
    assert!(DetectedEncoding::Utf32Be.has_bom());
}

#[test]
fn utf32_boms_are_detected_with_longest_prefix_precedence() {
    // The UTF-32LE BOM (FF FE 00 00) wins over the 2-byte UTF-16LE check;
    // the UTF-32BE BOM is 00 00 FE FF (U+FEFF encoded big-endian).
    assert_eq!(
        detect(&[0xFF, 0xFE, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00]),
        DetectedEncoding::Utf32Le
    );
    assert_eq!(
        detect(&[0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, 0x41]),
        DetectedEncoding::Utf32Be
    );
}

#[test]
fn fe_ff_00_00_is_utf16be_with_leading_nul_not_utf32be() {
    // WHATWG: FE FF sniffs as UTF-16BE; there is no UTF-32BE BOM that starts
    // with FE FF. A UTF-16BE file whose first character is U+0000 decodes
    // strictly as UTF-16BE.
    assert_eq!(
        detect(&[0xFE, 0xFF, 0x00, 0x00, 0x00, 0x41, 0x00, 0x42]),
        DetectedEncoding::Utf16Be
    );
}

#[test]
fn utf16_files_with_shorter_prefixes_stay_utf16() {
    // A 2- or 3-byte FF FE prefix is still UTF-16LE (length-gated).
    assert_eq!(detect(&[0xFF, 0xFE]), DetectedEncoding::Utf16Le);
    assert_eq!(detect(&[0xFF, 0xFE, 0x41, 0x00]), DetectedEncoding::Utf16Le);
}
