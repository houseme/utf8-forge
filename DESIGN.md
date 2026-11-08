# DESIGN.md — utf8-forge Architecture & Algorithm Design

> **Tool**: `utf8-forge` — a focused, idempotent, bidirectional charset transcoder for Windows source trees.
> **Edition**: Rust 2024. Resolver `"3"` implied.
> **Status**: Designed against the P1 phase of *Source UTF-8 Unification & Cross-Platform Charset — Executable
Playbook*.

---

## 1. Goals & Non-Goals

### 1.1 Goals

| #  | Goal                                                           | Acceptance                                 |
|----|----------------------------------------------------------------|--------------------------------------------|
| G1 | Default output **UTF-8 without BOM**                           | Byte-level verification on every `convert` |
| G2 | **Idempotent**: second run changes 0 bytes                     | `converted == 0` on re-run                 |
| G3 | **Lossless round-trip** across all supported encodings         | `A→B→A == A` in unit tests                 |
| G4 | **Whitelist-driven recursion** (never touch binaries / `dep/`) | `dry-run` audit passes                     |
| G5 | **Atomic writes** (crash-safe)                                 | Temp file + `rename`                       |
| G6 | **Optional remote download** → transcode → save                | `--url` flag works for HTTP/HTTPS          |
| G7 | Human-readable + JSON report                                   | `--report` emits stable schema             |
| G8 | **Parallel** file processing                                   | `rayon`; report re-sorted by path          |

### 1.2 Non-Goals

- No line-ending normalization (CRLF/LF preserved verbatim).
- No BOM-less UTF-16 guessing (only BOM-sniffed UTF-16 is recognized).
- `.rc` excluded by default (`--include-rc` opt-in) — rc.exe has limited UTF-8 support.
- No SVN/Git operations. No compilation. No GUI.
- Remote download is **single-URL fetch**, not recursive crawl / mirroring.

---

## 2. Encoding Model

### 2.1 Detectable Source Encodings

| Tag       | Detection                                                       | Notes                                  |
|-----------|-----------------------------------------------------------------|----------------------------------------|
| `Utf8`    | No BOM + `str::from_utf8` OK                                    | Fast path; pure ASCII hits here        |
| `Utf8Bom` | Starts with `EF BB BF`                                          | BOM stripped on decode                 |
| `Utf32Be` | Starts with `00 00 FE FF` (checked right after the UTF-8 BOM)   | Read-only: no UTF-32 write target      |
| `Utf32Le` | Starts with `FF FE 00 00` (`FF FE` prefix shared with UTF-16 LE; the next two bytes decide) | Same    |
| `Utf16Le` | Starts with `FF FE` **not** followed by `00 00`                 | BOM stripped before decoding           |
| `Utf16Be` | Starts with `FE FF`                                             | Same                                   |
| `Gb18030` | UTF-8 strict fail → `GB18030.decode` with `had_errors == false` | Covers GB2312/GBK/GB18030              |
| `Unknown` | All above fail                                                  | Reported & skipped — **never guessed** |

**Detection order is frozen**: UTF-8 BOM → UTF-32 BE BOM (`00 00 FE FF`) →
`FF FE` prefix (UTF-32 LE when followed by `00 00`, else UTF-16 LE) →
UTF-16 BE → strict UTF-8 → strict GB18030 (WHATWG-style longest-prefix
precedence). Reordering breaks idempotence on ASCII-only files. Consequence:
a file starting `FF FE 00 00` is always decoded as UTF-32LE — a UTF-16LE
encoding whose first character is U+0000 is genuinely indistinguishable there.
If the payload is not also valid UTF-32, the strict UTF-32 decode fails loudly
(`decode_error`, never silent mojibake); if it is, the file reads as UTF-32.
That ambiguity is exactly why the UTF-16LE encoder refuses content that begins
with U+0000 (§2.2): the output would start `FF FE 00 00` and could never be
re-read as UTF-16.

### 2.2 Target Encodings

