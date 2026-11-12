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

//! End-to-end pipeline tests on a mixed-encoding temp tree: scan → dry-run →
//! convert → verify → idempotence, plus backup integrity, whitelist
//! protection, and JSON report shape.

use std::fs;
use std::path::Path;

use clap::Parser;
use serde_json::Value;
use utf8_forge::cli::{Cli, Commands, ConvertArgs, DiffArgs, FixArgs, TreeArgs};
use utf8_forge::transcoder::TargetEncoding;

const GBK: &[u8] = &[0xC4, 0xE3, 0xBA, 0xC3]; // "你好" GB18030
const UTF8: &[u8] = "你好".as_bytes();

/// Write bytes to `root/rel` creating parents.
fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, bytes).unwrap();
}

/// Minimal TreeArgs for a temp root.
fn args(root: &Path, report: Option<&Path>) -> TreeArgs {
    TreeArgs {
        root: root.to_path_buf(),
        report: report.map(|p| p.to_path_buf()),
        ..Default::default()
    }
}

fn run_tree(cmd: impl Fn(TreeArgs) -> Commands, a: &TreeArgs) -> i32 {
    utf8_forge::run(Cli {
        command: cmd(a.clone()),
    })
}

/// Run `convert` in tree mode (no --url) from plain TreeArgs, with an
/// optional hook to set conversion-only fields (target/dry-run/backup-dir).
fn run_convert(a: &TreeArgs, modify: impl FnOnce(&mut ConvertArgs)) -> i32 {
    let mut ca = ConvertArgs {
        common: a.clone(),
        ..Default::default()
    };
    modify(&mut ca);
    utf8_forge::run(Cli {
        command: Commands::Convert(ca),
    })
}

fn read_report(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The standard mixed-encoding test tree.
fn build_mixed_tree(root: &Path) {
    put(root, "a.cpp", GBK); // GB18030 → convert
    put(root, "b.h", b"// plain ascii\n"); // UTF-8 → unchanged
    put(root, "c.txt", &[0xEF, 0xBB, 0xBF, b'x']); // BOM → convert (strip)
    put(root, "d.rs", &[0xFF, 0xFE, b'h', 0, b'i', 0]); // UTF-16LE → convert
    put(root, "sub/e.md", GBK); // GB18030 in subdir → convert
    put(root, "unknown.cpp", &[0xFF, 0xFF, 0xFF]); // Unknown → skipped
    put(root, "dep/skip.cpp", GBK); // excluded dir
    put(root, "OutputFile1/skip.cpp", GBK); // excluded prefix
    put(root, ".hidden.cpp", GBK); // hidden
    put(root, "image.png", b"\x89PNG"); // not whitelisted
}

#[test]
fn full_rollout_loop_scan_dry_run_convert_verify_idempotent() {
    let tree = tempfile::tempdir().unwrap();
    // The scanned tree is a subdirectory so report files (which are
    // themselves whitelisted .json) never pollute the counts.
    let root = tree.path().join("tree");
    let reports = tree.path();
    build_mixed_tree(&root);

    // ---------------- scan ----------------
    let scan_report = reports.join("scan.json");
    let code = run_tree(Commands::Scan, &args(&root, Some(&scan_report)));
    // 1 unknown file → exit 1.
    assert_eq!(code, 1);
    let doc = read_report(&scan_report);
    assert_eq!(doc["tool"], "utf8-forge");
    assert_eq!(doc["mode"], "scan");
    assert_eq!(doc["summary"]["total_files"], 6);
    assert_eq!(doc["summary"]["candidates"], 6);
    assert_eq!(doc["summary"]["utf8"], 1); // b.h
    assert_eq!(doc["summary"]["utf8_bom"], 1); // c.txt
    assert_eq!(doc["summary"]["utf16le"], 1); // d.rs
    assert_eq!(doc["summary"]["gb18030"], 2); // a.cpp + sub/e.md
    assert_eq!(doc["summary"]["unknown"], 1);
    // Whitelist protection: dep/, hidden, binary, prefix dirs absent.
    let paths: Vec<&str> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(paths.iter().all(|p| {
        let p = p.replace('\\', "/");
        !p.contains("dep/")
            && !p.contains(".hidden")
            && !p.contains("OutputFile1")
            && !p.contains("image.png")
    }));
    // files[] is sorted by path.
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    // Scan never writes.
    assert_eq!(fs::read(root.join("a.cpp")).unwrap(), GBK);

    // ---------------- dry-run ----------------
    let dry_report = reports.join("dry.json");
    let dry_args = args(&root, Some(&dry_report));
    let code = run_convert(&dry_args, |c| c.dry_run = true);
    assert_eq!(code, 1); // unknown still present
    let doc = read_report(&dry_report);
    assert_eq!(doc["mode"], "convert");
    let actions: Vec<(&str, &str)> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let p = f["path"].as_str().unwrap();
            let name = Path::new(p).file_name().unwrap().to_str().unwrap();
            (name, f["action"].as_str().unwrap())
        })
        .collect();
    assert_eq!(actions.len(), 6);
    for (name, action) in &actions {
        match *name {
            "a.cpp" | "c.txt" | "d.rs" | "e.md" => assert_eq!(*action, "would_convert"),
            "b.h" => assert_eq!(*action, "unchanged"),
            "unknown.cpp" => assert_eq!(*action, "skipped_unknown"),
            _ => panic!("unexpected file {name}"),
        }
    }
    // Dry-run writes nothing.
    assert_eq!(fs::read(root.join("a.cpp")).unwrap(), GBK);
    assert_eq!(
        fs::read(root.join("d.rs")).unwrap(),
        &[0xFF, 0xFE, b'h', 0, b'i', 0]
    );

    // ---------------- real convert with backup ----------------
    let b_h_mtime_before = fs::metadata(root.join("b.h")).unwrap().modified().unwrap();
    let backup = tree.path().join("backup");
    let apply_report = reports.join("apply.json");
    let conv_args = args(&root, Some(&apply_report));
    let code = run_convert(&conv_args, |c| c.backup_dir = Some(backup.clone()));
    assert_eq!(code, 1); // unknown.cpp remains
    let doc = read_report(&apply_report);
    assert_eq!(doc["summary"]["converted"], 4);
    assert_eq!(doc["summary"]["already_utf8"], 1);
    assert_eq!(doc["summary"]["unknown"], 1);
    assert_eq!(doc["summary"]["decode_errors"], 0);
    assert_eq!(doc["summary"]["io_errors"], 0);

    // Converted contents are now UTF-8 without BOM.
    assert_eq!(fs::read(root.join("a.cpp")).unwrap(), UTF8);
    assert_eq!(fs::read(root.join("c.txt")).unwrap(), b"x");
    assert_eq!(fs::read(root.join("d.rs")).unwrap(), b"hi");
    assert_eq!(fs::read(root.join("sub/e.md")).unwrap(), UTF8);
    // Unchanged file keeps its original bytes and its mtime.
    assert_eq!(fs::read(root.join("b.h")).unwrap(), b"// plain ascii\n");
    assert_eq!(
        fs::metadata(root.join("b.h")).unwrap().modified().unwrap(),
        b_h_mtime_before,
        "unchanged file must not be rewritten"
    );
    // Protected files keep theirs too.
    assert_eq!(fs::read(root.join("dep/skip.cpp")).unwrap(), GBK);

    // Backup holds the originals, mirrored (only written files are backed up).
    assert_eq!(fs::read(backup.join("a.cpp")).unwrap(), GBK);
    assert_eq!(
        fs::read(backup.join("c.txt")).unwrap(),
        &[0xEF, 0xBB, 0xBF, b'x']
    );
    assert_eq!(
        fs::read(backup.join("d.rs")).unwrap(),
        &[0xFF, 0xFE, b'h', 0, b'i', 0]
    );
    assert_eq!(fs::read(backup.join("sub/e.md")).unwrap(), GBK);
    assert!(
        !backup.join("b.h").exists(),
        "unchanged file must not be backed up"
    );
    assert!(
        !backup.join("dep").exists(),
        "excluded file must not be backed up"
    );

    // ---------------- verify ----------------
    let verify_report = reports.join("verify1.json");
    let code = run_tree(Commands::Verify, &args(&root, Some(&verify_report)));
    assert_eq!(code, 3); // unknown.cpp is still non-UTF-8 — expected while unknowns remain
    let doc = read_report(&verify_report);
    assert_eq!(doc["mode"], "verify");
    assert_eq!(doc["summary"]["passed"], 5);
    assert_eq!(doc["summary"]["failed"], 1);
    let error_path = doc["error_files"][0].as_str().unwrap();
    assert_eq!(
        Path::new(error_path).file_name().unwrap().to_str().unwrap(),
        "unknown.cpp"
    );

    // ---------------- P1 idempotence: second convert = 0 changes ----------------
    let apply2_report = reports.join("apply2.json");
    let code = run_convert(&args(&root, Some(&apply2_report)), |_| {});
    assert_eq!(code, 1); // unknown persists, nothing else
    let doc = read_report(&apply2_report);
    assert_eq!(doc["summary"]["converted"], 0);
    assert_eq!(doc["summary"]["already_utf8"], 5);
    // Backup untouched by the second pass.
    assert_eq!(fs::read(backup.join("a.cpp")).unwrap(), GBK);
}

