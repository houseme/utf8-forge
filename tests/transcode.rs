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

//! Transcoding tests: round-trips, idempotence, BOM policy, strict decode.

use utf8_forge::detector::{DetectedEncoding, detect};
use utf8_forge::error::ForgeError;
use utf8_forge::transcoder::{TargetEncoding, TranscodeOutcome, transcode};

const GBK_NIHAO: &[u8] = &[0xC4, 0xE3, 0xBA, 0xC3]; // "你好" GB18030
const UTF8_NIHAO: &[u8] = "你好".as_bytes();

#[test]
fn transcode_utf8_to_utf8_is_unchanged() {
    assert_eq!(
        transcode(b"hello", TargetEncoding::Utf8).unwrap(),
        TranscodeOutcome::Unchanged
    );
}

#[test]
fn utf8bom_to_utf8_strips_bom_and_is_idempotent() {
    let bom = [0xEF, 0xBB, 0xBF, b'a', b'b'];
    let first = transcode(&bom, TargetEncoding::Utf8).unwrap();
    let bytes = match &first {
        TranscodeOutcome::Changed(v) => v.clone(),
        TranscodeOutcome::Unchanged => panic!("BOM must be stripped"),
    };
    assert_eq!(bytes, b"ab");

    // Second run: detected as plain UTF-8 → fast path unchanged, so a second
    // convert pass changes 0 bytes.
    assert_eq!(
        transcode(&bytes, TargetEncoding::Utf8).unwrap(),
        TranscodeOutcome::Unchanged
    );
}

#[test]
fn gbk_to_utf8_to_gbk_round_trip_is_byte_lossless() {
    // Round-tripping through UTF-8 must reproduce the exact input bytes.
    let to_utf8 = transcode(GBK_NIHAO, TargetEncoding::Utf8).unwrap();
    let utf8 = match &to_utf8 {
        TranscodeOutcome::Changed(v) => v.clone(),
        TranscodeOutcome::Unchanged => panic!("GBK must change to UTF-8"),
    };
    assert_eq!(utf8, UTF8_NIHAO);

    let back = transcode(&utf8, TargetEncoding::Gb18030).unwrap();
    let gbk = match &back {
        TranscodeOutcome::Changed(v) => v.clone(),
        TranscodeOutcome::Unchanged => panic!("UTF-8 Chinese must change to GB18030"),
    };
    assert_eq!(gbk, GBK_NIHAO);
}

#[test]
fn utf16le_round_trip_with_manual_bom() {
    let le = [0xFF, 0xFE, b'h', 0, b'i', 0];
    let utf8 = match transcode(&le, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-16 must change"),
    };
    assert_eq!(utf8, b"hi");

    let back = match transcode(&utf8, TargetEncoding::Utf16Le).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("BOM must be manually prepended"),
    };
    assert_eq!(back, le);
}

#[test]
fn utf16be_round_trip_with_manual_bom() {
    let be = [0xFE, 0xFF, 0, b'h', 0, b'i'];
    let utf8 = match transcode(&be, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-16 must change"),
    };
    assert_eq!(utf8, b"hi");

    let back = match transcode(&utf8, TargetEncoding::Utf16Be).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("BOM must be manually prepended"),
    };
    assert_eq!(back, be);
}

#[test]
fn crlf_survives_utf16_round_trip() {
    // Line endings are preserved verbatim (no newline normalization).
    let text = b"a\r\nb\r\n";
    let le = match transcode(text, TargetEncoding::Utf16Le).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("expected change"),
    };
    let back = match transcode(&le, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("expected change"),
    };
    assert_eq!(back, text);
}

#[test]
fn invalid_utf16_surrogate_pair_is_hard_error() {
    // Lone high surrogate D800 encoded twice — illegal UTF-16.
    let bad = [0xFF, 0xFE, 0x00, 0xD8, 0x00, 0xD8];
    match transcode(&bad, TargetEncoding::Utf8) {
        Err(ForgeError::DecodeError(_)) => {}
        other => panic!("expected DecodeError, got {other:?}"),
    }
}

