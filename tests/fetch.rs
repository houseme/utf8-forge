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

//! Remote fetch tests using a minimal std-only mock HTTP
//! server — no extra dev-dependencies. Gated on the `remote` feature.

#![cfg(feature = "remote")]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use serde_json::Value;
use utf8_forge::cli::{Commands, ConvertArgs, TreeArgs};
use utf8_forge::fetcher::{derive_output_path, fetch_bytes};
use utf8_forge::run;

/// Response plan for the mock server.
struct Response {
    status: u16,
    /// Send a Content-Length header with this value (None → omit).
    content_length: Option<usize>,
    body: Vec<u8>,
    /// Close the connection without Content-Length (body read till EOF).
    close_without_length: bool,
}

/// Serve exactly one HTTP response on an ephemeral port.
fn serve_once(resp: Response) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept failed");
        drain_request(&mut stream);
        let head = if resp.close_without_length {
            format!("HTTP/1.1 {} MOCK\r\nConnection: close\r\n\r\n", resp.status)
        } else {
            format!(
                "HTTP/1.1 {} MOCK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp.status,
                resp.content_length.unwrap_or(resp.body.len())
            )
        };
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&resp.body).unwrap();
        stream.flush().unwrap();
        // Connection drops → close.
    });
    (format!("http://{addr}/file.cpp"), handle)
}

/// Read the request until the end of headers (bodies are not sent by the client).
fn drain_request(stream: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

const GBK_NIHAO: &[u8] = &[0xC4, 0xE3, 0xBA, 0xC3]; // "你好"
const UTF8_NIHAO: &[u8] = "你好".as_bytes();

#[test]
fn fetch_bytes_transcodes_gbk_response() {
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(GBK_NIHAO.len()),
        body: GBK_NIHAO.to_vec(),
        close_without_length: false,
    });
    let bytes = fetch_bytes(&url, 200 * 1024 * 1024).unwrap();
    assert_eq!(bytes, GBK_NIHAO);
}

#[test]
fn fetch_bytes_rejects_declared_oversize_before_reading() {
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(1000),
        body: b"tiny".to_vec(), // lie; the header check fires first
        close_without_length: false,
    });
    let err = fetch_bytes(&url, 10).unwrap_err();
    assert_eq!(err.category(), "too_large");
}

#[test]
fn fetch_bytes_rejects_actual_oversize_without_content_length() {
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: None,
        body: vec![b'x'; 64],
        close_without_length: true,
    });
    let err = fetch_bytes(&url, 16).unwrap_err();
    assert_eq!(err.category(), "too_large");
}

#[test]
fn fetch_bytes_maps_http_error_status() {
    let (url, _server) = serve_once(Response {
        status: 404,
        content_length: Some(0),
        body: Vec::new(),
        close_without_length: false,
    });
    let err = fetch_bytes(&url, 200 * 1024 * 1024).unwrap_err();
    assert_eq!(err.category(), "fetch_error");
}

#[test]
fn derive_output_path_rules() {
    // Documented derivation examples.
    assert_eq!(
        derive_output_path("https://example.com/data/legacy.cpp"),
        PathBuf::from("legacy.cpp")
    );
    assert_eq!(
        derive_output_path("https://example.com/foo"),
        PathBuf::from("foo.utf8")
    );
    assert_eq!(
        derive_output_path("https://example.com/"),
        PathBuf::from("download.utf8")
    );
}

/// Build `convert --url` args pointing at a temp output file.
fn convert_url_args(url: String, output: PathBuf, report: PathBuf, max: u64) -> ConvertArgs {
    ConvertArgs {
        common: TreeArgs {
            report: Some(report),
            max_file_size: Some(max),
            ..Default::default()
        },
        url: Some(url),
        output: Some(output),
        ..Default::default()
    }
}

#[test]
fn convert_url_writes_transcoded_output_and_report() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("legacy.cpp");
    let report = dir.path().join("fetch-report.json");
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(GBK_NIHAO.len()),
        body: GBK_NIHAO.to_vec(),
        close_without_length: false,
    });

    let code =
        run(Commands::Convert(convert_url_args(url, out.clone(), report.clone(), 1024)).into());
    assert_eq!(code, 0);

    assert_eq!(std::fs::read(&out).unwrap(), UTF8_NIHAO);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["mode"], "fetch");
    assert_eq!(doc["summary"]["converted"], 1);
    assert_eq!(doc["summary"]["fetch_error"], 0);
    assert!(doc["url"].as_str().unwrap().starts_with("http://127.0.0.1"));
    assert_eq!(doc["files"][0]["action"], "converted");
    assert_eq!(doc["files"][0]["detected"], "gb18030");
}