#[test]
fn verify_passes_clean_tree_and_fails_on_bom() {
    // Clean tree → exit 0.
    let clean = tempfile::tempdir().unwrap();
    put(clean.path(), "ok.cpp", b"int main() {}\n");
    assert_eq!(run_tree(Commands::Verify, &args(clean.path(), None)), 0);

    // BOM'd tree → exit 3.
    let dirty = tempfile::tempdir().unwrap();
    put(dirty.path(), "bom.h", &[0xEF, 0xBB, 0xBF, b'x']);
    put(dirty.path(), "ok.h", b"x");
    let report = dirty.path().join("v.json");
    let code = run_tree(Commands::Verify, &args(dirty.path(), Some(&report)));
    assert_eq!(code, 3);
    let doc = read_report(&report);
    assert_eq!(doc["summary"]["failed"], 1);
    let failed = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["action"] == "failed")
        .unwrap();
    assert_eq!(failed["detected"], "utf-8-bom");
}

#[test]
fn clean_tree_scan_exit_0_and_convert_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", GBK);
    put(dir.path(), "b.h", b"ascii");
    assert_eq!(run_tree(Commands::Scan, &args(dir.path(), None)), 0);
    assert_eq!(run_convert(&args(dir.path(), None), |_| {}), 0);
    // Idempotence at CLI level.
    let report = dir.path().join("second.json");
    assert_eq!(run_convert(&args(dir.path(), Some(&report)), |_| {}), 0);
    assert_eq!(read_report(&report)["summary"]["converted"], 0);
}

#[test]
fn reverse_conversion_to_gb18030_is_idempotent() {
    // Reverse conversion: UTF-8 → GB18030 for legacy scripts.
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "run.bat", UTF8);
    let mut a = args(dir.path(), None);
    a.ext = vec!["bat".into()];
    let report = dir.path().join("r.json");
    a.report = Some(report.clone());
    assert_eq!(
        run_convert(&a, |c| c.target =
            Some(utf8_forge::transcoder::TargetEncoding::Gb18030)),
        0
    );
    assert_eq!(fs::read(dir.path().join("run.bat")).unwrap(), GBK);
    // Second pass: GB18030 → GB18030 is byte-identical → unchanged.
    assert_eq!(
        run_convert(&a, |c| c.target =
            Some(utf8_forge::transcoder::TargetEncoding::Gb18030)),
        0
    );
    assert_eq!(read_report(&report)["summary"]["converted"], 0);
}

#[test]
fn conflict_flags_are_fatal_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"x");
    let mut a = args(dir.path(), None);
    a.include_rc = true;
    a.exclude_ext = vec!["rc".into()];
    assert_eq!(run_convert(&a, |_| {}), 2);
}

