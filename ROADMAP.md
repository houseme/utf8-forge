# ROADMAP.md — utf8-forge

> Source of truth for planned / done work. Produced from a three-expert
> adversarial review (code correctness + invariants, documentation vs code
> audit, destructive CLI QA) of the v0.1.0 implementation.
> Finding IDs: `C*` = code review, `D*` = docs audit, `T*` = adversarial QA.

---

## Status legend

`[ ]` open · `[~]` in progress · `[x]` done (with verification) · `[-]` rejected (with rationale)

---

## R1 — Correctness & safety (v0.1.x, this cycle)

| # | Priority | Item | Source | Acceptance |
|---|----------|------|--------|------------|
| R1.1 | [x] P0 | **Backup dir inside root destroys backups & idempotence.** `--backup-dir` under `--root` is walked on the next run, re-converts the backups, and nests `backup/backup/...`. Now a fatal (exit 2) pre-flight: canonicalized backup dir must be outside the canonicalized root. | C1 | `convert --root tree --backup-dir tree/backup` → exit 2, nothing touched; test added |
| R1.2 | [x] P0 | **Pub-API panic in `decode_strict`.** `&bytes[3..]` / `&bytes[2..]` slice without length check panics for BOM-detected encodings with truncated input. Now returns `DecodeError("truncated ... BOM")`. | C2 | `decode_strict(b"\xFF", Utf16Le)` returns `Err`, no panic; tests for all BOM arms |
| R1.3 | [x] P1 | **Read-only files silently overwritten.** POSIX rename defeats `chmod 444`; convert rewrote write-protected files with exit 0. Now a pre-write check: read-only file → `io_error` record ("file is write-protected"), skipped, original intact. | T1 | chmod 444 file → exit 1, content+mode unchanged, `error_files` contains it |
| R1.4 | [x] P1 | **Verbose lines hard-code `utf-8` as target.** `--target gb18030 -v` printed `utf-8 → utf-8`. Now prints the real target tag. | C3 | `convert --target utf-16le -v` shows `... → utf-16le` |
| R1.5 | [x] P1 | **Fetch mode rewrote byte-identical output and labeled it `converted`.** Now: if the destination exists with identical bytes → action `unchanged`, no write (dry-run reports the same). Missing/differing destination still writes as `converted`. | T2 | Second fetch of the same URL+output → `unchanged`, mtime preserved |
| R1.6 | [x] P1 | **Glob matcher hangs on star-heavy patterns.** Exponential backtracking (`************zzzzzzz` ran > 300 s). Fix: consecutive-star collapse at compile time (`**`/`***…` → `**`, `a**b` → `a*b`) + a step budget in the matchers (budget exhausted → no match). | C4 | Pathological pattern finishes instantly; existing glob semantics tests still pass |
| R1.7 | [x] P1 | **Report-write failure exits 2 *after* the tree was converted.** Now `--report` is pre-flighted (parent dirs created + writability probed via create/remove temp file) before any file is touched. | T5 + V9 (round 2) | Unwritable report dir (missing *or* existing read-only) → exit 2 with tree untouched |
| R1.8 | [x] P1 | **`--report` inside the tree pollutes later runs** (`.json` is whitelisted). Now the canonicalized report path is excluded from traversal in all modes. | C9 | `scan --report tree/scan.json` twice → identical counts |
| R1.9 | [x] P2 | **Double-BOM UTF-16 silently drops the content U+FEFF.** `encoding_rs` BOM-sniffs again after the manual strip. Now uses `decode_without_bom_handling`, so only the real BOM is stripped. | C5 | `FF FE FF FE 68 00 69 00` round-trips byte-identically to `utf-16le` |
| R1.10 | [x] P2 | **UTF-32 files (BOM `FF FE 00 00`) mangled as UTF-16LE with exit 0.** Now detected as `Unknown` (UTF-32 is out of scope per DESIGN §1.2 — never guessed). | C6 | UTF-32LE sample → `skipped_unknown`, file untouched |
| R1.11 | [x] P2 | **`OutputFile*` prefix rule also excluded *files*** named e.g. `outputfiles_config.md`. Name/prefix gate now applies to directories only; files are gated by the extension whitelist as before. | C7 | `OutputFile1_notes.txt` is scanned; `OutputFile1/` dir still pruned |
| R1.12 | [x] P2 | **`--ext .cpp` (leading dot) silently matched nothing.** `normalize_list` now strips leading dots. | C8/T4 | `scan --ext .txt,.md` works; unit test added |
| R1.13 | [x] P2 | **`scan` exited 0 despite per-file I/O errors**, contradicting USAGE §7. Now scan exits 1 when any file record is an `error` (consistent with convert). | C13 | chmod 000 file → scan exit 1 |
| R1.14 | [x] P2 | **Console mislabeled oversized URL payload as `error`** while the report says `skipped_too_large`. Console line now matches the report. | T3 | Same outcome text in stderr and JSON |
| R1.15 | [x] P2 | **`convert --url -v` printed no per-file line.** Fetch mode now routes through `print_records` like tree mode. | C12 | `-v` shows one record line in fetch mode |
| R1.16 | [x] P2 | **`started_at` stamped at report-build time** (post-processing). Now captured at run start and passed into the report. | C10 | `started_at + duration ≈ wall clock` on long runs |
| R1.17 | [x] P2 | Dead code removed: `FileRecord::new`, `FilterConfig::max_file_size` (never read by the walker). | C11 | `cargo build` warns nothing; tests updated |
| R1.18 | [x] P2 | **Feature normalization (subcommand surface):** `scan` / `verify` now structurally reject conversion-only flags (`--target`, `--dry-run`, `--backup-dir`) with a clap error (exit 2) instead of silently ignoring them. `ConvertArgs` owns `target/dry-run/backup-dir/url/output`. | C15 | `scan --target utf-8` → exit 2; docs updated |
| R1.19 | [x] P2 | `has_bom()` kept as deliberate public API (used by downstream callers/tests); documented. | C11 | — |
| R1.20 | [x] P2 | **Verification round 2:** writability probe for `--report` parent and `--backup-dir` (existing read-only dirs abort before conversion); backup placement check also enforced for existing dirs under `--dry-run`. | V9/R1.7 + V1 (round 2) | chmod 555 report dir → exit 2, tree untouched; dry-run + existing backup-in-root → exit 2 |
| R1.21 | [x] P1 | **Bare `--report r.json` bypasses the pre-flight** (`parent()` is `Some("")` → guard skips create+probe) → exit 2 *after* conversion. Treat an empty parent as `.`. | C21 (round 3) | cwd read-only + `--report r.json` → exit 2 with tree untouched |
| R1.22 | [x] P2 | **Fatal backup placement leaves created dirs in the tree** (`create_dir_all` runs before the placement check). Check placement from the deepest existing ancestor before creating anything. | C22 (round 3) | `--backup-dir <root>/sub/deep/bk` (nonexistent) → exit 2, no `sub/` left behind |
| R1.23 | [x] P2 | **Dry-run blesses nonexistent backup-in-root placement** (check only ran when `dir.exists()`). The placement check becomes existence-independent (projected path). | C23 (round 3) | dry-run + nonexistent backup-in-root → exit 2 |
| R1.24 | [x] P2 | **List normalization foot-guns:** `--exclude-ext "..h"` strips all dots (silent over-exclusion); `--ext ".cpp/"` silently matches nothing. Strip at most one leading dot; reject values that still start with a dot or contain `/` (fatal exit 2). | C24 (round 3) | `..h` → exit 2; `.cpp/` → exit 2; `.cpp` still works |
| R1.25 | [x] P2 | **Fetch generic errors print two stderr lines** (immediate `error <url>` + `print_records`' `error <out_path>`). Drop the immediate duplicate for the generic-error branch. | C25 (round 3) | HTTP-500 fetch → exactly one problem line |
| R1.26 | [x] P2 | **Scan summary buckets do not sum to candidates when a file errors** — add an `errors` counter to `ScanSummary` (buckets + errors + too_large == candidates) and document. | C26 (round 3) | chmod 000 file → `summary.errors == 1`, sums reconcile |
| R1.27 | [x] P2 | **`write_report` is truncate-then-write**, diverging from the crate's crash-safety doctrine. Reuse `writer::atomic_write`. | C27 (round 3) | report writes go through temp+rename |
| R1.28 | [x] P1 | **UTF-32BE BOM bytes inverted** (`FE FF 00 00` checked; the real UTF-32BE BOM is `00 00 FE FF`, WHATWG — a payload starting `FE FF 00 00` is UTF-16BE + leading NUL). Detector now follows WHATWG order; docs corrected. | W1 (final verification) | real UTF-32BE file → `utf-32be`, converted correctly; `FE FF 00 41 …` → `utf-16be` |
| R1.29 | [x] P2 | **Fix-mode report options showed defaults** (`cmd_fix` passed `None` to `options_block`). Now reports the real target/dry-run/backup-dir. | W2 (final verification) | `fix --target utf-16le --dry-run` → report `options.target = "utf-16le"`, `dry_run = true` |

## R2 — Documentation alignment (v0.1.x, this cycle)

| # | Priority | Item | Source |
|---|----------|------|--------|
| R2.1 | [x] P1 | Sync every "v0.2 / 0.2.0" reference to the actual 0.1.0 (DESIGN/AGENT/EXECUTION section labels, footers, schema example). | D1 |
| R2.2 | [x] P1 | DESIGN header `rust-version` 1.99 → 1.85. | D2 |
| R2.3 | [x] P1 | DESIGN §8.6 `derive_output_path` sketch rewritten to match §8.2 + code (extension kept / `.utf8` appended / `download.utf8`). | D3 |
| R2.4 | [x] P2 | DESIGN §2.2/§4.3: UTF-16 encoding documented as `str::encode_utf16()` byte serialization (encoding_rs' generic `encode()` maps UTF-16 targets back to UTF-8). | D4 |
| R2.5 | [x] P2 | DESIGN §12 + AGENT §5 module trees: add `lib.rs`; `main.rs` described as thin entry. DESIGN §10 gains the `[lib]` target. | D5/D10 |
| R2.6 | [x] P2 | `--exclude-glob` semantics documented (case-insensitive, root-relative components, `**` spans dirs, `\`→`/`, empty pattern fatal). | D6 |
| R2.7 | [x] P2 | `--include-hidden` documented as dot-prefixed files **and directories**. | D7 |
| R2.8 | [x] P2 | DESIGN §10 dependency pins refreshed (`encoding_rs 0.8.44`, `serde 1.0.229`). | D8 |
| R2.9 | [x] P2 | AGENT §3 exit-code cells gain the fatal `2`; §4 documents both flag-conflict rules + `--exclude-glob`/`-q`/`-v` rows. | D9/D11 |
| R2.10 | [x] P1 | EXECUTION §7.1 rollback path fixed (`orig\GameServer\...`, mirrored relative to `--root`; only rewritten files have backups). | D12 |
| R2.11 | [x] P1 | EXECUTION §3.2.3 `all_utf8=true` → `summary.failed == 0`. | D13 |
| R2.12 | [x] P1 | EXECUTION §5.5 derived-filename note corrected. | D14 |
| R2.13 | [x] P2 | EXECUTION §12 schema refreshed: optional-key omission, `error` field, `options.exclude_ext/exclude_globs`, full action list, per-mode summaries (scan/verify/fetch), `unknown_files`/`error_files` semantics. | D15/D16/D17/D19 |
| R2.14 | [x] P2 | Fetch-mode facts documented (`--backup-dir` fatal, too-large → exit 1, report shape, CWD-relative derived output). | D18 |
| R2.15 | [x] P2 | USAGE exit codes completed (fatal 2 everywhere; fetch `fetch_error`/too-large → 1; scan io_error → 1). | D20 |
| R2.16 | [x] P2 | USAGE §6: full action list, mode-specific summaries, `already_utf8` = "already at the target encoding". | D21 |
| R2.17 | [x] P2 | USAGE §3 notes: `--ext` replaces the whitelist (opt-in flags inert with it), `--threads 0` clamps to 1, `--url` ignores `--root`, leading-dot `--ext` stripped. | D22 |
| R2.18 | [x] P2 | USAGE.zh-CN parity: remove leftover meta text; keep strict 1:1 structure with USAGE.md. | D23 |
| R2.19 | [x] P2 | READMEs' "Remote download" section gains the sharp edges (backup conflict, too-large exit 1, derived-name rules). | D24 |
| R2.20 | [x] P1 | Document the new R1 behaviors: read-only refusal, UTF-32 → unknown, backup-inside-root fatal, report pre-flight + auto-exclusion, scan exit 1 on io errors, per-subcommand flag surface. | new |
| R2.21 | [x] P2 | **Docs verification round 2 (V1–V9):** backup-inside-root/dry-run qualifier, fetch `options` omission wording, AGENT §7.5 `Commands`+`run()` dispatch, DESIGN title → English, zh trailing blockquote removed for strict 1:1 parity, `changed = 0` → `converted == 0`, §12 `error` field comment, §8.2 "(no extension)". | V1–V8 (round 2) | — |
| R2.22 | [x] P2 | **Idempotence overclaim for double-BOM inputs (round 3):** `FF FE FF FE …` converges in TWO passes (pass 1 writes the content U+FEFF as a UTF-8 BOM, pass 2 strips it). Qualify DESIGN §4.4/§11, AGENT §6, README EN+zh, EXECUTION §3.2.4. | D25 (round 3) | Docs state the two-pass exception |
| R2.23 | [x] P2 | **EXECUTION §12 timing wording (round 3):** in tree modes `started_at`/`duration_ms` exclude the traversal — reword the schema comments and the closing note. | D26 (round 3) | §12 states "captured when per-file processing begins (traversal excluded in tree modes)" |
| R2.24 | [x] P2 | **Final verification round (7 fixes):** DESIGN §3 UTF-32BE snippet corrected; `--ignore-file`+`--no-ignore-file` added to all conflict lists; `error_files` fix-mode qualifier (EN/zh/§12); §12 `duration_ms` "per-file processing only"; zh "4 字节" typo; lib.rs module doc exit-3 covers fix; R1.25 single-line fetch errors restored. | final docs audit | all conflicts/error-files/timing claims match the binary |

## R3 — Rejected / accepted-as-is (rationale recorded)

| # | Item | Rationale |
|---|------|-----------|
| R3.1 | `[-]` Reject `--target` on `scan`/`verify` via clap conflict | Superseded by R1.18: the flags are now structurally absent from those subcommands. |
| R3.2 | `[-]` Auto-exclude `--backup-dir` from traversal (instead of erroring) | Silent magic hides operator mistakes; a fatal error makes the misconfiguration visible (EXECUTION §3.2.2 recommends keeping backups outside the tree anyway). |
| R3.3 | `[-]` Walk-level size enforcement via `FilterConfig` | Sizes are enforced at read time in the per-file processors; keeping one enforcement point avoids double metadata syscalls. |
| R3.4 | `[-]` `report`-write failure after conversion → treat as success | Exit 2 stays: an audit run without its report is not a completed run. R1.7 pre-flight removes the practical harm. |
| R3.5 | `[-]` Streaming remote download | Source files are small; `bytes()` + cap is sufficient (DESIGN §8.3 defers streaming). Revisit if >200 MiB fetches become real. — Superseded by R5.4 (streaming fetch implemented this cycle) at user request. |

## R4 — Future (not scheduled)

- Bazel/MSBuild project-graph-aware scoping.

## R5 — Feature wave (this cycle, per the approved feature spec)

All six former R4 items are now scheduled and specified (spec v1: CLI surface,
precedence, data structures, edge cases, report deltas, exit codes, acceptance
tests per feature). Implementation status tracked here.

| # | Priority | Item | Key decisions (spec v1) | Acceptance |
|---|----------|------|-------------------------|------------|
| R5.1 | [x] P1 | **Ignore file**: `--ignore-file <PATH>`, auto-discovery of `<root>/.utf8forgeignore`, `--no-ignore-file`. Gitignore-subset syntax: `#` comments, trailing `/` dir-only, leading `/` anchored, no-slash patterns match basename at any depth, `!` negation last-match-wins, no escapes. Independent exclusion layer (AND with existing gates). | Explicit missing file → fatal exit 2; auto missing → silent; auto unreadable → warning; case-insensitive; `Glob::compile_raw` (backslash is literal); ignore path added to walker skip set | Spec §1.8 tests 1–10 |
| R5.2 | [x] P1 | **Config file**: `--config <PATH>` TOML, precedence CLI > config > defaults; keys `[filter] extensions/exclude_dirs/exclude_ext/exclude_globs/include_*/include_hidden`, `[convert] target/backup_dir`, `[options] max_file_size/threads`; strict unknown keys → exit 2; lists replace across layers; conflicts checked post-merge; fetch mode ignores `--config`. | `--target`/`--max-file-size` become Option to detect "flag passed"; missing/malformed → exit 2 with TOML line/col; `toml 0.9` (parse+serde) dep | Spec §2.7 tests 1–10 |
| R5.3 | [x] P1 | **UTF-32 read support**: `Utf32Le`/`Utf32Be` detection (4-byte BOM checked before UTF-16; WHATWG-style precedence), strict decode (misaligned tail / surrogate / >0x10FFFF → decode_error), no UTF-32 encode target, verify fails utf-32 files, scan summary gains `utf32le`/`utf32be`. | Supersedes R1.10's `Unknown` verdict (that was the interim guard while UTF-32 was a non-goal) | Spec §3.8 tests 1–10 |
| R5.4 | [x] P2 | **Streaming fetch**: chunk loop to a unique temp file, per-chunk cap enforcement (mid-download abort, temp cleaned), final read-back; behavior byte-identical for payloads ≤ cap; memory bounded by cap. | No fsync on temp; `unique_suffix` shared with writer | Spec §4.5 tests 1–8 |
| R5.5 | [x] P1 | **`fix` subcommand**: one traversal, convert pass then verify pass on the same file list; report mode "fix", flat `FixSummary` (convert counters + `passed`/`failed`), `verify_failed_files` list (skip-if-empty); exit 3 gate-wins, else 1 convert issues, else 0; dry-run keeps verify live. | `--url`/`--output` structurally absent (clap exit 2); too-large → verify `failed` → exit 3 | Spec §5.7 tests 1–10 |
| R5.6 | [x] P2 | **`diff` subcommand**: `diff --before A.json --after B.json [--report P] [--fail-on-change]`; per-path added/removed/changed (action or byte-count drift); roots-differ → warning; malformed input → exit 2; default exit 0, `--fail-on-change` → 1 on drift. | Exact path-string compare (same-root requirement documented); duplicate paths last-wins; new `DiffReport` document type | Spec §6.8 tests 1–10 |

---

*ROADMAP v0.1 — R1/R2 completed and verified through adversarial rounds 1–3 (C1–C27, D1–D26, V1–V9, T1–T5, W1–W2 all fixed). The R5 feature wave (six features) went specify → implement → docs → two-round verification. R3 records deliberate rejections; R4 holds the remaining unscheduled item.*