| Target            | Output bytes                                        |
|-------------------|-----------------------------------------------------|
| `utf-8` (default) | `text.as_bytes().to_vec()` — **no BOM**             |
| `utf-8-bom`       | `[EF,BB,BF] ++ text.as_bytes()` — BOM **manual**    |
| `gb18030`         | `GB18030.encode(text)`                              |
| `utf-16le`        | `[FF,FE] ++ utf16le_bytes(text)` — BOM **manual**   |
| `utf-16be`        | `[FE,FF] ++ utf16be_bytes(text)` — BOM **manual**   |

where `utf16le_bytes` / `utf16be_bytes` are `str::encode_utf16()` serialized little-endian / big-endian.

**UTF-16LE NUL guard**: `--target utf-16le` (tree and fetch modes) refuses to
encode content whose first character is U+0000. The output would begin
`FF FE 00 00`, byte-identical to the UTF-32LE BOM, so a second run would
mis-read the file as UTF-32LE and corrupt it. The file is recorded as an
`error` row with an `encode_error` reason (counted under the summary's
`encode_errors` counter), exit 1; the original bytes are untouched.
`--target utf-16be` is safe (`FE FF 00 00` re-reads as UTF-16BE) and
unaffected.

**No UTF-32 target**: UTF-32 is read-only (detection + strict decode). A
UTF-32 value on `--target` is rejected by clap (exit 2); `verify` fails
UTF-32 files because they are not UTF-8.

> **Critical invariants**:
> - `encoding_rs`'s `UTF_16LE/BE` encoders **do not emit BOM**. The BOM must be prepended by `utf8-forge` itself.
>   Forgetting this produces files that `detect()` would classify as `Utf8` or `Unknown`.
> - encoding_rs' generic `Encoding::encode()` maps UTF-16 output encodings back to UTF-8; UTF-16 bytes are produced
>   with `str::encode_utf16()` directly. encoding_rs' `encode()` also BOM-sniffs, so UTF-16 decode uses
>   `decode_without_bom_handling` after the manual BOM strip (double-BOM files keep their content U+FEFF).

### 2.3 Line Endings & Surrogates

`encoding_rs` performs **no newline normalization**. `"\r\n"` survives UTF-16 ↔ UTF-8 round-trips byte-identically.
Invalid UTF-16 surrogate pairs set `had_errors = true` → hard error, never a half-written file.

---

## 3. Detection Algorithm (frozen)

```rust
pub fn detect(bytes: &[u8]) -> DetectedEncoding {
    if bytes.is_empty() {
        return DetectedEncoding::Utf8;
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return DetectedEncoding::Utf8Bom;
    }
    // WHATWG BOM sniffing: the 4-byte UTF-32 patterns are checked before the
    // 2-byte UTF-16 patterns. The UTF-32BE BOM is `00 00 FE FF`; a payload
    // starting `FE FF 00 00` is a UTF-16BE file whose first character is
    // U+0000 — UTF-16BE, not UTF-32BE.
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return DetectedEncoding::Utf32Be;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
            return DetectedEncoding::Utf32Le;
        }
        return DetectedEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return DetectedEncoding::Utf16Be;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return DetectedEncoding::Utf8;
    }
    let (_, _, had_errors) = GB18030.decode(bytes);
    if !had_errors {
        return DetectedEncoding::Gb18030;
    }
    DetectedEncoding::Unknown
}
```

**Why not `chardetng`?** The target tree is known to be GB18030 + some UTF-16 + UTF-8. Statistical detection produces
false positives on short files and destroys determinism. Rule-based detection is auditable and reproducible.

---

## 4. Transcoding

### 4.1 Fast Path

```rust
if src == DetectedEncoding::Utf8 & & target == TargetEncoding::Utf8 {
return TranscodeOutcome::Unchanged; // zero byte allocation
}
```

### 4.2 Strict Decode

