# AGENT.md — utf8-forge Maintenance Guide

> **Audience**: AI agents and new maintainers.
> **Goal**: Understand, extend, and safely modify `utf8-forge` without breaking invariants.

---

## 1. What This Tool Is

`utf8-forge` is a **small, focused Rust CLI** that:

- Unifies **mixed-encoding source trees** to **UTF-8 without BOM** (default).
- Supports **bidirectional** conversion (`→ utf-8-bom / gb18030 / utf-16le / utf-16be`).
- Optionally **fetches a remote file** (`--url`) and transcodes it in one step.
- Aligns with **P1 (Source Transcoding)** of the *Source UTF-8 Unification Playbook*.

**It does not**: compile, run tests, touch SVN/Git, process binaries, or modify `dep/`.

---

## 2. 30-Second Quick Start

```bash
# Build
cargo build --release

# 1) Scan distribution
./target/release/utf8-forge scan --root D:\server --report scan.json

# 2) Dry-run
./target/release/utf8-forge convert --root D:\server --dry-run --report dry.json

# 3) Convert with backup
./target/release/utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge

# 4) Verify
./target/release/utf8-forge verify --root D:\server
```

**Safety rule**: `scan` → `--dry-run` → `convert`. Never skip step 2.

---

## 3. Commands

| Command         | Purpose                                       |         Writes?         | Options note                                                       | Exit codes |
|-----------------|-----------------------------------------------|:-----------------------:|--------------------------------------------------------------------|------------|
| `scan`          | Detect only; distribution report              |           ❌            | —                                                                  | 0 / 1 / 2* |
| `convert`       | Detect + transcode + atomic write             | ✅ (unless `--dry-run`) | `--target/--dry-run/--backup-dir` are convert/fix-only             | 0 / 1 / 2* |
| `convert --url` | Fetch → transcode → save                      |           ✅            | `--url/--output`; `--target/--dry-run` allowed, `--root` ignored   | 0 / 1 / 2* |
| `verify`        | Assert all whitelisted files are UTF-8 no BOM |           ❌            | —                                                                  | 0 / 3 / 2* |
| `fix`           | Convert + verify in one pass, no re-walk      | ✅ (unless `--dry-run`) | Tree flags + `--target` (utf-8 only) / `--dry-run` / `--backup-dir`; no `--url/--output` | 0 / 3 / 1 / 2* |
| `diff`          | Compare two reports per path                  |           ❌            | `--before/--after/--report/--fail-on-change`; reads JSON, writes nothing else | 0 / 1† / 2* |

\* `2` = fatal setup/parse errors (missing root, bad flags, conflicts); also emitted by clap. `scan` and `verify`
**reject** the convert/fix-only flags `--target` / `--dry-run` / `--backup-dir` with exit 2.
† `1` only with `--fail-on-change` on a non-empty diff; otherwise `diff` exits 0 even with differences.

---

## 4. Core Options

| Option              | Default         | Notes                                                            |
|---------------------|-----------------|------------------------------------------------------------------|
| `-r, --root`        | `.`             | Recursive                                                        |
| `-e, --ext`         | See DESIGN §5.1 | **Overrides** default whitelist; empty normalized list (`--ext ""`) → exit 2 |
| `-X, --exclude-ext` | empty           | Appended                                                         |
| `-x, --exclude`     | See DESIGN §5.2 | Appended                                                         |
| `--exclude-glob`    | empty           | Repeatable; see `docs/USAGE.md §3` for glob semantics            |
| `--include-rc`      | false           | Adds `.rc/.rc2/.dlg`                                             |
| `--include-msbuild` | false           | Adds `.sln/.vcxproj/...`                                         |
| `--include-hidden`  | false           | Processes dot-prefixed files **and** directories                 |
| `--config`          | none            | TOML defaults; CLI > config > built-ins; missing/malformed/unknown keys → exit 2; fatal (exit 2) combined with `--url` (fetch reads no config) |
| `--ignore-file`     | auto            | Gitignore-style exclusions; explicit missing → exit 2; replaces auto-discovery of `<root>/.utf8forgeignore` |
| `--no-ignore-file`  | false           | Disables ignore-file handling entirely, including auto-discovery |
| `-t, --target`      | `utf-8`         | `utf-8-bom`, `gb18030`, `utf-16le`, `utf-16be` (convert; fix accepts only the default `utf-8`) |
| `--dry-run`         | false           | No writes (convert, fix)                                         |
| `--backup-dir`      | none            | Original bytes preserved (convert, fix)                          |
| `--max-file-size`   | 200 MiB         | Above → `too_large`                                              |
| `--threads`         | CPU count       | Rayon pool size (values below `1` clamp to `1`)                  |
| `--report`          | none            | JSON report path (auto-excluded from traversal)                  |
| `--url`             | none            | **Remote fetch (convert only)**                                  |
| `--output`          | derived         | Destination for `--url` (requires `--url`)                       |
| `-q, --quiet`       | false           | Errors only                                                      |
| `-v, --verbose`     | false           | Per-file detail                                                  |