#[test]
fn missing_root_is_fatal_exit_2() {
    let a = TreeArgs {
        root: std::path::PathBuf::from("/definitely/not/a/real/root/xyz"),
        ..Default::default()
    };
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );
}

#[test]
fn threads_option_processes_same_files() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..8 {
        put(dir.path(), &format!("f{i}.cpp"), GBK);
    }
    let mut a = args(dir.path(), None);
    a.threads = Some(2);
    assert_eq!(run_convert(&a, |_| {}), 0);
    for i in 0..8 {
        assert_eq!(
            fs::read(dir.path().join(format!("f{i}.cpp"))).unwrap(),
            UTF8
        );
    }
}

#[test]
fn report_options_block_reflects_effective_filter() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", GBK);
    let report = dir.path().join("opt.json");
    let mut a = args(dir.path(), Some(&report));
    a.exclude_glob = vec!["**/Generated/**".into()];
    a.ext = vec!["cpp".into()];
    run_convert(&a, |_| {});
    let doc = read_report(&report);
    assert_eq!(doc["options"]["target"], "utf-8");
    assert_eq!(doc["options"]["dry_run"], false);
    assert_eq!(doc["options"]["extensions"], serde_json::json!(["cpp"]));
    assert_eq!(
        doc["options"]["exclude_globs"],
        serde_json::json!(["**/Generated/**"])
    );
    assert!(doc["options"]["exclude_dirs"].as_array().unwrap().len() > 20);
    assert!(doc["duration_ms"].is_u64());
    assert!(doc["started_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn backup_dir_inside_root_is_fatal_exit_2() {
    // Backups under the root would be walked and re-converted on the next
    // run, so the configuration must be rejected.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("backup")).unwrap();
    put(&root, "a.cpp", GBK);
    let a = args(&root, None);
    let code = run_convert(&a, |c| c.backup_dir = Some(root.join("backup")));
    assert_eq!(code, 2);
    // Nothing was converted and no backup files were written.
    assert_eq!(fs::read(root.join("a.cpp")).unwrap(), GBK);

    // Placement is also checked under --dry-run when the directory already
    // exists: a preview must not bless a config the real run rejects.
    let code = run_convert(&a, |c| {
        c.dry_run = true;
        c.backup_dir = Some(root.join("backup"));
    });
    assert_eq!(code, 2);
}

#[cfg(unix)]
#[test]
fn unwritable_existing_report_dir_is_fatal_before_conversion() {
    // create_dir_all is a no-op on an existing read-only directory — a
    // writability probe must catch it up front.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "bom.cpp", &[0xEF, 0xBB, 0xBF, b'x']);
    let ro = dir.path().join("ro-out");
    fs::create_dir_all(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();

    let code = run_convert(&args(&root, Some(&ro.join("r.json"))), |_| {});
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        code, 2,
        "unwritable report dir must abort before conversion"
    );
    assert_eq!(
        fs::read(root.join("bom.cpp")).unwrap(),
        &[0xEF, 0xBB, 0xBF, b'x'],
        "tree must be untouched when the report pre-flight fails"
    );
}

#[test]
fn report_inside_tree_is_excluded_from_its_own_run() {
    // Repeated audits reusing the same report path must not count the report
    // itself (it is excluded from traversal).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", GBK);
    let report = root.join("scan.json");

    let code = run_tree(Commands::Scan, &args(&root, Some(&report)));
    assert_eq!(code, 0);
    let first = read_report(&report);
    assert_eq!(first["summary"]["total_files"], 1);

    // Second audit, same report path — the report file must not appear.
    run_tree(Commands::Scan, &args(&root, Some(&report)));
    let second = read_report(&report);
    assert_eq!(
        second["summary"]["total_files"], 1,
        "report file must be excluded from traversal"
    );
}

#[cfg(unix)]
#[test]
fn read_only_file_is_refused_never_overwritten() {
    // POSIX rename would bypass chmod 444; the tool must refuse.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "locked.cpp", GBK);
    fs::set_permissions(root.join("locked.cpp"), fs::Permissions::from_mode(0o444)).unwrap();

    let code = run_convert(&args(&root, None), |_| {});
    assert_eq!(code, 1, "per-file io error → exit 1");
    assert_eq!(
        fs::read(root.join("locked.cpp")).unwrap(),
        GBK,
        "read-only file must be untouched"
    );
    let mode = fs::metadata(root.join("locked.cpp"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o444);

    fs::set_permissions(root.join("locked.cpp"), fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(unix)]
#[test]
fn scan_exits_1_on_io_errors() {
    // Unreadable file → scan reports the issue and exits 1.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "ok.cpp", b"ascii");
    put(&root, "secret.cpp", GBK);
    fs::set_permissions(root.join("secret.cpp"), fs::Permissions::from_mode(0o000)).unwrap();

    let report = dir.path().join("r.json");
    let code = run_tree(Commands::Scan, &args(&root, Some(&report)));
    fs::set_permissions(root.join("secret.cpp"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code, 1, "io_error records must fail the scan");
    let doc = read_report(&report);
    let bad = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("secret.cpp"))
        .expect("the unreadable file must be recorded");
    assert_eq!(bad["action"], "error");
    assert!(bad["error"].as_str().unwrap().starts_with("io_error:"));
    assert!(doc["summary"]["errors"].as_u64().unwrap() >= 1);
}

#[test]
fn scan_and_verify_reject_conversion_only_flags() {
    // --target/--dry-run/--backup-dir are convert-only; misuse is a clap
    // parse error → exit 2 (verified through the real binary).
    let bin = env!("CARGO_BIN_EXE_utf8-forge");
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"x");

    for args in [
        vec![
            "scan",
            "--root",
            dir.path().to_str().unwrap(),
            "--target",
            "utf-8",
        ],
        vec![
            "verify",
            "--root",
            dir.path().to_str().unwrap(),
            "--dry-run",
        ],
    ] {
        let out = std::process::Command::new(bin)
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?} must be rejected");
    }
}

// ---------------------------------------------------------------------------
// Feature-surface tests (config / ignore-file / fix / diff)
// ---------------------------------------------------------------------------

fn run_fix(a: &TreeArgs, modify: impl FnOnce(&mut FixArgs)) -> i32 {
    let mut fa = FixArgs {
        common: a.clone(),
        ..Default::default()
    };
    modify(&mut fa);
    utf8_forge::run(Cli {
        command: Commands::Fix(fa),
    })
}

/// Config file replaces the whitelist; CLI wins on conflict; unknown
/// keys and missing files are fatal.
#[test]
fn config_file_precedence_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    put(&root, "b.h", b"x");
    let cfg = dir.path().join("forge.toml");
    fs::write(&cfg, "[filter]\nextensions = [\"h\"]\n").unwrap();

    // 1. Without a config both files are scanned; with the config the
    // whitelist is replaced (only .h).
    let code = run_tree(Commands::Scan, &args(&root, None));
    assert_eq!(code, 0);
    let mut a = args(&root, None);
    a.config = Some(cfg.clone());
    let report = dir.path().join("c1.json");
    a.report = Some(report.clone());
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a.clone())
        }),
        0
    );
    let doc = read_report(&report);
    assert_eq!(doc["summary"]["total_files"], 1, "only .h scanned");
    let scanned: Vec<&str> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        scanned.iter().all(|p| p.ends_with("b.h")),
        "config whitelist must select only b.h, got {scanned:?}"
    );
    assert_eq!(
        doc["options"]["config_file"].as_str().unwrap(),
        cfg.to_string_lossy()
    );

    // 2. CLI --ext wins over config (only a.cpp, not b.h).
    let report2 = dir.path().join("c2.json");
    a.report = Some(report2.clone());
    a.ext = vec!["cpp".into()];
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        0
    );
    let doc2 = read_report(&report2);
    assert_eq!(doc2["summary"]["total_files"], 1);
    assert!(
        doc2["files"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("a.cpp")
    );

    // 3. unknown key → exit 2.
    let bad = dir.path().join("bad.toml");
    fs::write(&bad, "[filter]\ntypo_key = 1\n").unwrap();
    let mut a = args(&root, None);
    a.config = Some(bad);
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );

    // 4. malformed TOML → exit 2.
    let bad2 = dir.path().join("bad2.toml");
    fs::write(&bad2, "[filter").unwrap();
    let mut a = args(&root, None);
    a.config = Some(bad2);
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );

    // 5. missing file → exit 2.
    let mut a = args(&root, None);
    a.config = Some(dir.path().join("missing.toml"));
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );
}