| Source    | Action                                                 |
|-----------|--------------------------------------------------------|
| `Utf8`    | `str::from_utf8(bytes)?`                               |
| `Utf8Bom` | `str::from_utf8(&bytes[3..])?`                         |
| `Utf16Le` | `UTF_16LE.decode_without_bom_handling(&bytes[2..])`; reject on `had_errors` |
| `Utf16Be` | `UTF_16BE.decode_without_bom_handling(&bytes[2..])`; reject on `had_errors` |
| `Utf32Le` | Manual unit decode of `&bytes[4..]`; reject surrogates, values > `0x10FFFF`, and misaligned tails (length not a multiple of 4) |
| `Utf32Be` | Same, big-endian units                                 |
| `Gb18030` | `GB18030.decode(bytes)`; reject on `had_errors`        |
| `Unknown` | `Err(UnknownEncoding)` — **no guessing**               |

### 4.3 Encode (BOM handling)

```rust
fn encode(text: &str, target: TargetEncoding) -> Vec<u8> {
    match target {
        TargetEncoding::Utf8 => text.as_bytes().to_vec(),
        TargetEncoding::Utf8Bom => {
            let mut v = Vec::with_capacity(3 + text.len());
            v.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            v.extend_from_slice(text.as_bytes());
            v
        }
        TargetEncoding::Gb18030 => {
            let (cow, _, _) = GB18030.encode(text);
            cow.into_owned()
        }
        TargetEncoding::Utf16Le => {
            // encoding_rs' generic encode() would map this target back to
            // UTF-8; serialize `str::encode_utf16()` little-endian instead.
            // NUL guard: the caller (transcode) refuses content whose first
            // character is U+0000 — the output would start `FF FE 00 00`,
            // byte-identical to the UTF-32LE BOM, and would be mis-read as
            // UTF-32LE on the next run (`encode_error`, exit 1).
            let mut v = Vec::with_capacity(2 + text.len() * 2);
            v.extend_from_slice(&[0xFF, 0xFE]);
            for unit in text.encode_utf16() {
                v.extend_from_slice(&unit.to_le_bytes());
            }
            v
        }
        TargetEncoding::Utf16Be => {
            // Same as above, serialized big-endian.
            let mut v = Vec::with_capacity(2 + text.len() * 2);
            v.extend_from_slice(&[0xFE, 0xFF]);
            for unit in text.encode_utf16() {
                v.extend_from_slice(&unit.to_be_bytes());
            }
            v
        }
    }
}
```

### 4.4 Idempotence Proof

- `Utf8` (no BOM) + `target = utf-8` → `Unchanged` (fast path).
- `Utf8Bom` + `target = utf-8` → BOM stripped; second run sees `Utf8` → `Unchanged`.
- `Gb18030` + `target = utf-8` → UTF-8; second run sees `Utf8` → `Unchanged`.

✅ **Any input converges to `Utf8` in one pass; second pass is a no-op.**
Known exception: a UTF-16 file whose content begins with a second BOM (U+FEFF)
converges in two passes — pass 1 writes the content U+FEFF as a UTF-8 BOM
(EF BB BF), which pass 2 strips; pass 3 is a no-op.

---

## 5. File Selection

### 5.1 Default Extension Whitelist

```rust
// Source code + config/data + docs (safe set)
const DEFAULT_EXTENSIONS: &[&str] = &[
        // C / C++
        "c", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "h++",
        "inl", "ipp", "tpp", "ixx", "m", "mm",
        // .NET / JVM
        "cs", "csx", "java", "kt", "kts",
        // Other languages
        "rs", "go", "py", "pyi", "rb", "php", "pl", "pm", "lua",
        "js", "jsx", "ts", "tsx", "mjs", "cjs",
        "sh", "bash", "zsh", "ps1", "bat", "cmd",
        // Data / config
        "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg", "conf",
        "xml", "csv", "tsv", "sql",
        // Docs / protocol
        "md", "markdown", "txt", "rst", "proto",
    ];

// Opt-in only (not enabled by default)
const OPT_IN_EXTENSIONS: &[&str] = &[
    "rc", "rc2", "dlg",                                     // rc.exe limited UTF-8
    "sln", "vcxproj", "vcproj", "props", "targets", "filters", "user", // VS re-saves with BOM
];
```

