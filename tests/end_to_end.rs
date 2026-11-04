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

use serde_json::Value;
use utf8_forge::cli::{Cli, Commands, ConvertArgs, FixArgs, TreeArgs};

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

/// The mixed tree from the rollout playbook.
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

    // ---------------- P0: scan ----------------
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
    assert!(paths.iter().all(|p| !p.contains("dep/")
        && !p.contains(".hidden")
        && !p.contains("OutputFile1")
        && !p.contains("image.png")));
    // Invariant #8: report files sorted by path.
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    // Scan never writes.
    assert_eq!(fs::read(root.join("a.cpp")).unwrap(), GBK);

    // ---------------- P1: dry-run ----------------
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
            let name = p.rsplit('/').next().unwrap();
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

    // ---------------- P1: real convert with backup ----------------
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
    assert_eq!(
        doc["error_files"][0]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap(),
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

    let code = run_tree(Commands::Scan, &args(&root, None));
    fs::set_permissions(root.join("secret.cpp"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code, 1, "io_error records must fail the scan");
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
// R5 feature acceptance tests (config / ignore-file / fix / diff)
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

/// R5.2: config file replaces the whitelist; CLI wins on conflict; unknown
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

    // 1. config replaces the whitelist.
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
    assert_eq!(
        doc["options"]["config_file"].as_str().unwrap(),
        cfg.to_string_lossy()
    );

    // 2. CLI --ext wins over config.
    let report2 = dir.path().join("c2.json");
    a.report = Some(report2.clone());
    a.ext = vec!["cpp".into()];
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        0
    );
    assert_eq!(read_report(&report2)["summary"]["total_files"], 1);

    // 4. unknown key → exit 2.
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

    // 5. malformed TOML → exit 2.
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

    // 6. missing file → exit 2.
    let mut a = args(&root, None);
    a.config = Some(dir.path().join("missing.toml"));
    assert_eq!(
        utf8_forge::run(Cli {
            command: Commands::Scan(a)
        }),
        2
    );
}

/// R5.1: ignore file — explicit fatal-missing, auto-discovery, negation
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
        .map(|f| f["path"].as_str().unwrap().rsplit('/').next().unwrap())
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

/// R5.5: fix = convert + verify in one pass; gate wins the exit code.
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

/// R5.6: diff compares two reports; --fail-on-change gates.
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("changed=1"), "{stdout}");
    assert!(stdout.contains("\"mode\": \"diff\""), "JSON on stdout");

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
