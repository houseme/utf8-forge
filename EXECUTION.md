# EXECUTION.md — utf8-forge Rollout Playbook

> **Target**: `D:\server` multi-solution Windows C++ game server.
> **Aligned with**: Playbook P0–P4 (*Source UTF-8 Unification & Cross-Platform Charset*).
> **Scope**: Source → UTF-8 without BOM only. `/utf-8` flag, rebuild, and smoke tests are covered in the original
> Playbook §2–§3.

---

## 0. Five-Minute Version

```bash
cargo build --release

./target/release/utf8-forge scan    --root D:\server --report D:\tmp\scan.json
./target/release/utf8-forge convert --root D:\server --dry-run --report D:\tmp\dry.json
./target/release/utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge
./target/release/utf8-forge verify  --root D:\server

# Shortcut: convert + verify in one pass (exit 3 when the gate fails)
./target/release/utf8-forge fix --root D:\server --report D:\tmp\fix.json
```

**If any step reports `Unknown` or `DecodeError`, stop and read §6.**

---

## 1. Prerequisites

| Item        | Requirement                                                       |
|-------------|-------------------------------------------------------------------|
| OS          | Windows 10/11 (tool is cross-platform; rollout target is Windows) |
| Rust        | Stable toolchain supporting edition 2024                          |
| SVN         | Working copy **clean**; coordinate a "transcode window"           |
| Disk        | Space for backup (≈ source tree size)                             |
| Permissions | Read/write on `D:\server`                                         |
| IDE         | Close auto-save on the target directory                           |

```bat
cd D:\server
svn status      :: must be empty
svn info        :: record revision
```

---

## 2. Build & Install

```bash
# In-tree
cd utf8-forge
cargo build --release
# Binary: target\release\utf8-forge.exe

# Or install to PATH
cargo install --path . --locked
```

**Smoke**:

```bash
utf8-forge --version
utf8-forge scan --help
```

> If you enabled only default features, `reqwest` is compiled in. To build a **local-only** binary:
> ```bash
> cargo build --release --no-default-features
> ```

---

## 3. Execution Flow (P0 → P1)

### 3.1 P0 — Inventory & Freeze

```bat
utf8-forge scan ^
  --root D:\server ^
  --report D:\backup\utf8-forge\scan-before.json ^
  --verbose
```

**Expected summary** (excerpt, order of magnitude):

```json
{
  "summary": {
    "total_files": 1247,
    "candidates": 1142,
    "utf8": 1035,
    "utf8_bom": 34,
    "utf16le": 2,
    "utf16be": 0,
    "utf32le": 0,
    "utf32be": 0,
    "gb18030": 171,
    "unknown": 5,
    "errors": 0,
    "too_large": 0
  }
}
```

**Check**:

- `gb18030` ≈ 97 (Playbook §1.2);
- `utf16le` ≈ 2 (`GameServer/room/FieldData.cpp` × 2);
- Every `unknown` is **manually confirmed**.

**Exit criteria**: report archived; team notified; unknown files triaged.

---

### 3.2 P1 — Transcode

#### 3.2.1 Dry-Run First

```bat
utf8-forge convert ^
  --root D:\server ^
  --dry-run ^
  --report D:\backup\utf8-forge\dry.json ^
  --verbose
```

**Audit `dry.json`**:

| Check                                      | Expected                           |
|--------------------------------------------|------------------------------------|
| `action == "would_convert"`                | Only expected paths                |
| `dep/`, `_tools/`, `temp/`, `OutputFile*/` | **Absent**                         |
| `.png/.zip/.exe/.lib/.dll`                 | **Absent**                         |
| `.rc`                                      | Absent (unless `--include-rc`)     |
| Any file > 200 MiB                         | None (or adjust `--max-file-size`) |

**Stop immediately if anything unexpected appears.** Adjust `--exclude` / `--ext` and re-run dry-run.

#### 3.2.2 Real Conversion

```bat
utf8-forge convert ^
  --root D:\server ^
  --backup-dir D:\backup\utf8-forge\orig ^
  --report D:\backup\utf8-forge\apply.json ^
  --verbose
```

**Guarantees**:

- **Atomic writes** — power loss never corrupts source.
- **Only changed files written** — `already_utf8` untouched (no mtime churn).
- **Per-file error isolation** — one failure does not abort the batch.
- **Replayable backup** — original bytes preserved under mirrored path.