### 5.2 Excluded Directories (case-insensitive + prefix rules)

```rust
const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    "dep", "deps", "third_party", "third-party", "3rdparty", "vendor", "external", "extern",
    "_tools", "tools_bin",
    ".svn", ".git", ".hg", ".vs", ".idea", ".vscode",
    "build", "out", "bin", "obj", "debug", "release", "target",
    "node_modules", "__pycache__", ".venv", "venv",
    "temp", "tmp", "cache",
];

const DEFAULT_EXCLUDE_PREFIXES: &[&str] = &[
    "outputfile", // OutputFile* / OutputFiles*
];
```

### 5.3 Additional Skips

- Symlinks: `follow_links(false)` — walker prunes automatically.
- Extension matching: `to_ascii_lowercase()`.
- `--max-file-size` (default 200 MiB) → recorded as `too_large`.
- Local reads stop after the limit plus one byte, allowing growth after the
  metadata check to be detected with bounded input buffering. The extra byte
  is used only to reject oversized input. For a growing file, the recorded
  size can therefore be a lower bound on its final size.
- Hidden files (`.`-prefixed) skipped unless `--include-hidden`.

Glob matching uses iterative latest-wildcard retries at both the character
and path-component levels. Component matching takes at most
O(pattern characters × filename characters) work. Path matching takes at most
O(pattern components × path components) component comparisons. The matcher
fully evaluates valid exclusions, including those that previously exhausted
the recursive step budget. `?` consumes one Unicode scalar value, `*` stays
within a component and `**` spans zero or more components. Matching is
case-insensitive (ASCII folding — non-ASCII byte pairs compare literally).
A pattern matches a prefix of the root-relative component list: a bare `src`
(or `src/`, or `src/**` — a trailing `**` spans zero components too) excludes
the whole `src/` subtree (matching directories are pruned with their contents);
`**/name/**` matches at any depth. A pattern must consume an entry's full
root-relative component list.

### 5.4 Ignore Files

Gitignore-style exclusions via `--ignore-file <PATH>` (fatal exit 2 when the
explicit file is missing) or auto-discovery of `<root>/.utf8forgeignore` when
the flag is absent (missing → silent; unreadable → warning + continue).
`--no-ignore-file` disables both sources. The ignore file itself is never
scanned (it is added to the walker skip set).

Supported syntax — a deliberate gitignore subset:

- `#` full-line comments; blank lines skipped.
- Trailing `/` → dir-only pattern.
- Leading `/` or any inner `/` → anchored to the root-relative path; a pattern
  without `/` matches the basename at any depth.
- `**` spans path components; `*` / `?` match within one component.
- `!` negation with **last-match-wins** ordering.
- No escapes — a backslash is a literal character. Matching is case-insensitive
  (ASCII folding — non-ASCII byte pairs compare literally); folding is pinned
  by tests to ASCII letters, so a non-ASCII pattern character matches only its
  literal counterpart.

Limitation inherited from git: a file cannot be re-included (`!`) when its
parent directory is excluded — the walker prunes the subtree on the directory
verdict and children are never evaluated.

The ignore layer is an **independent exclusion layer** ANDed with the existing
gates: `!` negation lifts only the ignore layer, never the whitelist /
`--exclude` / `--exclude-glob` layers.

### 5.5 Config File

`--config <PATH>` loads defaults from a TOML document
(`src/config.rs`, `toml 1.2` parse+serde):

```toml
[filter]
extensions        = ["h", "cpp"]   # replaces the default whitelist
exclude_dirs      = ["legacy"]     # appended to the default excludes
exclude_ext       = ["png"]
exclude_globs     = ["**/Generated/**"]
include_rc        = false
include_msbuild   = false
include_hidden    = false

[convert]
target     = "utf-8"               # utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be
backup_dir = "D:\\backup\\orig"

[options]
max_file_size = 209715200
threads       = 8
```

