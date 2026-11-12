# ROADMAP.md — utf8-forge

> Source of truth for planned / done work. Produced from a three-expert
> adversarial review (code correctness + invariants, documentation vs code
> audit, destructive CLI QA) of the v0.1.0 implementation.
> Finding IDs: `C*` = code review, `D*` = docs audit, `T*` = adversarial QA.

---

## Release record

- **v0.2.0** released 2026-11-05: tag `v0.2.0` → Release workflow green →
  GitHub Release with 5 platform archives (+ sha256) → crates.io `0.2.0` live →
  docs.rs build OK. Artifact smoke-tested on aarch64-apple-darwin (checksum
  verified, rollout loop scan→convert→verify passed on a mixed tree).

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
| R1.6 | [x] P1 | **Glob matcher hangs on star-heavy patterns.** Exponential backtracking (`************zzzzzzz` ran > 300 s). Fix: consecutive-star collapse at compile time (`**`/`***…` → `**`, `a**b` → `a*b`) + a step budget in the matchers (budget exhausted → no match). | C4 | Pathological pattern finishes instantly; exact matching replaces the budget fallback in R7.3 |
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
| R1.29 | [x] P2 | **Fix-mode report options showed defaults** (`cmd_fix` passed `None` to `options_block`). Now reports the real target/dry-run/backup-dir. | W2 (final verification) | `fix --target utf-8 --dry-run` → report `options.target = "utf-8"`, `dry_run = true`; R6.4 rejects other targets |

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

## R4 — Deferred scope

- `[-]` **Bazel/MSBuild project-graph-aware scoping** — rejected for now
  (2026-11-06): no demand signal, large surface (project-file parsers), and
  the whitelist/ignore machinery already covers the real trees. Revisit only
  when a concrete repository needs it.

## R5 — Feature wave (this cycle, per the approved feature spec)

All six former R4 items are now scheduled and specified (spec v1: CLI surface,
precedence, data structures, edge cases, report deltas, exit codes, acceptance
tests per feature). Implementation status tracked here.

| # | Priority | Item | Key decisions (spec v1) | Acceptance |
|---|----------|------|-------------------------|------------|
| R5.1 | [x] P1 | **Ignore file**: `--ignore-file <PATH>`, auto-discovery of `<root>/.utf8forgeignore`, `--no-ignore-file`. Gitignore-subset syntax: `#` comments, trailing `/` dir-only, leading `/` anchored, no-slash patterns match basename at any depth, `!` negation last-match-wins, no escapes. Independent exclusion layer (AND with existing gates). | Explicit missing file → fatal exit 2; auto missing → silent; auto unreadable → warning; case-insensitive; `Glob::compile_raw` (backslash is literal); ignore path added to walker skip set | Spec §1.8 tests 1–10 |
| R5.2 | [x] P1 | **Config file**: `--config <PATH>` TOML, precedence CLI > config > defaults; keys `[filter] extensions/exclude_dirs/exclude_ext/exclude_globs/include_*/include_hidden`, `[convert] target/backup_dir`, `[options] max_file_size/threads`; strict unknown keys → exit 2; lists replace across layers; conflicts checked post-merge. Fetch mode rejects `--config` following R6.5. | `--target`/`--max-file-size` become Option to detect "flag passed"; missing/malformed → exit 2 with TOML line/col; `toml 1.2` (parse+serde) dep — spec v1 said 0.9, bumped to 1.2 during implementation | Spec §2.7 tests 1–10 |
| R5.3 | [x] P1 | **UTF-32 read support**: `Utf32Le`/`Utf32Be` detection (4-byte BOM checked before UTF-16; WHATWG-style precedence), strict decode (misaligned tail / surrogate / >0x10FFFF → decode_error), no UTF-32 encode target, verify fails utf-32 files, scan summary gains `utf32le`/`utf32be`. | Supersedes R1.10's `Unknown` verdict (that was the interim guard while UTF-32 was a non-goal) | Spec §3.8 tests 1–10 |
| R5.4 | [x] P2 | **Streaming fetch**: chunk loop to a unique temp file, per-chunk cap enforcement (mid-download abort, temp cleaned), final read-back; behavior byte-identical for payloads ≤ cap; memory bounded by cap. | No fsync on temp; `unique_suffix` shared with writer | Spec §4.5 tests 1–8 |
| R5.5 | [x] P1 | **`fix` subcommand**: one traversal, convert pass then verify pass on the same file list; report mode "fix", flat `FixSummary` (convert counters + `passed`/`failed`), `verify_failed_files` list (skip-if-empty); exit 3 gate-wins, else 1 convert issues, else 0; dry-run keeps verify live. | `--url`/`--output` structurally absent (clap exit 2); too-large → verify `failed` → exit 3 | Spec §5.7 tests 1–10 |
| R5.6 | [x] P2 | **`diff` subcommand**: `diff --before A.json --after B.json [--report P] [--fail-on-change]`; per-path added/removed/changed (action or byte-count drift); roots-differ → warning; malformed input → exit 2; default exit 0, `--fail-on-change` → 1 on drift. | Root-relative path comparison following R6.3; duplicate paths last-wins; new `DiffReport` document type | Spec §6.8 tests 1–10 |