**Output**:

```text
[utf8-forge] mode=convert target=utf-8 root=D:\server
[utf8-forge] scanned=1247  candidates=1142
[utf8-forge] already_utf8=1035  converted=171  unknown=5  errors=0
[utf8-forge] duration=8.3s  report=D:\backup\utf8-forge\apply.json
```

#### 3.2.3 Immediate Verification

```bat
utf8-forge verify --root D:\server --report D:\backup\utf8-forge\verify1.json
```

- **Exit 0** + `summary.failed == 0` → pass.
- **Exit 3** → list contains unexpected files; investigate each.

> If `unknown > 0`, `verify` will return 3 because those files remain non-UTF-8. This is **expected**; triage per §6.

#### 3.2.4 Idempotence Self-Check

```bat
utf8-forge convert --root D:\server --report D:\backup\utf8-forge\apply2.json
```

**Expected**: `converted=0`. If not, stop — check `apply.json` vs `apply2.json` diff for concurrent edits.

> A leftover `converted=1` from a double-BOM UTF-16 file is expected (two-pass
> convergence); confirm via the apply.json/apply2.json diff before stopping.

The `diff` subcommand makes the comparison mechanical:

```bat
utf8-forge diff --before D:\backup\utf8-forge\apply.json --after D:\backup\utf8-forge\apply2.json --fail-on-change
```

---

### 3.3 P1 Exit Criteria

- [ ] First `convert` `converted` matches dry-run prediction.
- [ ] Second `convert` `converted == 0` (idempotent).
- [ ] `verify` exit 0 (or only known unknowns).
- [ ] SVN diff contains only expected files (§4).
- [ ] Backup directory complete and replayable.

---

### 3.4 P2 — Global `/utf-8` (Tool Does Not Participate)

```text
Per Playbook §2.2:
1. Add /utf-8 to every production .vcxproj ClCompile section.
2. Remove 3 per-file /utf-8 exceptions in GameServer.vcxproj.
3. Clean Rebuild.
```

**Precondition**: `verify` passed (source 100% UTF-8). Otherwise **do not** enable `/utf-8` — it will cause
double-mojibake.

> Optional: if `.vcxproj/.sln` should also be unified, use
> `utf8-forge convert --root D:\server --include-msbuild`.
> VS2019 may re-save with BOM — harmless.

---

### 3.5 P3 — Rebuild & Smoke (Playbook §6.3)

```text
Follow Playbook §7 checklist. This tool provides only the encoding evidence:
- [x] verify passes
- [ ] Clean Rebuild passes
- [ ] Chinese logs readable
- [ ] Chinese protocol/DB regression
- [ ] genbin Chinese export correct
```

---

### 3.6 P4 — CI / Pre-Commit Hardening

```bat
:: pre-commit hook (Windows batch sketch)
utf8-forge verify --root %CD% --ext %CHANGED_EXTS% || exit /b 1
```

```yaml
# Generic CI step
- name: Verify encoding
  run: utf8-forge verify --root . --report encoding-report.json
```

Also commit `.editorconfig`:

```ini
root = true
[*.{c,cc,cpp,h,hpp,inl,proto}]
charset = utf-8
end_of_line = crlf
insert_final_newline = true
```

---

## 4. SVN Collaboration & Diff Audit

### 4.1 Before

```bat
cd D:\server
svn status > D:\backup\utf8-forge\before-status.txt
```

### 4.2 After

```bat
svn status > D:\backup\utf8-forge\after-status.txt
svn diff --summarize > D:\backup\utf8-forge\summary.txt
```

**Must satisfy**:

- Every path in `summary.txt` ∈ `files[].path` from `apply.json`.
- No `dep/`, `_tools/`, `temp/`, `OutputFile*/`.
- No binary extensions.
- Commit per logical directory in a single window to avoid mixed GBK/UTF-8 intermediate states.

### 4.3 Diff Review

- Every line changing is **normal** (encoding change → byte change → SVN sees whole-line diff).
- Focus on: Chinese comments readable, narrow-string literals consistent.
- Spot-check (Playbook §6.1 P1 list):
    - `GameServer/Channel.cpp`
    - `GameServer/GameProcess.cpp`
    - `Share/common/String.h`
    - `GameServer/room/FieldData.cpp` (original UTF-16)
    - `genbin/genbin.cpp`

