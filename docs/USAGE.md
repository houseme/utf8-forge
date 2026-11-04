# docs/USAGE.md (English)

# utf8-forge — Full Usage Guide

> Complete CLI reference, recipes, and operational notes.
> For architecture and invariants see [DESIGN.md](../DESIGN.md).
> For the `D:\server` rollout see [EXECUTION.md](../EXECUTION.md).

---

## 1. Synopsis

```
utf8-forge <COMMAND> [OPTIONS]
```

Commands: `scan`, `convert`, `verify`, `fix`, `diff`.

---

## 2. Commands

### 2.1 `scan`

Detect encodings only. Writes nothing. Emits a summary + optional JSON report.

```bash
utf8-forge scan --root D:\server --report scan.json
```

Exit codes:

- `0` — all files classified.
- `1` — `unknown > 0`, or any per-file I/O error.
- `2` — fatal (missing root, bad args).

### 2.2 `convert`

Detect + transcode + atomic write.

```bash
# Default: → UTF-8 without BOM
utf8-forge convert --root D:\server

# Dry-run
utf8-forge convert --root D:\server --dry-run --report dry.json

# With backup
utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge

# Custom target
utf8-forge convert --root D:\server --target gb18030

# Remote file
utf8-forge convert --url https://example.com/a.cpp --output ./a.cpp
```

Exit codes:

- `0` — success (no unexpected errors).
- `1` — some files `unknown` / `decode_error` / `io_error`; with `--url`, also
  `fetch_error` (HTTP/TLS/timeout failure) and an oversized payload.
- `2` — fatal (bad args, missing root, flag conflicts, `--backup-dir` inside
  `--root` or combined with `--url`).

### 2.3 `verify`

Assert every whitelisted file is UTF-8 without BOM.

```bash
utf8-forge verify --root D:\server --report verify.json
```

Exit codes:

- `0` — all UTF-8 no BOM.
- `3` — one or more files not UTF-8 no BOM. Oversized or unreadable files count
  as failed (fail-closed).
- `2` — fatal (missing root, bad args).

### 2.4 `fix`

Convert + verify in ONE pass over one traversal: every candidate runs the
convert pipeline, then the verify gate over the same file list — no re-walk.

```bash
utf8-forge fix --root D:\server --report fix.json

# Preview: the convert pass writes nothing, but the verify pass still runs
# read-only — a dirty tree previews as exit 3 (the current end state).
utf8-forge fix --root D:\server --dry-run
```

Flags: the full tree flag set plus `--target` / `--dry-run` / `--backup-dir`.
No `--url` / `--output` (remote fetch stays convert-only).

Exit codes:

- `0` — conversion ran and every file passed the verify gate.
- `3` — the gate wins: any file fails the verify pass (not UTF-8 at the target,
  `unknown`, oversized, unreadable — all fail closed). The report carries
  `verify_failed_files`.
- `1` — convert-side issues (`unknown` / `decode_error` / `io_error`) with no
  verify failure.
- `2` — fatal (bad args, missing root, `--backup-dir` inside `--root`).

### 2.5 `diff`

Compare two utf8-forge JSON reports per path. Any report mode with `files[]`
works (`fix` reports expose their convert records; verify outcomes are not
compared).

```bash
utf8-forge diff --before apply.json --after apply2.json

# CI gate: non-empty diff → exit 1
utf8-forge diff --before a.json --after b.json --fail-on-change

# Write the diff JSON to a file (stdout then carries the summary line only)
utf8-forge diff --before a.json --after b.json --report diff.json
```

Paths are compared as exact strings — both reports must come from the same
root (differing `root` fields print a warning and continue).

Exit codes:

- `0` — comparison finished (differences do NOT change the exit code).
- `1` — `--fail-on-change` and the diff is non-empty (added + removed +
  changed > 0).
- `2` — missing or malformed input report.

---

## 3. Options