/// Ignore file — explicit fatal-missing, auto-discovery, negation
/// last-match-wins, `--no-ignore-file`, layer independence.
#[test]
fn ignore_file_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    put(&root, "drop.md", b"x");
    put(&root, "keep.md", b"x");

    // Auto-discovery via .utf8forgeignore at root.
    fs::write(root.join(".utf8forgeignore"), "*.md\n!keep.md\n").unwrap();
    let report = dir.path().join("ig.json");
    run_tree(Commands::Scan, &args(&root, Some(&report)));
    let doc = read_report(&report);
    assert_eq!(doc["summary"]["total_files"], 2, "a.cpp + keep.md");
    assert_eq!(
        doc["options"]["ignore_file"].as_str().unwrap(),
        root.join(".utf8forgeignore").to_string_lossy()
    );
    let paths: Vec<&str> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let p = f["path"].as_str().unwrap();
            Path::new(p).file_name().unwrap().to_str().unwrap()
        })
        .collect();
    assert!(paths.contains(&"keep.md"), "{paths:?}");
    assert!(!paths.contains(&"drop.md"), "{paths:?}");

    // --no-ignore-file disables discovery: all three files scanned.
    let mut a = args(&root, None);
    a.no_ignore_file = true;
    let report2 = dir.path().join("ig2.json");
    a.report = Some(report2.clone());
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        0
    );
    assert_eq!(read_report(&report2)["summary"]["total_files"], 3);
    assert!(read_report(&report2)["options"]["ignore_file"].is_null());

    // Explicit missing ignore file → exit 2.
    let mut a = args(&root, None);
    a.ignore_file = Some(dir.path().join("missing.ignore"));
    a.no_ignore_file = false;
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );

    // Negation cannot cross layers: --exclude-glob still applies.
    fs::write(root.join(".utf8forgeignore"), "!*.cpp\n").unwrap();
    let mut a = args(&root, None);
    a.exclude_glob = vec!["**/a.cpp".into()];
    let report3 = dir.path().join("ig3.json");
    a.report = Some(report3.clone());
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        0
    );
    let doc = read_report(&report3);
    assert_eq!(
        doc["summary"]["total_files"], 2,
        "negation lifts only the ignore layer, not the glob layer"
    );
}

/// Fix = convert + verify in one pass; the gate wins the exit code.
#[test]
fn fix_subcommand_gate_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", GBK); // needs conversion
    put(&root, "b.h", b"ascii"); // already clean
    put(&root, "unknown.cpp", &[0xFF, 0xFF, 0xFF]); // unfixable
    let report = dir.path().join("fix.json");

    // Mixed tree: unknown file fails the gate → exit 3.
    let code = run_fix(&args(&root, Some(&report)), |_| {});
    assert_eq!(code, 3, "gate failure wins over convert issues");
    let doc = read_report(&report);
    assert_eq!(doc["mode"], "fix");
    assert_eq!(doc["summary"]["converted"], 1);
    assert_eq!(doc["summary"]["already_utf8"], 1);
    assert_eq!(doc["summary"]["unknown"], 1);
    assert_eq!(doc["summary"]["passed"], 2);
    assert_eq!(doc["summary"]["failed"], 1);
    assert_eq!(doc["verify_failed_files"].as_array().unwrap().len(), 1);

    // Second fix on the converted tree: only unknown.cpp still fails.
    let report2 = dir.path().join("fix2.json");
    let code = run_fix(&args(&root, Some(&report2)), |_| {});
    assert_eq!(code, 3);
    assert_eq!(read_report(&report2)["summary"]["converted"], 0);

    // Clean tree → exit 0, all passed.
    let clean = tempfile::tempdir().unwrap();
    put(clean.path(), "ok.cpp", b"ascii");
    let report3 = clean.path().join("fix3.json");
    let code = run_fix(&args(clean.path(), Some(&report3)), |_| {});
    assert_eq!(code, 0);
    let doc = read_report(&report3);
    assert_eq!(doc["summary"]["failed"], 0);
    assert!(doc["verify_failed_files"].is_null(), "omitted when clean");

    // Dry-run: convert pass writes nothing; verify reports the dirty state.
    let dirty = tempfile::tempdir().unwrap();
    put(dirty.path(), "x.cpp", GBK);
    let mut a = args(dirty.path(), None);
    a.report = None;
    let code = run_fix(&a, |c| c.dry_run = true);
    assert_eq!(code, 3, "dry-run preview reports the current end state");
    assert_eq!(fs::read(dirty.path().join("x.cpp")).unwrap(), GBK);
}