- **Precedence**: CLI > config > built-in defaults. `--target` and
  `--max-file-size` are optional-valued flags: the CLI value applies only when
  the flag is passed, otherwise the config value, otherwise the default.
- **Lists replace** across layers (never concatenate) so the effective
  whitelist stays auditable.
- Unknown keys, malformed TOML, and wrong types are **fatal exit 2**; messages
  carry the TOML line/column.
- Fetch mode reads no config: `--config` combined with `--url` is a fatal
  exit 2. Deliberately absent from the schema: `root`,
  `report`, `dry_run`, `url`, `output`, `quiet`, `verbose`, `ignore_file` —
  options that select what gets written where or flip write behavior stay
  visible on the command line.

---

## 6. Traversal & Concurrency

```rust
// Phase 1: collect candidates (prune dirs early)
let files: Vec<PathBuf> = WalkDir::new(root)
.follow_links(false)
.into_iter()
.filter_entry( | e| ! is_excluded_dir(e.file_name(), & cfg))
.filter_map(Result::ok)
.filter( | e| e.file_type().is_file() & & is_whitelisted_ext(e.path(), & cfg.exts))
.map( | e| e.into_path())
.collect();

// Phase 2: parallel process
let results: Vec<FileOutcome> = files
.par_iter()
.map( | p| process_file(p, & cfg))
.collect();

// Phase 3: deterministic order
results.sort_by( | a, b| a.path.cmp( & b.path));
```

No shared mutable state between files. IO-bound workload; default rayon thread pool is appropriate. `--threads`
overrides via `ThreadPoolBuilder`.

---

## 7. Atomic Writes

`writer::atomic_write` performs the following steps.

1. Exclusively create a short temporary filename in the destination directory
   with `File::create_new`. Its suffix combines the process ID, timestamp and
   atomic sequence counter, supporting concurrent writers and long destination
   basenames.
2. Write the complete content and copy the destination's permissions when it
   exists. Permission-copy failures abort the replacement.
3. Sync and close the temporary file, then rename it over the destination on
   the same volume.
4. Attempt to sync the parent directory, using `.` for bare filenames.
5. On failure, attempt to remove the temporary file after closing its handle.
   Cleanup applies only to a temporary file successfully created by this call.

**Never truncate-then-write**: a crash mid-write destroys source code. Temp file + rename guarantees either old or new
content, never neither.

`--backup-dir` copies original bytes **before** rename, preserving relative path structure.

`--dry-run` records `would_convert` without touching disk.

---

## 8. Remote Download (Optional Feature)

### 8.1 Design

A new **`--url` flag** on `convert` enables:

```
Fetch URL → streaming chunks into a temp file (cap enforced per chunk) → detect → transcode → save to --output (or derived filename)
```

This reuses the **exact same** `detect()` / `transcode()` pipeline; only the source of `raw_bytes` changes.

### 8.2 CLI Surface

```text
utf8-forge convert --url <URL> --output <PATH> [--target utf-8] [--report out.json]

# If --output omitted, derive from URL last path segment
#   https://example.com/data/legacy.cpp → legacy.cpp
#   https://example.com/foo (no extension) → foo.utf8
```

### 8.3 Streaming vs Buffering

| Mode                        | When            | Memory                                                                             |
|-----------------------------|-----------------|------------------------------------------------------------------------------------|
| `Content-Length` pre-check  | When present    | Oversized responses rejected before the body is read                               |
| Streaming (current)         | Always          | 8 KiB `Read` loop into a unique temp file; cap enforced per chunk                   |

The download streams into an exclusively created read/write temporary file
with per-chunk cap enforcement (`--max-file-size`, default 200 MiB). The chunk
buffer holds 8 KiB. Final read-back rewinds the same handle and reads up to the
received byte count, bounded by the configured cap. The handle is closed
before cleanup on success, oversize rejection and I/O errors. A creation
collision preserves the existing path. Temporary names use
`writer::unique_suffix`.