| Flag                       | Default         | Notes                                                            |
|----------------------------|-----------------|------------------------------------------------------------------|
| `-r, --root <PATH>`        | `.`             | Recursive root                                                   |
| `-e, --ext <LIST>`         | see DESIGN §5.1 | **Replaces** default whitelist; leading dots stripped (`.cpp` == `cpp`) |
| `-x, --exclude <LIST>`     | see DESIGN §5.2 | Appended to default excludes                                     |
| `-X, --exclude-ext <LIST>` | empty           | Appended                                                         |
| `--exclude-glob <GLOB>`    | empty           | Repeatable. Case-insensitive glob over root-relative path components; `/`-separated, `\` accepted; `**` spans components, `*`/`?` match within one; empty pattern → exit 2 |
| `--include-rc`             | false           | Adds `.rc/.rc2/.dlg`                                             |
| `--include-msbuild`        | false           | Adds `.sln/.vcxproj/...`                                         |
| `--include-hidden`         | false           | Processes dot-prefixed files and directories                     |
| `--config <PATH>`          | none            | TOML config supplying defaults; CLI flags override config values; missing/malformed file or unknown keys → exit 2 (ignored by fetch mode) |
| `--ignore-file <PATH>`     | auto            | Gitignore-style exclusions; fatal (exit 2) when the explicit file is missing; replaces auto-discovery of `<root>/.utf8forgeignore` |
| `--no-ignore-file`         | false           | Disables ignore-file handling entirely, including auto-discovery |
| `-t, --target <ENC>`       | `utf-8`         | `utf-8-bom`, `gb18030`, `utf-16le`, `utf-16be` (convert, fix)    |
| `--dry-run`                | false           | No writes (convert, fix)                                         |
| `--backup-dir <PATH>`      | none            | Backup before overwrite (convert, fix)                           |
| `--max-file-size <BYTES>`  | 200 MiB         | Skip oversized                                                   |
| `--threads <N>`            | CPU count       | Rayon pool; `0` clamps to `1`                                    |
| `--report <PATH>`          | none            | JSON report; parent dirs are created, the report path is excluded from scanning |
| `--url <URL>`              | none            | **Remote fetch (convert only)**; ignores `--root`                |
| `--output <PATH>`          | derived         | Destination for `--url`                                          |
| `-q, --quiet`              | false           | Errors only                                                      |
| `-v, --verbose`            | false           | Per-file detail                                                  |

**Per-command flag surface**: `--target`, `--dry-run` and `--backup-dir` exist
on `convert` and `fix`. `scan` and `verify` reject them (clap error, exit 2).
`--url` / `--output` are convert-only. `diff` accepts only `--before`,
`--after`, `--report` and `--fail-on-change`.

Conflicts (fatal, exit 2):

- `--include-rc` + `--exclude-ext rc|rc2|dlg`
- `--include-msbuild` + `--exclude-ext sln|vcxproj|vcproj|props|targets|filters|user`
- `--ignore-file` + `--no-ignore-file`

**Notes**:

- (a) `-e/--ext` **replaces** the default whitelist; the opt-in flags
  (`--include-rc` / `--include-msbuild`) only extend the default set and have
  no effect together with `-e`.
- (b) `--backup-dir` cannot be combined with `--url` (fatal, exit 2).
- (c) `--backup-dir` inside `--root` is rejected (fatal, exit 2); an existing
  directory is placement-checked even under `--dry-run`.
- (d) `--threads 0` clamps to `1`.
- (e) `--url` ignores `--root`.
- (f) `--report` parent dirs are created; the report path itself is excluded
  from scanning.
- (g) read-only files are refused with an io error, never overwritten.
- (h) ignore file: an explicit `--ignore-file` that is missing is fatal
  (exit 2); auto-discovery of `<root>/.utf8forgeignore` (flag absent) is
  silent when missing and warns + continues when unreadable;
  `--no-ignore-file` disables both. The ignore file is an independent
  exclusion layer: `!` negation lifts only the ignore layer, never the
  whitelist / `--exclude` / `--exclude-glob` layers. The ignore file itself
  is never scanned.
- (i) config file: precedence CLI > config > built-in defaults; list values
  REPLACE across layers (never concatenate). `--target` and `--max-file-size`
  apply their CLI value only when the flag is passed, otherwise the config
  value, otherwise the default. Fetch mode ignores `--config`.

---

## 4. Encoding Semantics

### 4.1 Detection order

UTF-8 BOM → UTF-32 LE BOM → UTF-32 BE BOM → UTF-16 LE BOM → UTF-16 BE BOM →
strict UTF-8 → strict GB18030 → `Unknown`.

The UTF-32 BOMs (`FF FE 00 00` LE, `00 00 FE FF` BE) are checked before the
2-byte UTF-16 BOMs (WHATWG-style longest-prefix precedence). Documented
ambiguity: a UTF-16LE file whose first character is U+0000 shares those
leading bytes; it is classified `utf-32le` and then fails strict UTF-32 decode
loudly (`decode_error`, never silent mojibake).

UTF-32 is read-only: strict decode rejects surrogates, scalar values above
`0x10FFFF`, and misaligned tails (payload length not a multiple of 4 after the
BOM). There is **no** UTF-32 write target (`--target utf-32le` → exit 2), and
`verify` fails UTF-32 files (they are not UTF-8).

### 4.2 BOM policy

- Default output has **no BOM**.
- `--target utf-8-bom` adds `EF BB BF`.
- `--target utf-16le/be` emits `FF FE` / `FE FF` manually (encoding_rs does not).
- Input BOM is stripped during decode.
- Only the real leading BOM is stripped; a second BOM is content (U+FEFF).

### 4.3 Line endings

Never normalized. CRLF stays CRLF; LF stays LF.

---

## 5. Recipes

### 5.1 C/C++ only

```bash
utf8-forge convert --root D:\server --ext h,hpp,c,cpp,cc,inl
```

### 5.2 Include MSBuild

```bash
utf8-forge convert --root D:\server --include-msbuild
```

### 5.3 Exclude a module

```bash
utf8-forge scan --root D:\server --exclude "GameServer201,Test_Client"
```

### 5.4 Glob exclusion

```bash
utf8-forge convert --root D:\server --exclude-glob "**/Generated/**"
```

### 5.5 Remote GBK → UTF-8

```bash
utf8-forge convert \
  --url https://example.com/legacy_server.cpp \
  --output D:\server\external\legacy_server.cpp \
  --target utf-8 \
  --report fetch-report.json