#[test]
fn unknown_input_is_never_guessed() {
    match transcode(b"\xFF\xFF\xFF", TargetEncoding::Utf8) {
        Err(ForgeError::UnknownEncoding) => {}
        other => panic!("expected UnknownEncoding, got {other:?}"),
    }
}

#[test]
fn gbk_target_of_already_gbk_bytes_is_unchanged() {
    // Detect Gb18030 → decode → re-encode → identical bytes → Unchanged.
    assert_eq!(
        transcode(GBK_NIHAO, TargetEncoding::Gb18030).unwrap(),
        TranscodeOutcome::Unchanged
    );
}

#[test]
fn utf8_target_converges_every_valid_source() {
    // After one pass everything is plain UTF-8, so the second
    // pass must be a no-op for every target combination.
    let sources: Vec<Vec<u8>> = vec![
        b"plain ascii".to_vec(),
        UTF8_NIHAO.to_vec(),
        GBK_NIHAO.to_vec(),
        vec![0xEF, 0xBB, 0xBF, b'b', b'o', b'm'],
        vec![0xFF, 0xFE, 0x4F, 0x60, 0x7D, 0x59], // UTF-16LE "你好"
        vec![0xFE, 0xFF, 0x4F, 0x60, 0x7D, 0x59], // UTF-16BE "你好"
    ];
    for src in &sources {
        let out = match transcode(src, TargetEncoding::Utf8).unwrap() {
            TranscodeOutcome::Changed(v) => v,
            TranscodeOutcome::Unchanged => src.clone(),
        };
        assert_eq!(
            detect(&out),
            DetectedEncoding::Utf8,
            "must converge to UTF-8"
        );
        assert_eq!(
            transcode(&out, TargetEncoding::Utf8).unwrap(),
            TranscodeOutcome::Unchanged,
            "second pass must change 0 bytes"
        );
    }
}

#[test]
fn utf8_bom_target_is_idempotent_on_second_pass() {
    let out = match transcode(b"hello", TargetEncoding::Utf8Bom).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("BOM must be added"),
    };
    assert_eq!(&out[..3], &[0xEF, 0xBB, 0xBF]);
    // Decode strips BOM, encode re-adds it → byte-identical → Unchanged.
    assert_eq!(
        transcode(&out, TargetEncoding::Utf8Bom).unwrap(),
        TranscodeOutcome::Unchanged
    );
}

#[test]
fn gb18030_target_round_trip_via_cli_tags() {
    let out = match transcode(UTF8_NIHAO, TargetEncoding::Gb18030).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("expected change"),
    };
    assert_eq!(out, GBK_NIHAO);
    // Back to UTF-8 through the default target — the rollout loop's core.
    let utf8 = match transcode(&out, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("expected change"),
    };
    assert_eq!(utf8, UTF8_NIHAO);
}

#[test]
fn truncated_bom_payloads_error_never_panic() {
    // The pub API must not panic even when a verdict does not match the
    // payload length.
    let cases: Vec<(&[u8], DetectedEncoding)> = vec![
        (&[0xEF], DetectedEncoding::Utf8Bom),
        (&[0xEF, 0xBB], DetectedEncoding::Utf8Bom),
        (&[0xFF], DetectedEncoding::Utf16Le),
        (&[0xFE], DetectedEncoding::Utf16Be),
    ];
    for (bytes, enc) in cases {
        match utf8_forge::transcoder::decode_strict(bytes, enc) {
            Err(ForgeError::DecodeError(msg)) => assert!(msg.contains("truncated"), "{msg}"),
            other => panic!("expected DecodeError for {bytes:?}, got {other:?}"),
        }
    }
}

#[test]
fn double_bom_utf16_keeps_content_bom_char() {
    // BOM + content U+FEFF + "hi": only the real BOM is stripped; the second
    // BOM is content.
    let bytes = [0xFF, 0xFE, 0xFF, 0xFE, 0x68, 0x00, 0x69, 0x00];
    // Re-encoding the decoded text yields the input bytes exactly → the tool
    // correctly reports Unchanged (byte-identical → no write).
    match transcode(&bytes, TargetEncoding::Utf16Le).unwrap() {
        TranscodeOutcome::Unchanged => {}
        TranscodeOutcome::Changed(v) => panic!("BOM + U+FEFF must round-trip exactly, got {v:?}"),
    }
    // To UTF-8: BOM stripped, U+FEFF kept as EF BB BF content.
    let utf8 = match transcode(&bytes, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("expected change"),
    };
    assert_eq!(utf8, [0xEF, 0xBB, 0xBF, b'h', b'i']);
}