---

## 5. Recipes (v0.1)

### 5.1 C/C++ + proto/txt Only

```bat
utf8-forge convert ^
  --root D:\server ^
  --ext h,hpp,c,cpp,cc,inl,proto,txt ^
  --backup-dir D:\backup\utf8-forge\orig
```

### 5.2 Include MSBuild Files

```bat
utf8-forge convert ^
  --root D:\server ^
  --include-msbuild ^
  --backup-dir D:\backup\utf8-forge\orig
```

### 5.3 Extra Exclusions

```bat
utf8-forge scan ^
  --root D:\server ^
  --exclude "GameServer201,Test_Client" ^
  --exclude-glob "**/Generated/**"
```

### 5.4 Scan Only (for Review)

```bat
utf8-forge scan --root D:\server --report scan.json -q
```

### 5.5 Remote Download & Transcode

```bat
:: Fetch a GBK-encoded file from an HTTP endpoint and save as UTF-8
utf8-forge convert ^
  --url https://example.com/data/legacy_server.cpp ^
  --output D:\server\external\legacy_server.cpp ^
  --target utf-8 ^
  --report D:\backup\utf8-forge\fetch-report.json
```

**Notes**:

- `--output` omitted → filename derived from the URL's last path segment: an
  extension is kept verbatim (`…/a.cpp` → `a.cpp`); `.utf8` is appended only
  when the segment has none (`…/foo` → `foo.utf8`); with no usable segment the
  fallback is `download.utf8`. The derived path is recorded in the report
  relative to the **current working directory**.
- `--max-file-size` applies to the remote download too: an oversized payload
  fails with exit 1 (`summary.too_large=1`, `files[0].action="skipped_too_large"`)
  and nothing is written.
- `--backup-dir` cannot be combined with `--url` — fatal (exit 2).
- HTTP failures (non-2xx, TLS, timeout) → `summary.fetch_error=1`, exit 1.
- The fetch report is `mode: "fetch"` with `url` set; `root` is omitted; in `options`,
  `exclude_ext`/`exclude_globs`/`backup_dir` are omitted when empty (empty
  `extensions`/`exclude_dirs` remain).
- `--dry-run` works with `--url`: fetches and reports, but does not write.
- `--config` is ignored in fetch mode; the size cap is enforced during
  download (an oversized payload aborts mid-stream).

### 5.6 Reverse: UTF-8 → GB18030 (Legacy Scripts Only)

```bat
utf8-forge convert ^
  --root D:\server\legacy\scripts ^
  --target gb18030 ^
  --ext bat,cmd ^
  --backup-dir D:\backup\utf8-forge\orig-gbk
```

> ⚠️ Do not use unless a legacy external dependency requires GBK.

---

## 6. Unknown / DecodeError Handling

| Report Category | Meaning                         | Action                                            |
|-----------------|---------------------------------|---------------------------------------------------|
| `unknown`       | Not UTF-8 / UTF-16 / UTF-32 / GB18030 | Inspect hex; often a binary with a text extension |
| `decode_error`  | Legal encoding, truncated bytes | Hex editor; ask original author                   |
| `too_large`     | > `--max-file-size`             | Usually skip; raise limit only if necessary       |
| `io_error`      | Permission / locked file        | Close the locking process or fix ACLs             |
| `fetch_error`   | `--url` network/TLS failure     | Check URL, proxy, timeout                         |

**Principles**:

1. **Never guess.** The tool does not guess; humans must not either.
2. Unknown files are preserved verbatim and committed **separately**.
3. If a file is truly a BOM-less UTF-16 (rare), add BOM manually and re-run `convert`.
4. If a binary was misnamed as `.cpp`, exclude it from the whitelist.

---

## 7. Rollback

### 7.1 Single File (from backup)

```bat
copy /Y D:\backup\utf8-forge\orig\GameServer\Channel.cpp D:\server\GameServer\Channel.cpp
```

> Backups mirror the tree **relative to `--root`**; only rewritten files get a
> backup (unchanged/skipped files have none).

### 7.2 Full SVN Rollback

```bat
cd D:\server
svn revert -R .
svn status             :: must be empty
```

> If already committed: `svn merge -c -<rev> .`

### 7.3 Post-Rollback

- Re-run `utf8-forge scan` to confirm baseline.
- Notify the team; pause P2.

---

## 8. Playbook Mapping (v0.1)