```

### 5.6 Reverse (legacy only)

```bash
utf8-forge convert --root D:\server\legacy\scripts --target gb18030 --ext bat,cmd
```

### 5.7 Ignore file

```bash
# One-off: gitignore-style exclusions from an explicit file (missing → exit 2)
utf8-forge convert --root D:\server --ignore-file D:\server\my.ignore

# Standing setup: commit .utf8forgeignore at the tree root; it is picked up
# automatically (missing → silent, unreadable → warning + continue)
utf8-forge convert --root D:\server

# Turn every ignore source off
utf8-forge convert --root D:\server --no-ignore-file
```

Example `.utf8forgeignore` (one pattern per line; `#` only starts a full-line
comment):

```text
# comments and blank lines are skipped
Generated/
legacy.cpp
/tools/tmp*
build/**/auto*.h
!keep.gen
```

- `Generated/` — trailing `/` → directory only.
- `legacy.cpp` — no slash → matches the basename at any depth.
- `/tools/tmp*` — leading `/` (or any inner `/`) → anchored to the
  root-relative path.
- `build/**/auto*.h` — `**` spans components; `*` / `?` match within one.
- `!keep.gen` — `!` negates; the LAST matching pattern wins.

Syntax is a gitignore subset: case-insensitive, no escapes (a backslash is a
literal character), and — git parity — a file cannot be re-included when its
parent directory is excluded. Negation lifts only the ignore layer; the
whitelist / `--exclude` / `--exclude-glob` layers still apply.