// UTF-32 read support: decode is strict, transcode converges in one pass.

#[test]
fn utf32le_payload_decodes_and_transcodes_to_utf8() {
    // "4h" in UTF-32LE.
    let bytes = [
        0xFF, 0xFE, 0x00, 0x00, 0x34, 0x00, 0x00, 0x00, 0x68, 0x00, 0x00, 0x00,
    ];
    let out = match transcode(&bytes, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-32 must change"),
    };
    assert_eq!(out, b"4h");
}

#[test]
fn utf32_chinese_round_trips_to_utf8_and_is_idempotent() {
    // "你好" as UTF-32LE units.
    let mut bytes = vec![0xFF, 0xFE, 0x00, 0x00];
    for unit in "你好".chars().map(u32::from) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let out = match transcode(&bytes, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-32 must change"),
    };
    assert_eq!(out, "你好".as_bytes());
    // Second pass sees strict UTF-8 → unchanged.
    assert_eq!(
        transcode(&out, TargetEncoding::Utf8).unwrap(),
        TranscodeOutcome::Unchanged
    );
}

#[test]
fn utf32_surrogate_and_oversized_units_are_hard_errors() {
    // U+D800 encoded as a UTF-32LE unit — illegal.
    let bad = [0xFF, 0xFE, 0x00, 0x00, 0x00, 0xD8, 0x00, 0x00];
    match transcode(&bad, TargetEncoding::Utf8) {
        Err(ForgeError::DecodeError(msg)) => assert!(msg.contains("U+D800"), "{msg}"),
        other => panic!("expected DecodeError, got {other:?}"),
    }
    // U+110000 — above the Unicode maximum.
    let big = [0xFF, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x11, 0x00];
    match transcode(&big, TargetEncoding::Utf8) {
        Err(ForgeError::DecodeError(_)) => {}
        other => panic!("expected DecodeError, got {other:?}"),
    }
}

#[test]
fn utf32_misaligned_tail_and_truncated_bom_are_hard_errors() {
    // 3-byte tail after the BOM.
    let misaligned = [0xFF, 0xFE, 0x00, 0x00, 0x11, 0x00, 0x00];
    match transcode(&misaligned, TargetEncoding::Utf8) {
        Err(ForgeError::DecodeError(msg)) => assert!(msg.contains("misaligned"), "{msg}"),
        other => panic!("expected DecodeError, got {other:?}"),
    }
    // Lone 2-byte FF FE prefix is UTF-16, but a lone 4-byte BOM is valid
    // UTF-32 with no content and converts to an empty file.
    let lone_bom = [0xFF, 0xFE, 0x00, 0x00];
    let out = match transcode(&lone_bom, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("lone UTF-32 BOM must change"),
    };
    assert!(out.is_empty());
}

#[test]
fn utf32be_payload_decodes() {
    // "A" as UTF-32BE (BOM 00 00 FE FF): 00 00 FE FF 00 00 00 41.
    let bytes = [0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, 0x41];
    let out = match transcode(&bytes, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-32 must change"),
    };
    assert_eq!(out, b"A");
}