Value placeholders in `--help` are the option's field name (e.g. `<ROOT>`,
`<EXCLUDE_EXT>`); this table uses descriptive names for the same values.

**Conflicts** (fatal, exit 2):

- `--include-rc` + `--exclude-ext rc|rc2|dlg`
- `--include-msbuild` + `--exclude-ext sln|vcxproj|vcproj|props|targets|filters|user`
- `--ignore-file` + `--no-ignore-file`
- `--config` + `--url` (fetch reads no config)

**Safety notes**:

- `--backup-dir` inside `--root` → fatal (exit 2), nothing touched (existing
  directories are also checked under `--dry-run`).
- Read-only (write-protected) files are refused with an io error — never overwritten.
- `--backup-dir` cannot be combined with `--url` (fatal, exit 2).

---

## 5. Source Layout & English Comment Policy

```
src/
├── main.rs       # Thin entry: parse CLI, std::process::exit(run(cli))
├── lib.rs        # run() dispatch, per-file pipelines (scan_file/convert_file/verify_file), report assembly, exit codes
├── cli.rs        # clap derive; all flags documented with /// English
├── config.rs     # TOML config file; see DESIGN §5.5
├── detector.rs   # Encoding sniffing; see DESIGN §3
├── transcoder.rs # Strict decode / encode; see DESIGN §4
├── filter.rs     # Whitelist + blacklist; see DESIGN §5
├── ignorefile.rs # Gitignore-style exclusions; see DESIGN §5.4
├── walker.rs     # walkdir wrapper
├── writer.rs     # Atomic write; see DESIGN §7
├── fetcher.rs    # Optional remote download; see DESIGN §8
├── reporter.rs   # Console + JSON
├── differ.rs     # `diff` subcommand report comparison
└── error.rs      # ForgeError domain errors (categories feed the report); anyhow wraps fatal setup errors
```

**Rule**: Every `pub fn`, `pub struct`, and non-obvious `fn` carries a `///` doc comment in **English**. Internal tricky
blocks use `// English explanation.` No Chinese in `src/`.

---

## 6. Key Invariants (Must Read Before Coding)

| #  | Invariant                                | Consequence of Violation          |
|----|------------------------------------------|-----------------------------------|
| 1  | Default output = UTF-8 **no BOM**        | Breaks Playbook Strategy A        |
| 2  | UTF-16 encoders **manually prepend BOM** | Output unreadable                 |
| 3  | No newline normalization                 | Diff noise                        |
| 4  | Whitelist + blacklist double gate        | `dep/` or `.exe` corruption       |
| 5  | Byte-identical → no write                | mtime churn / SVN false positives |
| 6  | Temp file + rename                       | Source loss on power failure      |
| 7  | `had_errors == true` → hard error        | Half-written mojibake             |
| 8  | Report sorted by path                    | Non-reproducible audits           |
| 9  | No symlink recursion                     | Infinite loop                     |
| 10 | Second `convert` = 0 changes (double-BOM inputs converge in two passes — see the decode rules) | Idempotence regression            |

**Do not modify the code unless you can keep all 10 true.**

---

## 7. How to Extend

### 7.1 Add a Language Extension

Edit `src/filter.rs::DEFAULT_EXTENSIONS`; update `DESIGN.md §5.1`; add an integration test that places a `.xyz` file and
asserts `scan` recognizes it.

### 7.2 Add an Excluded Directory

Edit `DEFAULT_EXCLUDE_DIRS` or `DEFAULT_EXCLUDE_PREFIXES` in `src/filter.rs`. **Never** hard-code names in `walker.rs`.

### 7.3 Add a Target Encoding

