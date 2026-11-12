# Changelog

All notable changes to `utf8-forge` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the crate's
`Cargo.toml`. The library target (`utf8_forge`) is published alongside the
CLI, so entries marked **lib-visible** affect downstream Rust consumers.
Pre-1.0 releases may change the library API between patch versions — every
such change is listed explicitly.

## [0.2.4] — unreleased

### Fixed

- A closed pipe (`… | head`) no longer panics with `failed printing to
  stdout` (exit 101): SIGPIPE keeps its default disposition on Unix, and the
  JSON report is written before any console output, so a pipe cut can no
  longer drop `--report`.
- Fetch refuses bodyless success responses (204/304/3xx without a location)
  as `fetch_error` instead of silently writing a zero-byte output (which
  truncated an existing destination with exit 0).
- The whole download must finish within a 120 s wall-clock budget enforced
  by the streaming loop (the client timeout only applies per read, so a
  slow-but-steady trickle could run indefinitely).
- A root directory whose canonical path cannot be resolved is a fatal input
  error (previously it silently disabled report-exclusion and backup
  placement guards).

### Added

- Documentation for the fetch environment: proxy handling (`NO_PROXY`
  guidance), non-decompressed `Content-Encoding` behavior, redirect naming,
  and the tree-mode memory envelope (≈ `--max-file-size` × worker threads).
- `CHANGELOG.md` now ships in the published crate.

## [0.2.3] — 2026-11-13 (tag `v0.2.3`; published to crates.io)

### Fixed

- **gb18030: content containing U+E5E5 is refused** (`encode_error`, exit 1)
  instead of being silently rewritten as the literal text `&#58853;`. U+E5E5
  is the only Unicode scalar the WHATWG gb18030 encoder cannot map; the
  refusal reuses the UTF-16LE NUL-guard plumbing. A `--target gb18030` pass
  still canonicalizes legacy byte spellings into their WHATWG forms
  (content-equivalent; idempotent from the second pass) — now documented.
- **`verify` / `fix` fail closed on unreadable subtrees**: traversal warnings
  (e.g. a directory that cannot be entered) now fail the gate (exit 3) and
  are printed to stderr even under `--quiet`, instead of silently skipping
  un-inspected whitelisted files. `scan`/`convert` keep advisory warnings.
- **Report targets are protected**: a write-protected existing report file is
  refused at pre-flight (exit 2) instead of being silently replaced via
  rename; a report path equal to the active ignore file
  (`<root>/.utf8forgeignore` or `--ignore-file`) is refused; a bare-filename
  `--report` is reliably excluded from traversal.
- **Config strictness** (same posture as the CLI): `[filter].extensions = []`
  (present but empty) is now fatal instead of silently meaning "use the
  default whitelist"; `[convert].backup_dir = ""` is fatal, naming the key.

### Added

- **lib-visible**: the report document gains an additive top-level
  `warnings: string[]` field (omitted when empty) carrying traversal/ignore
  warnings.
- `CHANGELOG.md` (this file).

### Changed

- **lib-visible (breaking)**: `encode()` now returns
  `Result<Vec<u8>, ForgeError>` (paired with the gb18030 refusal above).
- **lib-visible (breaking)**: `FilterSection.extensions` is now
  `Option<Vec<String>>` — `None` means the key is absent. Struct literals
  need `Some(...)` / `None`.

## [0.2.2] — 2026-11-09 (unreleased: committed to `main`, not tagged or published)

### Fixed

- `--threads` above 1024 is a fatal input error in every mode (rayon pool
  startup grows quadratically; implausible values silently wedged the
  process). `--url` fetch enforces the same bound up front.
- Fetch mode classifies unrecognizable payloads as `skipped_unknown` (the
  summary `unknown` bucket and `unknown_files` list), matching tree mode,
  instead of leaking them into `io_errors`/`error_files`.
- Fetch mode refuses write-protected destinations (`io_error: file is
  write-protected`), matching tree mode; rename bypasses mode bits, so the
  0444 destination used to be silently replaced and re-stamped.
- `TreeArgs::default()` leaves `max_file_size` unset so programmatic runs get
  the same `[options].max_file-size` config merge as the CLI (the built-in
  200 MiB cap still applies via `effective_max_file_size`).
- CI: the Windows smoke leg uses `python` (windows-latest runners ship no
  `python3` alias); release/clippy workflow hygiene (dead variable, step
  names, advisory-SARIF annotation).

## [0.2.1] — 2026-11-06 (unreleased: committed to `main`, not tagged or published)

### Fixed

- **UTF-16LE NUL guard (P0)**: `--target utf-16le` refuses content whose
  first character is U+0000 — the output would begin `FF FE 00 00`, which the
  next run reads back as the UTF-32LE BOM and silently mangles.
- Fetch mode runs the report pre-flight (unwritable report aborts before the
  download) and prints exactly one stderr line per failure.
- `fix` accepts only the default target `utf-8` (the verify gate asserts
  UTF-8 without BOM); other targets are a fatal exit 2.
- `diff` hardening: inputs must be real utf8-forge reports (a `files[]`
  array — **lib-visible (breaking)**: `DiffInput.files` deserialization no
  longer defaults), record paths compare relative to each report's root
  (**lib-visible (breaking)**: `DiffReport` gained `before_root` /
  `after_root`, omitted from JSON when absent), and the summary line moved
  to stderr so stdout stays pipeable; plus the full adversarial-review fix
  wave (fetch pre-flight, per-subcommand flag surfaces, report and config
  strictness) recorded in `ROADMAP.md`.

### Added

- **lib-visible (breaking)**: `ForgeError::EncodeError(String)` variant with
  the `encode_error` report category. (The follow-up signature change —
  `encode()` returning `Result<Vec<u8>, ForgeError>` — is listed under
  0.2.3, where it landed.)
- **lib-visible (breaking)**: `IgnoreFile::len` / `IgnoreFile::is_empty`
  removed (dead code with no callers).
- Summary `encode_errors` counter in the convert/fetch/fix summaries.

## [0.2.0] — 2026-11-05

Released on crates.io and GitHub Releases (tag `v0.2.0`). Feature wave:
`.utf8forgeignore` support (gitignore subset, auto-discovery, negation),
`--config` TOML (CLI > config > defaults, strict unknown keys), UTF-32
LE/BE read support (WHATWG BOM precedence), streaming fetch (temp file,
per-chunk cap), the `fix` subcommand (convert + verify gate, exit 3), and the
`diff` subcommand (`--before/--after/--report/--fail-on-change`).

## [0.1.0] — 2026-11-05

Initial release: rule-based detection (BOM → strict UTF-8 → strict GB18030,
never guesses), idempotent conversion (byte-identical → no write), atomic
temp+rename writes with optional `--backup-dir`, scan/convert/verify plus
remote fetch (`convert --url`), JSON reports sorted by path, exit codes
0/1/2/3.

## [Unreleased]

See `ROADMAP.md` for planned work and the review-wave history behind every
entry above.