| Playbook Phase         | This Manual              | Tool Command                       |
|------------------------|--------------------------|------------------------------------|
| P0 Inventory & Freeze  | §3.1                     | `scan --report`                    |
| P1 Source Transcode    | §3.2                     | `convert [--dry-run] --backup-dir` |
| P1 Exit Criteria       | §3.3                     | `verify` + idempotence check (a single `fix` run can replace convert + verify) |
| P2 Global `/utf-8`     | §3.4 (tool not involved) | —                                  |
| P3 Rebuild & Smoke     | §3.5                     | `verify` as evidence               |
| P4 CI Hardening        | §3.6                     | `verify` in pre-commit/CI          |
| Optional Remote Source | §5.5                     | `convert --url ... --output ...`   |

---

## 9. Risks & Mitigations (Playbook §2.4)

| Risk                                 | Mitigation                                        |
|--------------------------------------|---------------------------------------------------|
| Incomplete transcode before `/utf-8` | `verify` exit 0 as P2 gate                        |
| Console mojibake (narrow strings)    | P3 smoke scope; tool only guarantees source bytes |
| PCH / incremental stale objects      | Clean Rebuild in P3                               |
| Accidental `dep/` modification       | Default blacklist + dry-run audit                 |
| Large SVN diff / partial commit      | Single window + `svn diff --summarize`            |
| Runtime protocol/DB regression       | Boundaries already UTF-8; regression tests cover  |
| Remote fetch hangs                   | `--max-file-size` + 120 s timeout                 |

---

## 10. Acceptance Checklist (Printable)

### Before

- [ ] `svn status` empty
- [ ] Transcode window announced
- [ ] `scan.json` archived and reviewed
- [ ] `dry.json` reviewed; path list clean
- [ ] Backup directory writable with sufficient space

### During

- [ ] First `convert` `converted` = dry-run prediction
- [ ] `apply.json` has no `error` entries
- [ ] `unknown` list generated and assigned

### After

- [ ] `verify` exit 0 (or only known unknowns)
- [ ] Second `convert` `converted == 0`
- [ ] `svn diff --summarize` only expected files
- [ ] Spot-check: 5 representative Chinese-comment files readable
- [ ] Backup complete and replayable

### P2 / P3 Hand-off

- [ ] `/utf-8` added per Playbook §2.2
- [ ] Per-file `/utf-8` exceptions removed
- [ ] Clean Rebuild passes
- [ ] Smoke tests pass (Playbook §7)

---

## 11. Troubleshooting

| Symptom                          | Check                                             | Fix                                           |
|----------------------------------|---------------------------------------------------|-----------------------------------------------|
| Files not converted              | `-v`; check whitelist, excluded dirs, hidden flag | Adjust `--ext` / `--include-hidden`           |
| `verify` exit 3                  | Read report for non-UTF-8 files                   | If `unknown`, §6; if missed, re-run `convert` |
| Chinese comments became mojibake | Likely double-transcode or wrong target           | `svn diff`; roll back single file             |
| Unexpected dirs in `svn diff`    | Dry-run skipped or ignored                        | Roll back; re-run dry-run                     |
| Very slow conversion             | Large files included                              | Lower `--max-file-size`; tune `--threads`     |
| Huge backup directory            | Backup uses original bytes                        | Keep ≥ 1 version cycle; prune later           |
| `--url` returns 403/404          | Server-side auth / path                           | Verify URL; use a direct download link        |
| `--url` memory spike             | Download streams to a temp file, capped per chunk — memory is bounded by `--max-file-size` | Raise limit cautiously or split file |

---

## 12. Appendix: JSON Report Schema (v0.1)

Optional keys are **omitted when empty**: `url` appears only with `--url`; a
fetch report omits `root`; `options.exclude_ext` / `exclude_globs` /
`backup_dir` are omitted when empty/none; `options.ignore_file` is omitted when
no ignore rules are active; `options.config_file` is omitted without `--config`;
`files[].detected` is omitted when the file was unreadable;
`verify_failed_files` is omitted when the fix gate is clean.

`options` reflects the post-config effective values (after merging
`--config`); `config_file` records the `--config` value verbatim.