/// Diff compares two reports; --fail-on-change gates.
#[test]
fn diff_subcommand_compare_and_gate() {
    let dir = tempfile::tempdir().unwrap();
    // Build a real convert report pair: first convert (converted), then
    // second convert (unchanged) — diff must show the action drift.
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", GBK);
    let rep1 = dir.path().join("apply.json");
    assert_eq!(run_convert(&args(&root, Some(&rep1)), |_| {}), 0);
    let rep2 = dir.path().join("apply2.json");
    assert_eq!(run_convert(&args(&root, Some(&rep2)), |_| {}), 0);

    let bin = env!("CARGO_BIN_EXE_utf8-forge");
    let out = std::process::Command::new(bin)
        .args([
            "diff",
            "--before",
            rep1.to_str().unwrap(),
            "--after",
            rep2.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "default exit is 0");
    // Stdout carries only the JSON document (piping into other tools stays
    // clean); the summary line lives on stderr.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"mode\": \"diff\""), "JSON on stdout");
    assert!(
        !stdout.contains("[utf8-forge]"),
        "no console-prefixed lines on stdout: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("diff: added="), "{stderr}");
    assert!(stderr.contains("changed=1"), "{stderr}");

    // --fail-on-change: drift → exit 1.
    let out = std::process::Command::new(bin)
        .args([
            "diff",
            "--before",
            rep1.to_str().unwrap(),
            "--after",
            rep2.to_str().unwrap(),
            "--fail-on-change",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));

    // Identical reports → exit 0 even with --fail-on-change.
    let out = std::process::Command::new(bin)
        .args([
            "diff",
            "--before",
            rep2.to_str().unwrap(),
            "--after",
            rep2.to_str().unwrap(),
            "--fail-on-change",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Malformed input → exit 2.
    let bad = dir.path().join("bad.json");
    fs::write(&bad, "{ truncated").unwrap();
    let out = std::process::Command::new(bin)
        .args([
            "diff",
            "--before",
            bad.to_str().unwrap(),
            "--after",
            rep2.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// A UTF-16LE target refuses NUL-leading content: the output would begin
/// `FF FE 00 00` (the UTF-32LE BOM), so the file is reported as an error and
/// left byte-identical, and every rerun reports the same thing.
#[test]
fn utf16le_target_refuses_nul_leading_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "nul.txt", b"\x00B\n");
    let original = fs::read(root.join("nul.txt")).unwrap();

    let report = dir.path().join("r1.json");
    let code = run_convert(&args(&root, Some(&report)), |a| {
        a.target = Some(TargetEncoding::Utf16Le)
    });
    assert_eq!(code, 1, "encode_error is a per-file issue");
    let doc = read_report(&report);
    assert_eq!(doc["files"][0]["action"], "error");
    assert!(
        doc["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("encode_error")
    );
    assert_eq!(doc["summary"]["encode_errors"], 1);
    assert_eq!(doc["summary"]["io_errors"], 0);
    assert_eq!(fs::read(root.join("nul.txt")).unwrap(), original);

    // Rerun is byte- and verdict-stable: no drift accumulates.
    let report2 = dir.path().join("r2.json");
    let code2 = run_convert(&args(&root, Some(&report2)), |a| {
        a.target = Some(TargetEncoding::Utf16Le)
    });
    assert_eq!(code2, 1);
    assert_eq!(fs::read(root.join("nul.txt")).unwrap(), original);
    assert_eq!(
        read_report(&report2)["files"][0]["action"],
        "error",
        "the refusal must be idempotent, not a one-time effect"
    );
}

/// Fix accepts only the default target: any other target is a fatal (exit 2)
/// before anything is touched, because the verify gate asserts UTF-8.
#[test]
fn fix_rejects_non_utf8_target() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "x.cpp", GBK);
    let original = fs::read(root.join("x.cpp")).unwrap();

    let code = run_fix(&args(&root, None), |a| {
        a.target = Some(TargetEncoding::Gb18030)
    });
    assert_eq!(code, 2);
    assert_eq!(
        fs::read(root.join("x.cpp")).unwrap(),
        original,
        "a rejected fix must not touch the tree"
    );
}

/// Diff requires real utf8-forge reports (a `files` array) and compares paths
/// relative to each report's root, so the same tree under two roots diffs
/// clean while a non-report JSON is fatal.
#[test]
fn diff_schema_check_and_root_relativization() {
    let dir = tempfile::tempdir().unwrap();
    let tree1 = dir.path().join("tree1");
    let tree2 = dir.path().join("tree2");
    for root in [&tree1, &tree2] {
        fs::create_dir_all(root).unwrap();
        put(root, "a.cpp", b"same");
        put(root, "b.h", b"same");
    }
    let r1 = dir.path().join("r1.json");
    let r2 = dir.path().join("r2.json");
    assert_eq!(run_tree(Commands::Scan, &args(&tree1, Some(&r1))), 0);
    assert_eq!(run_tree(Commands::Scan, &args(&tree2, Some(&r2))), 0);

    // Same tree under two roots → clean diff (paths relativized per root).
    let dargs = DiffArgs {
        before: r1.clone(),
        after: r2.clone(),
        report: None,
        fail_on_change: false,
    };
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Diff(dargs)
        }),
        0
    );

    // --fail-on-change on a clean diff stays 0.
    let dargs = DiffArgs {
        before: r1.clone(),
        after: r2.clone(),
        report: None,
        fail_on_change: true,
    };
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Diff(dargs)
        }),
        0
    );

    // Non-report JSON is fatal, not an empty baseline.
    let wrong = dir.path().join("wrong.json");
    fs::write(&wrong, r#"{"schema": "wrong"}"#).unwrap();
    let dargs = DiffArgs {
        before: wrong.clone(),
        after: r2.clone(),
        report: None,
        fail_on_change: false,
    };
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Diff(dargs)
        }),
        2
    );
    fs::write(&wrong, "[]").unwrap();
    let dargs = DiffArgs {
        before: wrong,
        after: r2,
        report: None,
        fail_on_change: false,
    };
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Diff(dargs)
        }),
        2
    );
}