### 5.8 Config file

```bash
# Defaults from TOML; CLI flags override config values when passed
utf8-forge convert --root D:\server --config D:\server\forge.toml
```

Example `forge.toml`:

```toml
[filter]
extensions = ["h", "hpp", "c", "cpp", "cc"]
exclude_dirs = ["legacy"]
exclude_ext = ["png"]
exclude_globs = ["**/Generated/**"]
include_rc = false
include_msbuild = false
include_hidden = false

[convert]
target = "utf-8"
backup_dir = "D:\\backup\\utf8-forge\\orig"

[options]
max_file_size = 209715200
threads = 8
```

Precedence is CLI > config > defaults. List values REPLACE across layers
(never concatenate). Missing, malformed, or unknown-key files are fatal
(exit 2) — messages carry the TOML line/column. `root`, `report`, `dry-run`,
`url`, `output`, and quiet/verbose are deliberately absent from the schema;
fetch mode ignores `--config`.

---

## 6. Reports

JSON report schema is documented in `EXECUTION.md §12`.

Key fields:

- `summary.total_files` — every file visited.
- `summary.candidates` — files matching whitelist.
- `files[].action` — full list: `detected` | `passed` | `failed` |
  `would_convert` | `converted` | `unchanged` | `skipped_unknown` |
  `skipped_too_large` | `error`.
- `unknown_files` — detection `unknown` (any mode) or `skipped_unknown` in
  convert; `error_files` — action `error` or verify `failed` (a file may appear
  in both; in fix mode verify failures are listed only in
  `verify_failed_files`).
- `verify_failed_files` — fix mode only: paths whose verify record is `failed`;
  omitted when the gate is clean.
- `options.ignore_file` — ignore rules in effect (explicit path verbatim, or
  the auto-discovered `<root>/.utf8forgeignore`); omitted when no rules are
  active.
- `options.config_file` — the `--config` value verbatim; omitted when absent.

The `summary` block is mode-specific:

- `scan` — encoding distribution: `utf8` / `utf8_bom` / `utf16le` / `utf16be` /
  `utf32le` / `utf32be` / `gb18030` / `unknown` / `errors` / `too_large`
  (plus `total_files` / `candidates`; `errors` counts unreadable files, so
  buckets + errors + too_large == candidates).
- `verify` — gate result: `passed` / `failed`.
- `convert` / `fetch` — outcome counts: `already_utf8` / `converted` /
  `unknown` / `decode_errors` / `io_errors` / `too_large` / `fetch_error`.
- `fix` — convert counters (as above, without `fetch_error`) plus `passed` /
  `failed` from the verify pass over the same tree.

`already_utf8` counts files already byte-identical at the **target** encoding:
with `--target gb18030`, untouched ASCII files count too.

---

## 7. Exit Codes

| Code | Meaning                                                                                     |
|------|---------------------------------------------------------------------------------------------|
| 0    | Success (`diff`: comparison finished — differences do not change the exit code unless `--fail-on-change`) |
| 1    | Non-fatal per-file issues (`unknown`, `decode_error`, `io_error`; convert: also `fetch_error`, oversized `--url` payload); `diff --fail-on-change`: the reports differ |
| 2    | Fatal in every command (bad args, missing root, flag conflicts, I/O setup failure); also emitted by clap; `diff`: missing/malformed input report |
| 3    | `verify` found non-UTF-8 whitelisted files; `fix`: any file fails the verify gate (oversized/unreadable count as failed — fail-closed; the gate wins over convert-side issues) |

---

## 8. CI Integration

```yaml
- name: Verify encoding
  run: utf8-forge verify --root . --report encoding-report.json
```

`verify` exit 3 fails the job — this is the intended gate.

---

## 9. Troubleshooting

See `AGENT.md §9` and `EXECUTION.md §11`.