1. Extend `TargetEncoding` enum in `transcoder.rs`.
2. Add alias in `parse()`.
3. Implement in `encode()` — **remember BOM if stateful**.
4. If round-trip detection is needed, extend `DetectedEncoding` and `detect()`.
5. Update `DESIGN.md §2`.
6. Add round-trip test (`A→B→A == A`) in `tests/transcode.rs`.

### 7.4 Modify Remote Fetch Behaviour

`src/fetcher.rs` is isolated. Changes there **must not** affect `detect()` / `transcode()` / `writer::atomic_write`.
Follow the streaming/buffering split in `DESIGN §8.3`.

### 7.5 Add a New Subcommand

1. Add variant in `cli.rs::Commands`.
2. Dispatch it in `lib.rs::run()` (`main.rs` stays a thin wrapper).
3. Reuse `walker::collect_files` and `filter::*` — do not duplicate traversal logic.
4. New write commands **must** support `--dry-run` and `atomic_write`.

### 7.6 Add a Config Key

Config keys live in `src/config.rs` (`ForgeConfig` and its section structs).
A new key must: (1) be added to the right section struct with `#[serde(default)]`,
(2) be merged with the same precedence rule (CLI > config > built-in; lists
replace, never concatenate), (3) keep `[serde(deny_unknown_fields)]` strict —
unknown keys are fatal on purpose, and (4) update `DESIGN.md §5.5`. Fetch mode
must keep ignoring `--config`.

---

## 8. Code Quality Gates

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (unit + integration)
- **No `unwrap()` / `expect()`** in non-test code. Use `anyhow::Result`.
- English comments only in `src/`. Chinese belongs in `README.zh-CN.md` / `docs/USAGE.zh-CN.md`.

---

## 9. Troubleshooting Cheat Sheet

| Symptom                        | Check                                                                |
|--------------------------------|----------------------------------------------------------------------|
| Files skipped unexpectedly     | `-v`; verify extension, excluded dir, hidden flag, `--max-file-size` |
| Files unexpectedly ignored     | Check `<root>/.utf8forgeignore`, `--ignore-file`, `--no-ignore-file`; remember a negation cannot re-include under an excluded parent |
| Files not detected (report inside tree) | The report path is auto-excluded; keep it outside the tree if you want it counted |
| High `Unknown` count           | Real binaries with wrong extension — inspect with hex editor         |
| `DecodeError`                  | Truncated byte sequences; also malformed UTF-32 payloads (surrogates, values > `0x10FFFF`, misaligned tails); do **not** change heuristics |
| Second `convert` changes files | Violation of invariant #5 or #10 (double-BOM inputs converge in two passes — expected once) |
| `verify` exit 3                | Non-UTF-8 whitelisted files; `-v` lists them                         |
| `fix` exit 3                   | The verify gate wins: unknown/oversized/unreadable files all fail closed; see `verify_failed_files` |
| Permission denied on convert   | Read-only files are refused; `chmod +w` first                        |
| `--url` fetch fails            | Check timeout, TLS, `Content-Length` vs `--max-file-size`            |
| VS re-adds BOM to `.vcxproj`   | Normal; MSBuild editor behaviour, not a tool bug                     |

---

## 10. Mapping to Playbook P0–P4

| Playbook Phase   | Tool Role                    |
|------------------|------------------------------|
| P0 Inventory     | `scan --report`              |
| P1 Transcoding   | `convert` (main battlefield) |
| P2 `/utf-8` flag | ❌ Not this tool             |
| P3 Rebuild       | `verify` as gate             |
| P4 CI            | `verify` in pre-commit / CI  |

See `EXECUTION.md` for concrete steps.

---

## 11. Documentation Files

| File                  | Language              | Purpose                        |
|-----------------------|-----------------------|--------------------------------|
| `README.md`           | **English (default)** | Overview, install, quick start |
| `README.zh-CN.md`     | Chinese               | Same content, localized        |
| `docs/USAGE.md`       | **English**           | Full CLI reference, recipes    |
| `docs/USAGE.zh-CN.md` | Chinese               | Same content, localized        |
| `DESIGN.md`           | English               | Architecture & invariants      |
| `AGENT.md`            | English               | This file                      |
| `EXECUTION.md`        | English               | `D:\server` rollout playbook   |
| `ROADMAP.md`          | English               | Planned/done work, release record |

---

*AGENT.md v0.1 — The 10 invariants in §6 are non-negotiable.*