/// diff --report writes the diff document to a file (branch coverage).
#[test]
fn diff_report_file_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", GBK);
    let r1 = dir.path().join("r1.json");
    let r2 = dir.path().join("r2.json");
    assert_eq!(run_tree(Commands::Scan, &args(&root, Some(&r1))), 0);
    fs::write(root.join("a.cpp"), UTF8).unwrap();
    assert_eq!(run_tree(Commands::Scan, &args(&root, Some(&r2))), 0);

    let out = dir.path().join("diff.json");
    let dargs = DiffArgs {
        before: r1,
        after: r2,
        report: Some(out.clone()),
        fail_on_change: true,
    };
    let code = utf8_forge::run(Cli {
        command: Commands::Diff(dargs),
    });
    assert_eq!(code, 1, "the drift must trip --fail-on-change");
    let doc = read_report(&out);
    assert_eq!(doc["mode"], "diff");
    assert_eq!(doc["summary"]["action_changed"], 1);
    assert_eq!(doc["changed"][0]["path"], "a.cpp");
}

/// Oversized files are bucketed without affecting the scan verdict, and the
/// summary identity `total == buckets + errors + too_large` holds.
#[test]
fn scan_buckets_oversized_files_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "big.cpp", b"longer-than-two-bytes");
    put(&root, "small.cpp", b"ok");

    let report = dir.path().join("r.json");
    let mut a = args(&root, Some(&report));
    a.max_file_size = Some(2);
    assert_eq!(run_tree(Commands::Scan, &a), 0, "too_large is not an error");
    let doc = read_report(&report);
    let s = &doc["summary"];
    assert_eq!(s["too_large"], 1);
    assert_eq!(s["total_files"], 2);
    assert_eq!(s["candidates"], 1, "candidates = total - too_large");
    let buckets = s["utf8"].as_u64().unwrap()
        + s["utf8_bom"].as_u64().unwrap()
        + s["utf16le"].as_u64().unwrap()
        + s["utf16be"].as_u64().unwrap()
        + s["utf32le"].as_u64().unwrap()
        + s["utf32be"].as_u64().unwrap()
        + s["gb18030"].as_u64().unwrap()
        + s["unknown"].as_u64().unwrap();
    assert_eq!(buckets + s["errors"].as_u64().unwrap(), s["candidates"]);
    assert_eq!(
        buckets + s["errors"].as_u64().unwrap() + s["too_large"].as_u64().unwrap(),
        s["total_files"]
    );
}

/// An empty file is already at the target encoding: unchanged, never written.
#[test]
fn convert_empty_file_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "empty.cpp", b"");
    let report = dir.path().join("r.json");
    assert_eq!(run_convert(&args(&root, Some(&report)), |_| {}), 0);
    let doc = read_report(&report);
    assert_eq!(doc["files"][0]["action"], "unchanged");
    assert_eq!(doc["files"][0]["bytes_before"], 0);
}

/// A utf-8-bom target converges in one pass; the second pass changes nothing.
#[test]
fn utf8_bom_target_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "x.cpp", UTF8);

    let report1 = dir.path().join("r1.json");
    let code1 = run_convert(&args(&root, Some(&report1)), |a| {
        a.target = Some(TargetEncoding::Utf8Bom)
    });
    assert_eq!(code1, 0);
    assert_eq!(read_report(&report1)["summary"]["converted"], 1);

    let report2 = dir.path().join("r2.json");
    let code2 = run_convert(&args(&root, Some(&report2)), |a| {
        a.target = Some(TargetEncoding::Utf8Bom)
    });
    assert_eq!(code2, 0);
    let doc2 = read_report(&report2);
    assert_eq!(doc2["summary"]["already_utf8"], 1);
    assert_eq!(doc2["summary"]["converted"], 0);
}

/// Fix supports --backup-dir: originals are mirrored before the rewrite.
#[test]
fn fix_with_backup_dir_mirrors_originals() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "x.cpp", GBK);
    let backup = dir.path().join("backup");

    let code = run_fix(&args(&root, None), |a| {
        a.backup_dir = Some(backup.clone());
    });
    assert_eq!(code, 0);
    let backed = backup.join("x.cpp");
    assert_eq!(fs::read(&backed).unwrap(), GBK, "backup holds the original");
    assert_eq!(fs::read(root.join("x.cpp")).unwrap(), UTF8);
}

/// An explicit `--ext` that normalizes to nothing is fatal, not a silent
/// zero-file scan.
#[test]
fn ext_list_normalizing_to_empty_is_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");

    let mut a = args(&root, None);
    a.ext = vec![String::new()];
    assert_eq!(run_tree(Commands::Scan, &a), 2);
}