#[test]
fn convert_url_dry_run_reports_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("legacy.cpp");
    let report = dir.path().join("dry.json");
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(GBK_NIHAO.len()),
        body: GBK_NIHAO.to_vec(),
        close_without_length: false,
    });

    let args = ConvertArgs {
        common: TreeArgs {
            report: Some(report.clone()),
            ..Default::default()
        },
        dry_run: true,
        url: Some(url),
        output: Some(out.clone()),
        ..Default::default()
    };
    let code = run(Commands::Convert(args).into());
    assert_eq!(code, 0);
    assert!(!out.exists(), "dry-run must not write the output");

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["files"][0]["action"], "would_convert");
}

#[test]
fn convert_url_oversize_payload_is_exit_1_with_report() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.cpp");
    let report = dir.path().join("fetch-report.json");
    let body = vec![b'x'; 64];
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(body.len()),
        body,
        close_without_length: false,
    });

    let code = run(Commands::Convert(convert_url_args(url, out, report.clone(), 16)).into());
    assert_eq!(code, 1);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["summary"]["fetch_error"], 0);
    assert_eq!(doc["summary"]["too_large"], 1);
    assert_eq!(doc["files"][0]["action"], "skipped_too_large");
    assert!(
        doc["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("too_large")
    );
}

#[test]
fn convert_url_404_is_exit_1_with_fetch_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.cpp");
    let report = dir.path().join("fetch-report.json");
    let (url, _server) = serve_once(Response {
        status: 404,
        content_length: Some(0),
        body: Vec::new(),
        close_without_length: false,
    });

    let code = run(Commands::Convert(convert_url_args(url, out, report.clone(), 1024)).into());
    assert_eq!(code, 1);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["summary"]["fetch_error"], 1);
}

/// Serve the same response twice (two sequential connections).
fn serve_twice(resp: Response) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            drain_request(&mut stream);
            let head = format!(
                "HTTP/1.1 {} MOCK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp.status,
                resp.content_length.unwrap_or(resp.body.len())
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&resp.body).unwrap();
            stream.flush().unwrap();
        }
    });
    (format!("http://{addr}/file.cpp"), handle)
}

#[test]
fn convert_url_identical_refetch_is_unchanged_no_rewrite() {
    // Destination already holds the same bytes → `unchanged`, no write churn.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("plain.cpp");
    let report = dir.path().join("fetch-report.json");
    let body: Vec<u8> = UTF8_NIHAO.to_vec();
    let (url, _server) = serve_twice(Response {
        status: 200,
        content_length: Some(body.len()),
        body: body.clone(),
        close_without_length: false,
    });

    let args1 = convert_url_args(url.clone(), out.clone(), report.clone(), 1024);
    assert_eq!(run(Commands::Convert(args1).into()), 0);
    let mtime1 = fs::metadata(&out).unwrap().modified().unwrap();
    assert_eq!(fs::read(&out).unwrap(), body);

    let args2 = convert_url_args(url, out.clone(), report.clone(), 1024);
    assert_eq!(run(Commands::Convert(args2).into()), 0);
    let mtime2 = fs::metadata(&out).unwrap().modified().unwrap();
    assert_eq!(
        mtime1, mtime2,
        "identical refetch must not rewrite the file"
    );

    let doc: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["files"][0]["action"], "unchanged");
    assert_eq!(doc["summary"]["already_utf8"], 1);
}

/// A decode-invalid payload yields exactly one error record (decode_error),
/// exit 1, and no output file.
#[test]
fn fetch_decode_error_records_once_and_fails() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bad.cpp");
    let report = dir.path().join("fetch-report.json");
    // UTF-16LE BOM + an unpaired high surrogate → strict decode refuses.
    let body: Vec<u8> = vec![0xFF, 0xFE, 0x00, 0xD8, 0x00, 0xD8];
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(body.len()),
        body,
        close_without_length: false,
    });
    let args = convert_url_args(url, out.clone(), report.clone(), 1024);
    assert_eq!(run(Commands::Convert(args).into()), 1);
    let doc: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["summary"]["decode_errors"], 1);
    assert!(
        doc["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("decode_error")
    );
    assert!(!out.exists(), "a failed fetch must not write the output");
}

/// The derived output path's parent directories are created on demand.
#[test]
fn fetch_output_parent_dirs_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deep").join("nested").join("out.cpp");
    let report = dir.path().join("fetch-report.json");
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(GBK_NIHAO.len()),
        body: GBK_NIHAO.to_vec(),
        close_without_length: false,
    });
    let args = convert_url_args(url, out.clone(), report, 1024);
    assert_eq!(run(Commands::Convert(args).into()), 0);
    assert_eq!(fs::read(&out).unwrap(), UTF8_NIHAO);
}