### 8.4 HTTPS & Redirects

`reqwest::blocking::Client` is built with:

- `redirect::Policy::limited(5)`
- `danger_accept_invalid_certs(false)` (default; configurable via future flag)
- Timeout 30 s (connect) / 120 s (total)

### 8.5 Security Notes

- URL is **user-supplied**; no recursive link following.
- Downloaded content is treated as **untrusted text** — detection + strict decoding prevents injection via malformed
  byte sequences.
- No shell execution.

### 8.6 Implementation Sketch

```rust
fn fetch_bytes(url: &str, max_size: u64) -> Result<Vec<u8>, ForgeError> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let mut resp = client.get(url).send()?.error_for_status()?;
    // Fast path: reject before reading when Content-Length is present.
    if let Some(len) = resp.content_length() && len > max_size {
        return Err(ForgeError::TooLarge { size: len, max: max_size });
    }
    // The helper owns exclusive creation, streaming, rewind and cleanup.
    let tmp = std::env::temp_dir().join(format!(".utf8-forge-fetch-{}", unique_suffix()));
    read_body_to_temp(&mut resp, max_size, &tmp)
}

fn derive_output_path(url: &str) -> PathBuf {
    let segment = reqwest::Url::parse(url).ok().and_then(|u| {
        u.path_segments()?.rev().find(|s| !s.is_empty()).map(str::to_owned)
    });
    match segment {
        Some(seg) if has_extension(&seg) => PathBuf::from(seg),
        Some(seg) => PathBuf::from(format!("{seg}.utf8")),
        None => PathBuf::from("download.utf8"),
    }
}

// has_extension(seg): true when the last URL segment carries a non-empty
// extension. The extension is kept verbatim; `.utf8` is appended only when
// the segment has none. See §8.2 for the normative examples.
```

---

## 9. CLI Interface (v0.1)

```text
utf8-forge <COMMAND> [OPTIONS]

COMMANDS:
  scan       Detect only; report distribution (no writes)
  convert    Detect + transcode + atomic write (supports --dry-run, --url)
  verify     Assert all whitelisted files are UTF-8 without BOM
  fix        Detect + transcode + verify in one pass (exit 3 when the gate fails)
  diff       Compare two utf8-forge JSON reports per path (any mode with files[])

SHARED TREE OPTIONS (accepted by scan / verify / convert / fix after the subcommand):
  -r, --root <PATH>            Root directory (default .)
  -e, --ext <LIST>             Override default extension whitelist
                               (an empty normalized list is fatal, exit 2)
  -x, --exclude <LIST>         Extra excluded directories
  -X, --exclude-ext <LIST>     Extra excluded extensions
      --exclude-glob <GLOB>    Extra glob exclusion over root-relative paths (dirs and files)
      --include-rc             Include .rc/.rc2/.dlg
      --include-msbuild        Include .sln/.vcxproj/...
      --include-hidden         Process dot-prefixed files/dirs
      --config <PATH>          TOML config supplying defaults (CLI > config > defaults)
      --ignore-file <PATH>     Gitignore-style exclusions (fatal when missing;
                               replaces auto-discovery of <root>/.utf8forgeignore)
      --no-ignore-file         Disable ignore-file handling entirely
      --max-file-size <BYTES>  Skip files above this size (default 200 MiB)
      --threads <N>            Rayon thread count (values below 1 clamp to 1;
                               values above 1024 are a fatal input error, exit 2)
      --report <PATH>          Write JSON report
  -q, --quiet                  Errors only
  -v, --verbose                Per-file detail

CONVERT / FIX ONLY:
  -t, --target <ENC>           utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be
                               (convert; fix accepts only the default utf-8)
      --dry-run                No writes; report only
      --backup-dir <PATH>      Backup original bytes before overwrite

REMOTE DOWNLOAD (convert only):
      --url <URL>              Fetch remote file and transcode
      --output <PATH>          Destination for --url (derived if omitted;
                               requires --url)

DIFF ONLY:
      --before <REPORT>        The earlier report
      --after <REPORT>         The later report
      --report <PATH>          Write the diff JSON here; otherwise stdout
      --fail-on-change         Exit 1 when the diff is non-empty

# --target / --dry-run / --backup-dir exist on convert and fix (fix accepts
# only --target utf-8, fatal exit 2 otherwise): scan and verify reject them
# with a clap error (exit 2). --url / --output are convert-only; --output
# requires --url and --config cannot be combined with --url (both fatal,
# exit 2). fix has no remote-fetch surface.

EXAMPLES:
  utf8-forge scan    --root D:\server --report scan.json
  utf8-forge convert --root D:\server --dry-run
  utf8-forge convert --url https://example.com/legacy.cpp --output ./legacy.cpp
  utf8-forge fix     --root D:\server --report fix.json
  utf8-forge diff    --before apply.json --after apply2.json --fail-on-change
```