/// `--output` is only meaningful with `--url`; the parser rejects the
/// tree-mode combination at the clap level (exit 2).
#[test]
fn output_flag_requires_url() {
    let err = Cli::try_parse_from(["utf8-forge", "convert", "--root", ".", "--output", "o.bin"]);
    assert!(err.is_err(), "--output without --url must be a parse error");
    let ok = Cli::try_parse_from([
        "utf8-forge",
        "convert",
        "--root",
        ".",
        "--url",
        "https://example.com/a.cpp",
        "--output",
        "o.bin",
    ]);
    assert!(ok.is_ok(), "--output with --url must parse");
}

/// An invalid `[convert].target` value is fatal in every tree mode, naming
/// the config key — scan/verify never silently ignore the section.
#[test]
fn invalid_config_target_is_fatal_for_scan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    let cfg = dir.path().join("bad.toml");
    fs::write(&cfg, "[convert]\ntarget = \"bogus\"\n").unwrap();

    let mut a = args(&root, None);
    a.config = Some(cfg);
    assert_eq!(run_tree(Commands::Scan, &a), 2);
    assert_eq!(run_tree(Commands::Verify, &a), 2);
}

/// An unreadable auto-discovered .utf8forgeignore warns and continues the
/// scan without the ignore layer instead of failing the run.
#[test]
fn unreadable_auto_ignore_file_warns_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    let ignore = root.join(".utf8forgeignore");
    fs::write(&ignore, "a.cpp\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ignore, fs::Permissions::from_mode(0o000)).unwrap();
    }
    #[cfg(not(unix))]
    {
        // Non-unix: keep the file readable; the branch is covered on unix.
        return;
    }

    let report = dir.path().join("r.json");
    assert_eq!(
        run_tree(Commands::Scan, &args(&root, Some(&report))),
        0,
        "the unreadable ignore file must not fail the run"
    );
    let doc = read_report(&report);
    assert_eq!(
        doc["summary"]["total_files"], 1,
        "a.cpp is scanned because the ignore layer was dropped with a warning"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&ignore, fs::Permissions::from_mode(0o644));
    }
}

/// A valid source basename must also work for backups and atomic replacement.
#[cfg(unix)]
#[test]
fn convert_long_filename_preserves_backup_and_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    let backup = dir.path().join("backup");
    let name = format!("{}.cpp", "a".repeat(236));
    put(&root, &name, GBK);
    let path = root.join(&name);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    let a = args(&root, None);
    assert_eq!(run_tree(Commands::Scan, &a), 0);
    assert_eq!(run_convert(&a, |c| c.dry_run = true), 0);
    assert_eq!(fs::read(&path).unwrap(), GBK);
    assert_eq!(run_convert(&a, |c| c.backup_dir = Some(backup.clone())), 0);
    assert_eq!(fs::read(&path).unwrap(), UTF8);
    assert_eq!(fs::read(backup.join(&name)).unwrap(), GBK);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(run_tree(Commands::Verify, &a), 0);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(run_convert(&a, |_| {}), 0);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
}

/// Auto-discovery warnings must be visible in verify as well as scan.
#[test]
fn verify_reports_invalid_auto_ignore_file_warning() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.cpp", b"ascii");
    put(dir.path(), ".utf8forgeignore", b"!\n");

    for quiet in [false, true] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_utf8-forge"));
        command.arg("verify").arg("--root").arg(dir.path());
        if quiet {
            command.arg("--quiet");
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        if quiet {
            assert!(stderr.is_empty(), "{stderr}");
        } else {
            assert!(
                stderr.contains("warning: ignoring unreadable .utf8forgeignore"),
                "{stderr}"
            );
            assert!(stderr.contains("line 1"), "{stderr}");
        }
    }
}

/// Size limits apply consistently at each public per-file entry point.
#[test]
fn per_file_size_limits_preserve_oversized_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.cpp");
    fs::write(&path, GBK).unwrap();
    let mut ctx = utf8_forge::ConvertCtx {
        root: dir.path().to_path_buf(),
        target: TargetEncoding::Utf8,
        dry_run: false,
        backup_dir: Some(dir.path().join("backup")),
        max_file_size: 3,
    };
    assert_eq!(utf8_forge::scan_file(&path, 3).action, "skipped_too_large");
    assert_eq!(
        utf8_forge::convert_file(&path, &ctx).action,
        "skipped_too_large"
    );
    let record = utf8_forge::verify_file(&path, 3);
    assert_eq!(record.action, "failed");
    assert!(record.error.unwrap().starts_with("too_large"));
    assert_eq!(fs::read(&path).unwrap(), GBK);
    assert!(!ctx.backup_dir.as_ref().unwrap().exists());

    ctx.max_file_size = GBK.len() as u64;
    assert_eq!(
        utf8_forge::scan_file(&path, ctx.max_file_size).detected,
        Some("gb18030")
    );
    assert_eq!(utf8_forge::convert_file(&path, &ctx).action, "converted");
    assert_eq!(fs::read(&path).unwrap(), UTF8);
    assert_eq!(
        utf8_forge::verify_file(&path, UTF8.len() as u64).action,
        "passed"
    );
}

/// Conversion and fix share config precedence across all config sections.
#[test]
fn convert_and_fix_merge_config_with_cli_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    let backup = dir.path().join("backup");
    let override_backup = dir.path().join("override-backup");
    put(&root, "a.h", GBK);
    put(&root, "b.cpp", GBK);
    let config = dir.path().join("forge.toml");
    fs::write(&config, format!(
        "[filter]\nextensions = [\"h\"]\n[convert]\ntarget = \"utf-8-bom\"\nbackup_dir = {}\n[options]\nmax_file_size = 1024\nthreads = 1\n",
        serde_json::to_string(&backup).unwrap()
    )).unwrap();
    let report = dir.path().join("report.json");
    let mut a = args(&root, Some(&report));
    a.config = Some(config);

    // These helpers use an explicit UTF-8 target by default. None models
    // the CLI invocation with --target omitted, allowing config to apply.
    assert_eq!(
        run_convert(&a, |c| {
            c.target = None;
            c.dry_run = true;
        }),
        0
    );
    assert!(!backup.exists());
    assert_eq!(run_convert(&a, |c| c.target = None), 0);
    let bom_utf8 = [b"\xEF\xBB\xBF".as_slice(), UTF8].concat();
    assert_eq!(fs::read(root.join("a.h")).unwrap(), bom_utf8);
    assert_eq!(fs::read(root.join("b.cpp")).unwrap(), GBK);
    assert_eq!(fs::read(backup.join("a.h")).unwrap(), GBK);
    assert_eq!(read_report(&report)["options"]["target"], "utf-8-bom");

    assert_eq!(run_fix(&a, |c| c.target = None), 2);
    assert_eq!(
        run_fix(&a, |c| {
            c.target = Some(TargetEncoding::Utf8);
            c.backup_dir = Some(override_backup.clone());
        }),
        0
    );
    assert_eq!(fs::read(root.join("a.h")).unwrap(), UTF8);
    assert_eq!(fs::read(override_backup.join("a.h")).unwrap(), bom_utf8);
    assert_eq!(fs::read(backup.join("a.h")).unwrap(), GBK);
    assert_eq!(read_report(&report)["options"]["target"], "utf-8");
}