/// `--config` has no effect in fetch mode (the only mergeable key,
/// [convert].target, would silently change nothing), so the combination is
/// fatal before any network access.
#[test]
fn fetch_with_config_is_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("forge.toml");
    fs::write(&cfg, "[convert]\ntarget = \"gb18030\"\n").unwrap();
    let args = ConvertArgs {
        common: TreeArgs {
            config: Some(cfg),
            ..Default::default()
        },
        url: Some("http://127.0.0.1:9/never-reached.cpp".into()),
        ..Default::default()
    };
    assert_eq!(run(Commands::Convert(args).into()), 2);
}

/// Fetch runs the same report pre-flight as tree modes: an unusable report
/// path aborts (exit 2) before the download happens.
#[test]
fn fetch_report_preflight_runs_before_download() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.cpp");
    // The report path is an existing directory → fatal, nothing fetched.
    let report_dir = dir.path().join("report-dir");
    std::fs::create_dir_all(&report_dir).unwrap();
    let args = ConvertArgs {
        common: TreeArgs {
            report: Some(report_dir),
            ..Default::default()
        },
        url: Some("http://127.0.0.1:9/never-reached.cpp".into()),
        output: Some(out.clone()),
        ..Default::default()
    };
    assert_eq!(run(Commands::Convert(args).into()), 2);
    assert!(!out.exists(), "the pre-flight must abort before any output");
}

/// A bounded destination comparison must distinguish equal prefixes from
/// equal files, including the dry-run path.
#[test]
fn fetch_replaces_longer_destination_with_matching_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.cpp");
    let report = dir.path().join("report.json");
    let original = [UTF8_NIHAO, b"extra bytes"].concat();
    fs::write(&out, &original).unwrap();

    for dry_run in [true, false] {
        let (url, server) = serve_once(Response {
            status: 200,
            content_length: Some(GBK_NIHAO.len()),
            body: GBK_NIHAO.to_vec(),
            close_without_length: false,
        });
        let mut args = convert_url_args(url, out.clone(), report.clone(), 1024);
        args.dry_run = dry_run;
        assert_eq!(run(Commands::Convert(args).into()), 0);
        server.join().unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        if dry_run {
            assert_eq!(fs::read(&out).unwrap(), original);
            assert_eq!(doc["files"][0]["action"], "would_convert");
        } else {
            assert_eq!(fs::read(&out).unwrap(), UTF8_NIHAO);
            assert_eq!(doc["files"][0]["action"], "converted");
        }
    }
}

/// An unclassifiable payload gets the same verdict as tree mode for the
/// same bytes: `skipped_unknown`, the summary `unknown` bucket, and the
/// `unknown_files` list — not the generic error bucket.
#[test]
fn fetch_unknown_payload_is_skipped_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("unknown.cpp");
    let report = dir.path().join("fetch-report.json");
    let body: Vec<u8> = vec![0xFF, 0xFF, 0xFF];
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(body.len()),
        body,
        close_without_length: false,
    });
    let args = convert_url_args(url, out.clone(), report.clone(), 1024);
    assert_eq!(run(Commands::Convert(args).into()), 1);
    let doc: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["files"][0]["action"], "skipped_unknown");
    assert_eq!(doc["summary"]["unknown"], 1);
    assert_eq!(doc["summary"]["io_errors"], 0);
    assert_eq!(doc["unknown_files"].as_array().unwrap().len(), 1);
    assert!(doc["error_files"].as_array().unwrap().is_empty());
    assert!(!out.exists(), "an unknown payload must not be written");
}

/// A write-protected destination is refused exactly like tree mode: the
/// record is an error naming the protection, and the file is untouched.
#[test]
#[cfg(unix)]
fn fetch_refuses_write_protected_destination() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("locked.cpp");
    fs::write(&out, b"old bytes").unwrap();
    fs::set_permissions(&out, fs::Permissions::from_mode(0o444)).unwrap();
    let report = dir.path().join("fetch-report.json");
    let body = GBK_NIHAO.to_vec();
    let (url, _server) = serve_once(Response {
        status: 200,
        content_length: Some(body.len()),
        body,
        close_without_length: false,
    });
    let args = convert_url_args(url, out.clone(), report.clone(), 1024);
    assert_eq!(run(Commands::Convert(args).into()), 1);
    let doc: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(doc["files"][0]["action"], "error");
    assert!(
        doc["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("write-protected")
    );
    assert_eq!(fs::read(&out).unwrap(), b"old bytes");
    let mode = fs::metadata(&out).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o444,
        "the locked mode must survive the refusal"
    );
    // Restore so tempdir cleanup can remove the file.
    fs::set_permissions(&out, fs::Permissions::from_mode(0o644)).unwrap();
}