Value placeholders in `--help` are the option's field name (e.g. `<ROOT>`,
`<EXCLUDE_EXT>`); this section uses descriptive names for the same values.

---

## 10. Dependencies (Latest as of 2026-11)

```toml
[package]
name = "utf8-forge"
version = "0.2.2"
edition = "2024"

[[bin]]
name = "utf8-forge"
path = "src/main.rs"

[lib]
name = "utf8_forge"
path = "src/lib.rs"

[dependencies]
anyhow = "1.0.104"          # latest
clap = { version = "4.6.7", features = ["derive"] }  # latest stable
encoding_rs = "0.8.44"           # latest
rayon = "1.12.0"           # latest
reqwest = { version = "0.13.5", features = ["blocking"], optional = true }  # latest stable; 0.13.0-rc.1 available if RC is desired
serde = { version = "1.0.229", features = ["derive"] }  # latest
serde_json = "1.0.154"          # latest
toml = { version = "1.2", default-features = false, features = ["parse", "serde"] }  # config file (§5.5)
walkdir = "2.5.0"            # latest

[dev-dependencies]
tempfile = "3.27.0"           # latest

[features]
default = ["remote"]
remote = ["reqwest"]              # disable with --no-default-features
```

**RC/Beta note**: User permits RC/beta. If desired, replace `reqwest = "0.13.5"` with `reqwest = "0.13.0-rc.1"` — but
stable `0.13.5` is already available and newer. No other crate currently has a meaningful RC/beta that improves this
tool.

### 10.1 Rust 2024 Considerations

| Change                             | Impact on this tool                   |
|------------------------------------|---------------------------------------|
| `unsafe extern` blocks             | No `extern` blocks used               |
| RPIT lifetime capture              | No RPIT-heavy code                    |
| `gen` reserved keyword             | No `gen` identifiers                  |
| Prelude adds `Future`/`IntoFuture` | No ambiguity in current traits        |
| Resolver = "3" implied             | Correct; dependencies resolve against the current toolchain |

No migration shims required.

---

## 11. Key Invariants (Maintainer Must Preserve)

| #  | Invariant                                | Violation Consequence              |
|----|------------------------------------------|------------------------------------|
| 1  | Default target = UTF-8 **without BOM**   | Breaks Playbook Strategy A         |
| 2  | UTF-16 encoders **manually prepend BOM** | Output misidentified               |
| 3  | No line-ending normalization             | Diff noise                         |
| 4  | Whitelist + blacklist double gate        | Binary/third-party corruption      |
| 5  | Byte-identical → skip write              | Spurious mtime / SVN churn         |
| 6  | Temp file + `rename` atomic write        | Source destruction on crash        |
| 7  | `had_errors == true` → hard error        | Half-written / mojibake files      |
| 8  | Report sorted by `path`                  | Non-reproducible audits            |
| 9  | No symlink recursion                     | Infinite loops / collateral damage |
| 10 | Second `convert` changes 0 bytes (double-BOM inputs converge in two passes — see the decode rules) | Non-idempotent regression          |