```jsonc
{
  "tool": "utf8-forge",
  "version": "0.1.0",
  "mode": "convert",                     // scan | convert | verify | fetch | fix
  "root": "D:\\server",                  // omitted in fetch mode
  "url": "https://example.com/a.cpp",    // omitted without --url
  "started_at": "2026-...",              // captured when per-file processing begins
                                         // (tree modes: after traversal; fetch: at request start)
  "duration_ms": 8300,                   // per-file processing + report assembly
                                         // (traversal excluded in tree modes)
  "options": {
    "target": "utf-8",
    "dry_run": false,
    "extensions": ["h","hpp","c","cpp"],
    "exclude_dirs": ["dep","_tools"],
    "exclude_ext": ["png"],               // omitted when empty
    "exclude_globs": ["**/Generated/**"], // omitted when empty
    "backup_dir": "D:\\backup\\utf8-forge\\orig",  // omitted when none
    "config_file": "D:\\server\\forge.toml",       // omitted without --config
    "ignore_file": "D:\\server\\.utf8forgeignore"  // omitted when no rules active
  },
  "summary": { /* mode-specific; see the shapes below */ },
  "files": [
    {
      "path": "D:\\server\\GameServer\\Channel.cpp",
      "detected": "gb18030",
      "action": "converted",             // detected | passed | failed | would_convert | converted |
                                         // unchanged | skipped_unknown | skipped_too_large | error
      "bytes_before": 12873,
      "bytes_after": 15201,
      "duration_us": 912,
      "error": "io_error: file is write-protected (read-only); chmod +w to convert"
                                           // present only on error/failed records
    }
  ],
  "unknown_files": [],                   // detection unknown (any mode) or skipped_unknown in convert
  "error_files": [],                     // action error or verify failed — a file may appear in both; in fix
                                         // mode verify failures are listed only in verify_failed_files
  "verify_failed_files": []              // fix mode only: paths whose verify record is failed;
                                         // omitted when the gate is clean
}
```

`summary` is a per-mode block (`mode` determines the shape):

```jsonc
// scan — encoding distribution:
{ "total_files": 1247, "candidates": 1142, "utf8": 1035, "utf8_bom": 34,
  "utf16le": 2, "utf16be": 0, "utf32le": 0, "utf32be": 0, "gb18030": 171,
  "unknown": 5, "errors": 0, "too_large": 0 }
// errors = files that could not be read; buckets + errors + too_large == candidates

// verify — gate result:
{ "total_files": 1247, "candidates": 1142, "passed": 1142, "failed": 0 }

// convert / fetch — outcome counts:
{ "total_files": 1247, "candidates": 1142, "already_utf8": 1035, "converted": 171,
  "unknown": 5, "decode_errors": 0, "io_errors": 0, "too_large": 0, "fetch_error": 0 }

// fix — convert counters (no fetch_error) plus the verify gate on the same tree:
{ "total_files": 1247, "candidates": 1142, "already_utf8": 1035, "converted": 171,
  "unknown": 5, "decode_errors": 0, "io_errors": 0, "too_large": 0,
  "passed": 1142, "failed": 0 }
```

> `started_at` is captured when per-file processing begins; in tree modes
> neither it nor `duration_ms` includes the directory walk.

### 12.1 diff Output Document

`diff` does not produce a run report; it emits its own document type (to
`--report` or pretty-printed on stdout):

```jsonc
{
  "tool": "utf8-forge",
  "version": "0.1.0",
  "mode": "diff",
  "before": "apply.json",                // --before value verbatim
  "after": "apply2.json",                // --after value verbatim
  "before_mode": "convert",              // input report's mode; omitted when absent
  "after_mode": "convert",
  "summary": {
    "added": 0, "removed": 0, "action_changed": 1, "unchanged": 1141
  },
  "added":   [],                         // paths only in the after report
  "removed": [],                         // paths only in the before report
  "changed": [                           // action OR byte-count drift
    {
      "path": "D:\\server\\GameServer\\Channel.cpp",
      "before": { "path": "...", "action": "converted", "bytes_before": 12873, "bytes_after": 15201 },
      "after":  { "path": "...", "action": "unchanged",  "bytes_before": 15201, "bytes_after": 15201 }
    }
  ]
}
```

Comparison is per exact path string; both inputs must come from the same root
(differing `root` fields → stderr warning). Any report mode with `files[]` is a
valid input; verify outcomes are not compared. Duplicate paths within one
input resolve to the last occurrence.

Field semantics map 1:1 to `DESIGN.md §9`.