#[test]
fn utf32be_chinese_round_trips() {
    // "你" as UTF-32BE with BOM.
    let mut bytes = vec![0x00u8, 0x00, 0xFE, 0xFF];
    for unit in "你".chars().map(u32::from) {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    let out = match transcode(&bytes, TargetEncoding::Utf8).unwrap() {
        TranscodeOutcome::Changed(v) => v,
        TranscodeOutcome::Unchanged => panic!("UTF-32 must change"),
    };
    assert_eq!(out, "你".as_bytes());
}

#[test]
fn utf32_is_never_a_write_target() {
    assert_eq!(TargetEncoding::parse("utf-32le"), None);
    assert_eq!(TargetEncoding::parse("utf-32be"), None);
}

#[test]
fn utf16le_target_refuses_nul_leading_content() {
    // The output would begin `FF FE 00 00`, which the next run reads back as
    // the UTF-32LE BOM — so the encoder refuses instead of writing a file it
    // could never re-read as UTF-16.
    let err = transcode(b"\x00B\n", TargetEncoding::Utf16Le).unwrap_err();
    assert_eq!(err.category(), "encode_error");
    // The big-endian BOM `FE FF 00 00` re-reads as UTF-16BE, so NUL-leading
    // content is fine there.
    match transcode(b"\x00B\n", TargetEncoding::Utf16Be).unwrap() {
        TranscodeOutcome::Changed(v) => {
            assert_eq!(&v[..4], &[0xFE, 0xFF, 0x00, 0x00]);
        }
        TranscodeOutcome::Unchanged => panic!("utf-8 content must change to utf-16be"),
    }
}

#[test]
fn utf16le_nul_leading_file_is_never_produced_by_second_pass() {
    // A UTF-16LE file whose first character is U+0000 detects as UTF-32LE on
    // the next run; re-targeting it to utf-16le must not silently rewrite the
    // mis-decoded text into different UTF-16 bytes.
    let bytes = [0xFF, 0xFE, 0x00, 0x00, 0x42, 0x00]; // reads as UTF-32LE BOM + payload
    let det = detect(&bytes);
    assert_eq!(det, DetectedEncoding::Utf32Le);
    // Whatever the decode verdict, an encode to utf-16le of NUL-leading text
    // is refused — the corrupting second pass from the old behavior is gone.
    let text = "\u{0}B";
    let err = transcode(text.as_bytes(), TargetEncoding::Utf16Le).unwrap_err();
    assert_eq!(err.category(), "encode_error");
}

#[test]
fn gb18030_four_byte_round_trip_is_lossless() {
    // U+20000 needs the 4-byte GB18030 sequence; astral content must survive
    // the full GB18030 → UTF-8 → GB18030 cycle.
    let u20000 = "\u{20000}";
    let to_gb = transcode(u20000.as_bytes(), TargetEncoding::Gb18030).unwrap();
    let gb_bytes = match &to_gb {
        TranscodeOutcome::Changed(v) => v.clone(),
        TranscodeOutcome::Unchanged => panic!("utf-8 astral text must change to gb18030"),
    };
    assert_eq!(gb_bytes.len(), 4, "U+20000 uses the 4-byte GB18030 form");
    assert_eq!(detect(&gb_bytes), DetectedEncoding::Gb18030);
    let back = transcode(&gb_bytes, TargetEncoding::Utf8).unwrap();
    match back {
        TranscodeOutcome::Changed(v) => assert_eq!(v, u20000.as_bytes().to_vec()),
        TranscodeOutcome::Unchanged => panic!("gb18030 must decode back to utf-8"),
    }
}

#[test]
fn gb18030_target_refuses_the_unmappable_scalar_u_e5e5() {
    // U+E5E5 is the only Unicode scalar WHATWG's gb18030 encoder cannot map;
    // encoding_rs would silently write the literal text `&#58853;`. The
    // encoder refuses instead of writing mojibake.
    let err = transcode("x \u{E5E5} y".as_bytes(), TargetEncoding::Gb18030).unwrap_err();
    assert_eq!(err.category(), "encode_error");
    // Neighbors map fine and round-trip.
    for cp in ["\u{E5E4}", "\u{E5E6}", "\u{4E2D}"] {
        match transcode(cp.as_bytes(), TargetEncoding::Gb18030).unwrap() {
            TranscodeOutcome::Changed(bytes) => {
                let back = transcode(&bytes, TargetEncoding::Utf8).unwrap();
                match back {
                    TranscodeOutcome::Changed(restored) => {
                        assert_eq!(restored, cp.as_bytes().to_vec(), "round trip of {cp}");
                    }
                    TranscodeOutcome::Unchanged => panic!("gb bytes must restore to {cp}"),
                }
            }
            TranscodeOutcome::Unchanged => panic!("{cp} must encode to gb18030"),
        }
    }
}