/// Thread counts beyond 1024 are a fatal input error: rayon pool startup
/// cost grows quadratically and implausible values silently wedge the
/// process. The bound itself stays accepted.
#[test]
fn threads_above_maximum_is_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");

    let mut a = args(&root, None);
    a.threads = Some(1025);
    assert_eq!(run_tree(Commands::Scan, &a), 2);

    a.threads = Some(1024);
    assert_eq!(
        run_tree(Commands::Scan, &a),
        0,
        "the maximum itself is accepted"
    );
}

/// The config file's [options].max_file_size applies to runs built on
/// TreeArgs::default (the cap comes from the merge, not the struct default),
/// while a run without --config keeps the built-in 200 MiB cap.
#[test]
fn config_max_file_size_applies_to_default_runs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "big.cpp", b"longer-than-two");
    let cfg = dir.path().join("forge.toml");
    fs::write(&cfg, "[options]\nmax_file_size = 2\n").unwrap();

    let report = dir.path().join("r.json");
    let mut a = args(&root, Some(&report));
    a.config = Some(cfg);
    assert_eq!(run_tree(Commands::Scan, &a), 0);
    let doc = read_report(&report);
    assert_eq!(doc["summary"]["too_large"], 1);
    assert_eq!(doc["summary"]["candidates"], 0);

    // Without --config the built-in cap applies and the file is processed.
    let report2 = dir.path().join("r2.json");
    assert_eq!(run_tree(Commands::Scan, &args(&root, Some(&report2))), 0);
    assert_eq!(read_report(&report2)["summary"]["too_large"], 0);
}

/// fix fails closed (exit 3) on a whitelisted file that exceeds the size
/// cap: a gate must not wave through what it cannot inspect.
#[test]
fn fix_fails_closed_on_oversized_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "big.cpp", GBK);

    let mut a = args(&root, None);
    a.max_file_size = Some(2);
    assert_eq!(run_fix(&a, |_| {}), 3);
    assert_eq!(fs::read(root.join("big.cpp")).unwrap(), GBK);
}

/// Non-UTF-8 filenames survive the pipeline lossily: the file is scanned,
/// reported under a replacement-character path, and never panics. APFS
/// (macOS) rejects non-UTF-8 names at creation, so this runs on Linux only.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_filename_is_reported_lossily() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    let p = root.join(OsStr::from_bytes(b"bad\xff.cpp"));
    fs::write(&p, b"plain\n").unwrap();

    let report = dir.path().join("r.json");
    assert_eq!(run_tree(Commands::Scan, &args(&root, Some(&report))), 0);
    let doc = read_report(&report);
    let path = doc["files"][0]["path"].as_str().unwrap();
    assert!(path.contains('\u{FFFD}'), "lossy path in {path:?}");
    assert_eq!(doc["files"][0]["detected"], "utf-8");
}
/// A whitelisted file inside a directory the traversal cannot enter must not
/// slip past the gate: verify fails closed (exit 3), the warning survives
/// --quiet in the report, and a clean tree stays exit 0.
#[test]
#[cfg(unix)]
fn verify_gate_fails_closed_on_unreadable_subtree() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    put(&root, "top.cpp", b"plain");
    put(&locked, "bom.cpp", &[0xEF, 0xBB, 0xBF, b'x']);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let report = dir.path().join("r.json");
    let mut a = args(&root, Some(&report));
    a.quiet = true;
    let code = run_tree(Commands::Verify, &a);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, 3, "an uninspectable subtree must fail the gate");
    let doc = read_report(&report);
    assert!(
        !doc["warnings"].as_array().unwrap().is_empty(),
        "the walk warning must reach the report even under -q"
    );
    // fix shares the gate semantics.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let code_fix = run_fix(&a, |c| c.dry_run = true);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code_fix, 3, "fix applies the same gate semantics");
}

/// A write-protected existing report file is refused at pre-flight instead
/// of being silently replaced (and re-stamped read-only) by the rename.
#[test]
#[cfg(unix)]
fn write_protected_report_file_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    let report = dir.path().join("ro.json");
    fs::write(&report, b"old").unwrap();
    fs::set_permissions(&report, fs::Permissions::from_mode(0o444)).unwrap();

    let code = run_tree(Commands::Scan, &args(&root, Some(&report)));
    assert_eq!(code, 2, "a read-only report must be a fatal pre-flight");
    assert_eq!(fs::read(&report).unwrap(), b"old");
    fs::set_permissions(&report, fs::Permissions::from_mode(0o644)).unwrap();
}

/// The report target may never be the active ignore rule file.
#[test]
fn report_target_cannot_be_the_ignore_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a.cpp", b"x");
    let ignore = root.join(".utf8forgeignore");
    fs::write(&ignore, "*.md\n").unwrap();
    let before = fs::read(&ignore).unwrap();

    let code = run_tree(Commands::Scan, &args(&root, Some(&ignore)));
    assert_eq!(code, 2, "the ignore file must not become the report target");
    assert_eq!(fs::read(&ignore).unwrap(), before);
}