## R6 — Review-fix wave (v0.2.1, four-expert adversarial review of v0.2.0)

Produced from a four-expert review (code correctness `A*`, docs-vs-code audit
`B*`, adversarial CLI QA `Q*`, dependency/API hygiene `D*`) of the shipped
v0.2.0. All findings were reproduced or code-confirmed before scheduling.
Verification (two adversarial rounds against the real binary + docs): every
functional claim measured TRUE; the follow-up round's findings were the
`-v` duplicate unknown line, the `encode_errors` summary counter (added),
and twelve docs items (W1–W12, incl. the settled-runs diff gate recipe) —
all fixed and re-verified.

| # | Priority | Item | Source | Acceptance |
|---|----------|------|--------|------------|
| R6.1 | [x] P0 | **UTF-16LE output beginning with U+0000 is silently re-read as UTF-32LE and mangled.** `--target utf-16le` on content whose first char is U+0000 writes `FF FE 00 00 …`; the next run detects `utf-32le` (WHATWG longest-BOM precedence), re-decodes the NUL-padded tail as astral scalars and rewrites the file with exit 0 — silent corruption, idempotence broken. Fix: `transcode_detected` refuses the encode (`ForgeError::EncodeError`, category `encode_error`) when a UTF-16LE target's decoded text begins with `\u{0}`; UTF-16BE is safe (`FE FF 00 00` re-reads as UTF-16BE). Pre-existing ambiguous files are decoded as UTF-32LE (documented; the byte streams are genuinely indistinguishable). | Q1/A1 | `convert --target utf-16le` on a NUL-leading file → exit 1, `encode_error` record, bytes untouched, rerun identical; utf-16be NUL-leading still converts and reruns `unchanged`; docs no longer claim the ambiguous case "always fails loudly" |
| R6.2 | [x] P1 | **Fetch-mode pre-flight & dedup:** (a) `cmd_convert_url` never runs the report pre-flight — an unwritable `--report` dir aborts exit 2 *after* the download was written (and a `--report` pointing at a directory yields a raw `Is a directory` error); (b) fetch decode errors print two stderr lines (immediate `error <url>` + `print_records`' `error <out_path>`); (c) convert/fix create `--backup-dir` before probing `--report` writability, so a report failure leaves an empty backup dir behind. | A2/A3/A4 | read-only report dir + `--url` → exit 2, output file not written; invalid-encoding payload → exactly one stderr line; `convert --backup-dir new --report <readonly>/r.json` → exit 2, backup dir does not exist |
| R6.3 | [x] P1 | **diff hardening:** (a) lenient deserialization accepts any JSON with/without `files[]` as a phantom-empty baseline (`{"schema":"wrong"}`, `[]` → exit 0 bogus diff); (b) record paths embed the root, so the same tree under two roots diffs as 100% churn. Fix: `files` becomes a required field (malformed input → exit 2); strip each side's `root` prefix (separator-normalized) before comparing; `DiffReport` gains `before_root`/`after_root` (skipped when absent); keep the roots-differ warning. | Q3/Q4 | `diff --before wrong.json` → exit 2; scan of `tree` vs identical `tree2` → added=0 removed=0 changed=0 + warning; same-root behavior unchanged |
| R6.4 | [x] P1 | **`fix` with a non-UTF-8 target can never exit 0.** The verify gate hard-asserts UTF-8 without BOM, so `fix --target gb18030` converts files then fails them, exit 3, tree left in a state its own gate rejects. Fix: fatal (exit 2) post-config-merge when the effective target is not `utf-8`. | Q5/A8 | `fix --target gb18030` → exit 2, tree untouched; config-supplied `[convert].target` equally rejected |
| R6.5 | [x] P2 | **Strict-input closures:** (a) `--output` silently ignored in tree-mode convert → clap `requires = "url"`; (b) `--config` silently ignored in fetch mode (its `[convert].target` would be dropped) → fatal exit 2 with `--url`; (c) invalid `[convert].target` accepted by scan/verify (validated only on the convert path) → validate in the shared tree merge; (d) `--ext ""` / all-blank list silently scans nothing → fatal empty-extension-list; (e) `--threads 0` clamp documented in `--help`. | D1/D4/Q7/A7/Q10/A12 | `convert -r t --output o` → exit 2; `convert --url U --config c.toml` → exit 2; `[convert] target="gbk"` + `scan` → exit 2 naming the key; `--ext ""` → exit 2; help shows the clamp |
| R6.6 | [x] P2 | **Console/report contract polish:** (a) `scan` exits 1 for unknown files but prints no stderr problem line (their action is `detected`, tag `unknown`, which `print_records` skips); (b) `diff` prints its summary line to stdout before the JSON, so bare `diff | jq` fails; (c) fatal io messages render the io error twice (`Display` prefix + `source()` chain); (d) `ScanSummary::errors` rustdoc states `buckets + errors + too_large == candidates`, but the identity is `buckets + errors == candidates`. | Q6/Q8/A9/A6/B14 | scan with a Latin-1 file → one `unknown <path>` stderr line; `diff … \| jq .` parses; report failure message contains "Permission denied" once; rustdoc formula corrected |
| R6.7 | [x] P2 | **Crash-safety & consistency polish:** (a) `atomic_write` never fsyncs the containing directory after rename (machine-crash window); (b) `verify_file` lacks the post-read size re-check that `scan_file`/`convert_file` have (a file grown between stat and read is verified despite exceeding the cap); (c) `derive_output_path` takes the literal last segment, so a trailing-slash URL (`…/dir/`) skips the doc-implied `dir` lookup; (d) `merge_convert_config` loads the same TOML twice. | A10/A5/D17/A11/D18 | dir fsync added best-effort; re-check mirrors the other processors; `derive_output_path("https://h/a/dir.txt/")` → `dir.txt`; one `config::load` per `--config` |
| R6.8 | [x] P2 | **Hygiene & API:** (a) tests/transcoder comments again reference plan IDs (`R5.x`, `Invariant #`, `Playbook`, `G3 acceptance`, `P0:`/`P1:`) — scrub to plain functional statements; (b) `IgnoreFile::is_empty` is dead pub API with a stale doc claim (and `len()` has only test callers); (c) `FixArgs::target_or_default` impl lives in lib.rs while the identical `ConvertArgs` impl lives in cli.rs; (d) 200 MiB default duplicated as a magic number in two places → `const DEFAULT_MAX_FILE_SIZE`; (e) `TargetEncoding`'s `Default` impl justifies itself via a derive that no longer exists. | D3/D5/D7/D8/D9 | grep for plan/finding IDs in src+tests returns nothing; no pub item without a non-test caller or documented purpose; single definition of the default |
| R6.9 | [x] P2 | **Tests & lockfile:** (a) config-precedence test asserts only `total_files == 1`, which passes even if precedence is inverted → assert the scanned path; (b) tag-stability/bom tests omit the UTF-32 variants (report buckets key on those strings); (c) no test covers the `--no-default-features` `--url` bail; (d) named gaps: `fix --backup-dir`, `diff --report` file branch, fetch `--output` parent creation, `IgnoreFile::load` disk path + unreadable-auto-ignore warning, scan `too_large` e2e, empty-file convert e2e, `utf-8-bom` second-pass idempotence; (e) stale unreferenced `ring 0.17.14` in Cargo.lock → prune via `cargo update`. | D2/D6/D15/D16/D10 | each gap has a named test; inverting the merge makes the precedence test fail; `cargo test --no-default-features` exercises the bail; `cargo build --locked` green; the stale `ring` entry turned out to be tolerated by `--locked` and only removable via a full lock refresh — accepted as-is rather than churning the whole lockfile |
| R6.10 | [x] P1 | **Docs alignment (B1–B19):** candidates arithmetic corrected everywhere (`buckets + errors == candidates`; EXECUTION examples made internally consistent); `total_files`/`candidates` semantics; fix gate wording (asserts UTF-8 no BOM regardless of `--target`, now enforced by R6.4); EXECUTION `0.1.0` → current version; DESIGN/ROADMAP `toml 0.9` → 1.2; DESIGN §3 detect snippet mirrors detector.rs (incl. empty arm); USAGE detection-order line; config absent-key list gains `ignore_file`; README UTF-32 marked detect/read-only; value-name placeholders vs `--help`; DESIGN §9 "GLOBAL OPTIONS" → shared tree options; AGENT/DESIGN inventory gains ROADMAP.md + error.rs description; EXECUTION §12.1 cross-ref, gb18030 97-vs-171, `duration_ms` comment, `unknown_files` convert/fix wording; README zh 安全规则 wording. Plus documentation of every new R6.1–R6.5 behavior (EN/zh 1:1). | B1–B19 | every claim re-verified against the binary in the verification round |

---

## R7 — Reliability and filtering follow-up (unreleased, 2026-11-07)

The source review against `a2a6231` found that R6.7's single-load config
acceptance still needed implementation. Commit `74356a4` completes that part
alongside bounded reads, verify diagnostics and atomic-write improvements.
The remaining work below follows the existing filtering, streaming-fetch and
cross-platform validation contracts. R4's project-graph integration remains
deferred pending a concrete repository requirement.

| # | Priority | Item | Acceptance and verification |
|---|----------|------|-----------------------------|
| R7.1 | [x] P1 | **Bounded reads and shared config snapshot.** Local input and fetch-output comparisons read at most the cap plus one byte. Convert/fix reuse the parsed config. Verify prints auto-ignore warnings. | Exact-cap, zero-cap, growing-input read, config precedence, warning/quiet and matching-prefix fetch tests pass. Implemented in `74356a4`. |
| R7.2 | [x] P1 | **Atomic-write reliability.** Short exclusive temp names support long basenames. A process-local counter separates concurrent names. Permissions are copied before sync, failures abort replacement, and bare destinations use the current directory for parent sync. | Long-name conversion with backups, permission retention, failed-rename cleanup and concurrent complete writes pass. Implemented in `74356a4`. |
| R7.3 | [x] P1 | **Exact glob matching with bounded complexity.** Replace recursive backtracking and the false-negative step-budget fallback with iterative matching. | Adversarial valid exclusions still match, Unicode `?` and component `**` semantics hold, and exhaustive small-input reference comparisons pass. |
| R7.4 | [x] P1 | **Fetch temp-file ownership and cleanup.** Exclusively create the stream file, rewind the original handle for read-back and close it before cleanup. | Success, oversize and injected read failures clean up owned temps. Existing files and symlinks at a colliding path retain their contents. |
| R7.5 | [x] P1 | **Complete CI matrix.** Run Linux/macOS/Windows against default/all/local-only features and map Cargo arguments explicitly. | Nine unique build/test combinations, default-target smoke enabled, workflow lint and exact exit-status smoke checks pass. |

Integrated local verification used stable Rust on macOS with 173 all-feature
tests and 147 local-only tests passing. Both feature configurations pass
Clippy with warnings denied. Formatting, rustdoc with warnings denied,
`actionlint` and the extracted CI smoke script pass. Glob tests compare
178,425 generated cases against an independent exhaustive matcher. Matrix
expansion checks confirm all nine platform/feature combinations.

## R8 — Review-fix wave (v0.2.2, four-expert adversarial review of the R7 tree)

Produced from a four-expert review (code correctness `E*`, docs-vs-code audit
`F*`, adversarial CLI QA `G*`, dependency/API hygiene `H*`) of HEAD `f24c342`.
23 findings: one P1, four P2, eighteen P3. The R7 surfaces (glob matcher,
bounded reads, atomic write, fetch temp ownership) were independently cleared
by three experts (178,425-case oracle, 800-pattern differential, ~150 CLI
probes) with zero regressions. Verification of this wave: every change claim
measured TRUE against the binary; the follow-up round's findings — the glob
note's `src` vs `src/**` contrast (no observable difference), the threads
bound not enforced in fetch mode (now enforced there too), and one internally
inconsistent §12 example — were fixed and re-verified.

| # | Priority | Item | Source | Acceptance |
|---|----------|------|--------|------------|
| R8.1 | [x] P1 | **CI smoke job fails on Windows**: the smoke script calls `python3`, which GitHub windows-latest runners deliberately do not provide (only `python`). With `fail-fast: false` the Windows leg fails on every push once it reaches that line. | H1 | smoke assertions run via `python` (or `jq`) on all three OSes; CI matrix green |
| R8.2 | [x] P2 | **Fetch mode misbuckets unknown-encoding payloads as `io_errors`.** `cmd_convert_url` records action `error` for every transcode failure, while tree mode maps `UnknownEncoding` to `skipped_unknown`; the convert bucketing then falls through to `io_errors`, the row lands in both `unknown_files` and `error_files`, and the summary disagrees with tree mode on identical bytes. Fix: mirror `convert_file`'s `skipped_unknown` mapping (console line then matches the report row). | E1 | fetch of an unclassifiable payload → exit 1, `summary.unknown == 1`, `io_errors == 0`, action `skipped_unknown`, absent from `error_files`; test added |
| R8.3 | [x] P2 | **Fetch silently clobbers a write-protected destination.** The read-only refusal lives in `convert_file` only; `--url` writes jump straight to `atomic_write`, and rename bypasses the mode bits (the file even gets its 0444 mode re-stamped). Fix: replicate the write-protected guard and message in the fetch write branch. | E2 | 0444 destination + differing fetch → exit 1, `error` record naming write-protected, bytes and mode unchanged; test added |
| R8.4 | [x] P2 | **`--threads` has no upper bound**: `--threads 99999` deterministically wedges the process for tens of CPU-minutes even on a one-file tree (quadratic rayon pool startup), silently. Fix: values above 1024 are a fatal input error (exit 2), symmetric with the loud-strict-input posture; help and docs state the bound; fetch mode (which builds no pool) enforces the same bound up front. | G1 | `--threads 1024` accepted; `--threads 1025` → exit 2 with a clear message in tree and fetch modes; help + USAGE document the bound |
| R8.5 | [x] P2 | **EXECUTION §12 example record contradicts its own rule**: a `converted` record carries an `error` field, impossible per reporter.rs (`skip_serializing_if`). Split the write-protected case into its own `error` record. | F1 | example matches the schema |
| R8.6 | [x] P3 | **Strictness & defaults polish:** (a) `TreeArgs::default()` sets `max_file_size: Some(…)` so programmatic runs skip the `[options].max_file_size` merge — default to `None` like the parser (the masked config test asserts the effective cap); (b) `wall_now` doc comment says "when the run starts" but the stamp is taken after the walk — reword to "when per-file processing begins" (docs already say this); (c) replace `from_ref(&report.files[0])` with the slice; (d) document `FilterConfig` pub fields as post-normalization output of `build()`. | E4/E5/H7/H9 | config `[options].max_file_size` reaches programmatic runs; comments match behavior; no indexing in the fetch print path |
| R8.7 | [x] P3 | **Docs precision (F/E/G):** ASCII-only case-insensitivity qualified in USAGE/DESIGN; `--exclude-glob` prefix semantics spelled out (a bare directory-name pattern excludes the whole subtree); EXECUTION §12 `options` example sorted as the binary emits; `duration_ms` comment split per mode (fetch excludes console lines); zh USAGE stray trailing `---` removed; DESIGN §13 / AGENT §8 CI-gate summary matches ci.yml (`--all-features`, `cargo doc` gate). | E3/G2/F2/F3/F4/F6 | every claim re-verified against the binary |
| R8.8 | [x] P3 | **Workflow hygiene:** release.yml unused `BIN` variable (actionlint SC2034) removed, misleading smoke step renamed, stale `bsdtar` comment corrected, rust-clippy's `continue-on-error` annotated as advisory SARIF; pre-existing `(G1)`/`§7` comments in ci.yml scrubbed to plain statements. | H2–H6 | actionlint clean with no dead variables; workflow comments self-contained |
| R8.9 | [x] P3 | **Tests & packaging notes:** ignore-file CRLF × negation combo test; e2e fix-with-oversized-file (exit 3); non-UTF-8 filename lossy-path test (cfg unix); the writer long-name test's unix-only gating noted. `AGENT.md` staying in the published crate's include list is recorded as deliberate (user-trimmed list). | H12/E-gaps | each gap has a named test; include-list decision recorded |

## R9 — Review-fix wave (v0.2.3, four-expert adversarial review of v0.2.2)

Produced from a four-expert review (code correctness `I*`, docs-vs-code audit
`J*`, adversarial CLI QA `K*`, dependency/API hygiene `L*`) of HEAD `deb139b`.
Two P1s (the GB18030 mojibake found independently by both code review and QA,
and release-notes debt), six P2s, ~twenty P3s. The QA round ran a 1,112,064
-scalar encode/decode oracle; the docs round executed every runnable example
block; the hygiene round diffed the published 0.2.0 tag against HEAD item by
item. Verification of this wave: every change claim measured TRUE against the
binary; gate semantics were refined mid-wave so only traversal warnings
(uninspectable subtrees) fail verify/fix while ignore-layer problems stay
advisory; R9.8's decisions are recorded in the item and await the user.

| # | Priority | Item | Source | Acceptance |
|---|----------|------|--------|------------|
| R9.1 | [x] P1 | **GB18030 encode silently rewrites U+E5E5 as the literal text `&#58853;`** (exit 0). `encode()` ignores encoding_rs' `had_errors`; U+E5E5 (PUA, common in legacy CJK identifiers) is the single unmappable scalar per the 1.1M-scalar oracle. The corrupted file is pure ASCII, so `scan` buckets `utf8` and `verify` exits 0 — the tool's own gates certify the corruption. Fix: check `had_errors`, return `ForgeError::EncodeError` (the R6 plumbing already counts/exits it); document the adjacent WHATWG facts (legacy-GB18030 byte spellings canonicalize on a gb18030→gb18030 pass; 19 PUA scalars consolidate — WHATWG-correct, now documented). | I1/K1/I3/K2 | file containing U+E5E5 + `--target gb18030` → exit 1, `encode_error` record, bytes untouched; exhaustive-oracle unit test; docs state the unmappable scalar and the canonicalization behavior |
| R9.2 | [x] P1 | **No CHANGELOG and no release tags/notes for published versions**: crates.io has 0.1.0/0.2.0 while `main` carries 0.2.1/0.2.2 untagged, and five lib-visible breaks (DiffReport fields, `ForgeError::EncodeError` + `encode()` returning `Result`, `IgnoreFile::len/is_empty` removal, `DiffInput.files` required, `TreeArgs::default` semantics) are recorded nowhere a consumer can find — the verification round confirmed the `fetcher` feature gate predates 0.1.0 and is therefore not a break, dropping L2's sixth item. Fix: add `CHANGELOG.md` (Keep-a-Changelog) retro-documenting 0.1.0–0.2.2 with the breaks and migrations; tagging/publishing is a release decision made after this wave. | L1/L2/J1 | CHANGELOG lists every lib-visible change 0.1.0→current with a migration sentence; ROADMAP release record cross-references it |
| R9.3 | [x] P2 | **verify/fix gates fail open for unreadable subtrees**: walk errors (chmod-000 directory holding a BOM'd whitelisted file) become warnings only — `verify` exits 0 with the file unasserted, and `--quiet` hides the warning entirely. Fix: in gate modes walk warnings fail the gate (exit 3, gate-wins precedence), warnings print to stderr regardless of `--quiet`, and the report gains an additive `warnings` array (omitted when empty). | I2 | chmod-000 dir with a BOM'd `.cpp` → verify/fix exit 3, stderr line under `-q`, report `warnings` non-empty; clean trees unchanged |
| R9.4 | [x] P2 | **Report-target strictness:** (a) `--report`/`diff --report` silently overwrite an existing read-only report FILE (rename bypasses mode bits) and the new report inherits 0444; (b) `--report <root>/.utf8forgeignore` replaces the operator's auto-ignore rule file with JSON; (c) a bare-filename `--report` is dropped from the traversal skip set (empty parent canonicalizes to nothing). Fix: pre-flight refuses a write-protected report file (exit 2); the active ignore-file path is refused as a report target; the skip-set maps an empty parent to `.`. | K3/K6/I4 | chmod-444 report → exit 2, file untouched; report == ignore file → exit 2; bare `--report r.json` never enters `files[]` across runs |
| R9.5 | [x] P2 | **Config strict-input closures** (same posture as the CLI): `[filter].extensions = []` (present-but-empty) silently means "use the default whitelist" while the identical CLI form is fatal — make the key `Option<Vec<String>>` and fatal on present-but-empty; `[convert].backup_dir = ""` is silently swallowed (and its downstream behavior drifts with cwd) — fatal naming the key. | K4/K5 | both forms → exit 2 with the config key named; absent keys unchanged; CHANGELOG notes the `FilterSection.extensions` type change |
| R9.6 | [x] P2 | **CI coverage & lib docs:** (a) the lint job only clips `--all-features`, so `#[cfg(not(feature = "remote"))]` code is never clippy-checked — add a `--no-default-features` clippy pass; (b) the lib target is published but has no purpose statement or example (the crate-level doc reads as internal orchestration) — add both (doctest exercises `detect`); (c) stale document-version stamps: DESIGN §9/§14 "(v0.1)" qualifiers and the DESIGN/AGENT "v0.1" footers describe post-0.1 content — drop/re-stamp. | L4/J2/J3 | lint job runs two clippy invocations; `cargo test --doc` ≥ 1; no "(v0.1)" stamps remain |
| R9.7 | [x] P3 | **Hardening polish:** `decode_strict`'s Gb18030 arm uses the BOM-sniffing `decode()` (unreachable from the CLI; switch to `decode_without_bom_handling` for doc consistency); the writability-probe file is left behind on remove failure (warn); `scan_exits_1_on_io_errors` asserts only the exit code (add report-content assertions); the `derive_output_path_rules` integration test duplicates fetcher unit coverage (remove); named test gaps: config-only `include_rc` conflict, non-whitelisted unreadable files skipped without I/O, cross-mode diff, ignore×hidden pruning, `verify_file` zero-cap. | I5/I6/L6/L7/L5 | each gap has a named test; no behavior change |
| R9.8 | [x] P3 | **Release & packaging posture (decisions recorded):** whether to publish 0.2.3 (J1: docs describe unreleased behavior — the honest fix is shipping), keep or drop `AGENT.md` from the crate `include` list (L3), accept rust-clippy.yml's unpinned advisory tools (L8), the `@master` toolchain ref (L12), daily-only audit cadence (L13), and the `tests/`-not-packaged warning (L15). | J1/L3/L8/L12/L13/L15 | each decision recorded in ROADMAP; executed per the user's choice |

---

*ROADMAP v0.7 — R1/R2, R5–R8 record previous delivery waves; R9 tracks the
review of the v0.2.2 tree. R3 and R4 retain scope decisions. `CHANGELOG.md`
now carries the per-version history these waves produced.*