---

## 12. Module Layout

```
utf8-forge/
├── Cargo.toml
├── README.md          # English (default)
├── README.zh-CN.md    # Chinese
├── AGENT.md
├── EXECUTION.md
├── DESIGN.md
├── ROADMAP.md
├── docs/
│   ├── USAGE.md       # English usage guide
│   └── USAGE.zh-CN.md # Chinese usage guide
├── src/
│   ├── main.rs        # Thin entry: parse CLI, std::process::exit(run(cli))
│   ├── lib.rs         # run() dispatch, per-file pipelines (scan_file/convert_file/verify_file), report assembly, exit codes
│   ├── cli.rs         # clap definitions
│   ├── config.rs      # TOML config file (§5.5): CLI > config > defaults
│   ├── detector.rs    # detect() + DetectedEncoding
│   ├── transcoder.rs  # decode_strict / encode / transcode
│   ├── filter.rs      # Extension & directory filtering
│   ├── ignorefile.rs  # Gitignore-style exclusions (§5.4)
│   ├── walker.rs      # walkdir wrapper + candidate collection
│   ├── writer.rs      # Atomic write + backup
│   ├── fetcher.rs     # Optional remote download (reqwest)
│   ├── reporter.rs    # Console summary + JSON serialization
│   ├── differ.rs      # `diff` subcommand: per-path report comparison
│   └── error.rs       # Custom error types
└── tests/
    ├── detect.rs
    ├── transcode.rs
    ├── filter.rs
    ├── fetch.rs
    ├── end_to_end.rs
    └── remote_off.rs
```

**English comment policy**: Every public function, module, and non-trivial code block carries a `///` or `//` comment in
English. Chinese comments are permitted only in `docs/USAGE.zh-CN.md` and `README.zh-CN.md`. No mixed-language comments
in `src/`.

---

## 13. Testing Strategy

| Test                                                    | Expected                        |
|---------------------------------------------------------|---------------------------------|
| `detect(b"hello")`                                      | `Utf8`                          |
| `detect(b"\xEF\xBB\xBFhello")`                          | `Utf8Bom`                       |
| `detect(b"\xFF\xFEh\0i\0")`                             | `Utf16Le`                       |
| `detect(GBK "你好")`                                    | `Gb18030`                       |
| `detect(b"\xFF\xFF\xFF")`                               | `Unknown`                       |
| `transcode(Utf8, Utf8)`                                 | `Unchanged` (fast path)         |
| `transcode(Utf8Bom, Utf8)` twice                        | 2nd `Unchanged` (idempotent)    |
| `transcode(GBK "你好", Utf8)` → `transcode(…, Gb18030)` | Bytes identical to original GBK |
| `filter`: `foo.PNG`                                     | Not whitelisted                 |
| `filter`: `dep/`                                        | Pruned                          |
| `fetch`: mock HTTP server returns GBK                   | Transcodes to UTF-8             |
| `fetch`: oversized response                             | `Err(too_large)`                |
| `end_to_end`: mixed temp tree                           | Summary counts match            |

CI gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, and `cargo doc` with warnings denied.

---

## 14. ADR (v0.1 Deltas)

| Decision         | Choice                      | Rationale                                             |
|------------------|-----------------------------|-------------------------------------------------------|
| Edition          | **2024**                    | resolver 3; modern idioms                             |
| `reqwest`        | **0.13.5** stable           | Latest stable; RC available but unnecessary           |
| Remote download  | **Opt-in `--url`**          | Reuses pipeline; zero risk to local path              |
| Streaming        | **Adopted** (temp-file chunking, §8.3) | Memory bounded by the size cap during download |
| English comments | **Mandatory in `src/`**     | Cross-team auditability                               |
| README           | **English default + zh-CN** | Matches user request                                  |

---

*DESIGN.md v0.1 — All maintainers must read §11 before modifying code.*

