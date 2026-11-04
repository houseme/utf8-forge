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
| `Utf32Le` | Starts with `FF FE 00 00` (checked **before** `FF FE`)          | Read-only: no UTF-32 write target      |
| `Utf32Be` | Starts with `00 00 FE FF` (checked **before** the UTF-16 BOMs) | Same                                   |
| `Utf16Le` | Starts with `FF FE`                                             | BOM stripped before decoding           |
| `Utf16Be` | Starts with `FE FF`                                             | Same                                   |
| `Gb18030` | UTF-8 strict fail → `GB18030.decode` with `had_errors == false` | Covers GB2312/GBK/GB18030              |
| `Unknown` | All above fail                                                  | Reported & skipped — **never guessed** |

**Detection order is frozen**: UTF-8 → GB18030, with the 4-byte UTF-32 BOMs
checked before the 2-byte UTF-16 BOMs (WHATWG-style longest-prefix precedence).
Reordering breaks idempotence on ASCII-only files. Documented ambiguity: a
UTF-16LE file whose first character is U+0000 shares the UTF-32LE BOM bytes; it
classifies as `utf-32le` and then fails strict UTF-32 decode loudly
(`decode_error`) — never silent mojibake.

### 2.2 Target Encodings

| Target            | Output bytes                                        |
|-------------------|-----------------------------------------------------|
| `utf-8` (default) | `text.as_bytes().to_vec()` — **no BOM**             |
| `utf-8-bom`       | `[EF,BB,BF] ++ text.as_bytes()` — BOM **manual**    |
| `gb18030`         | `GB18030.encode(text)`                              |
| `utf-16le`        | `[FF,FE] ++ utf16le_bytes(text)` — BOM **manual**   |
| `utf-16be`        | `[FE,FF] ++ utf16be_bytes(text)` — BOM **manual**   |

where `utf16le_bytes` / `utf16be_bytes` are `str::encode_utf16()` serialized little-endian / big-endian.

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
    if bytes.is_empty() { return DetectedEncoding::Utf8; }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { return DetectedEncoding::Utf8Bom; }
    // 4-byte UTF-32 BOMs are checked before the 2-byte UTF-16 BOMs
    // (WHATWG-style longest-prefix precedence).
    if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) { return DetectedEncoding::Utf32Le; }
    // The UTF-32BE BOM is `00 00 FE FF` (checked first, WHATWG-style); a
    // payload starting `FE FF 00 00` is UTF-16BE with a leading NUL char.
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) { return DetectedEncoding::Utf32Be; }
    if bytes.starts_with(&[0xFF, 0xFE]) { return DetectedEncoding::Utf16Le; }
    if bytes.starts_with(&[0xFE, 0xFF]) { return DetectedEncoding::Utf16Be; }
    if std::str::from_utf8(bytes).is_ok() { return DetectedEncoding::Utf8; }
    let (_, _, had_errors) = GB18030.decode(bytes);
    if !had_errors { return DetectedEncoding::Gb18030; }
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
- Hidden files (`.`-prefixed) skipped unless `--include-hidden`.

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
- No escapes — a backslash is a literal character. Matching is case-insensitive.

Limitation inherited from git: a file cannot be re-included (`!`) when its
parent directory is excluded — the walker prunes the subtree on the directory
verdict and children are never evaluated.

The ignore layer is an **independent exclusion layer** ANDed with the existing
gates: `!` negation lifts only the ignore layer, never the whitelist /
`--exclude` / `--exclude-glob` layers.

### 5.5 Config File

`--config <PATH>` loads defaults from a TOML document
(`src/config.rs`, `toml 0.9` parse+serde):

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
- Fetch mode ignores `--config`. Deliberately absent from the schema: `root`,
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

```rust
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp = unique_tmp_path(parent, path);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path)?; // atomic on same volume
    Ok(())
}
```

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

Current implementation: **streaming to a temp file with per-chunk cap
enforcement** (`--max-file-size`, default 200 MiB). At most `max_size` bytes
are ever held in memory (the final read-back). An oversized payload aborts
mid-download — the same `TooLarge` error as before, the temp file is deleted,
and nothing is buffered beyond the cap. Payloads ≤ the cap behave
byte-identically to the previous full-buffer implementation. No `fsync` on the
temp file; the temp name shares `writer::unique_suffix`.

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
    // Stream into a unique temp file, enforcing the cap per chunk: an
    // oversized payload is aborted mid-download and the temp file deleted;
    // nothing beyond the cap is ever buffered.
    let tmp = std::env::temp_dir().join(format!(".utf8-forge-fetch-{}", unique_suffix()));
    let cleanup = |p: &Path| { let _ = fs::remove_file(p); };
    let mut file = fs::File::create(&tmp)?;
    let (mut total, mut buf) = (0u64, [0u8; 8192]);
    loop {
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > max_size { cleanup(&tmp); return Err(ForgeError::TooLarge { size: total, max: max_size }); }
                file.write_all(&buf[..n])?;
            }
            Err(e) => { cleanup(&tmp); return Err(ForgeError::Fetch(format!("read body failed: {e}"))); }
        }
    }
    let body = fs::read(&tmp)?; // the only moment the payload is fully in memory
    cleanup(&tmp);
    Ok(body)
}

fn derive_output_path(url: &str) -> PathBuf {
    let segment = url::Url::parse(url).ok().and_then(|u| {
        u.path_segments()?.next_back().map(str::to_owned).filter(|s| !s.is_empty())
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

GLOBAL OPTIONS:
  -r, --root <PATH>            Root directory (default .)
  -e, --ext <LIST>             Override default extension whitelist
  -x, --exclude <LIST>         Extra excluded directories
  -X, --exclude-ext <LIST>     Extra excluded extensions
      --exclude-glob <GLOB>    Extra directory glob exclusion
      --include-rc             Include .rc/.rc2/.dlg
      --include-msbuild        Include .sln/.vcxproj/...
      --include-hidden         Process dot-prefixed files/dirs
      --config <PATH>          TOML config supplying defaults (CLI > config > defaults)
      --ignore-file <PATH>     Gitignore-style exclusions (fatal when missing;
                               replaces auto-discovery of <root>/.utf8forgeignore)
      --no-ignore-file         Disable ignore-file handling entirely
      --max-file-size <BYTES>  Skip files above this size (default 200 MiB)
      --threads <N>            Rayon thread count
      --report <PATH>          Write JSON report
  -q, --quiet                  Errors only
  -v, --verbose                Per-file detail

CONVERT / FIX ONLY:
  -t, --target <ENC>           utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be
      --dry-run                No writes; report only
      --backup-dir <PATH>      Backup original bytes before overwrite

REMOTE DOWNLOAD (convert only):
      --url <URL>              Fetch remote file and transcode
      --output <PATH>          Destination for --url (derived if omitted)

DIFF ONLY:
      --before <REPORT>        The earlier report
      --after <REPORT>         The later report
      --report <PATH>          Write the diff JSON here; otherwise stdout
      --fail-on-change         Exit 1 when the diff is non-empty

# --target / --dry-run / --backup-dir exist on convert and fix: scan and
# verify reject them with a clap error (exit 2). --url / --output are
# convert-only; fix has no remote-fetch surface.

EXAMPLES:
  utf8-forge scan    --root D:\server --report scan.json
  utf8-forge convert --root D:\server --dry-run
  utf8-forge convert --url https://example.com/legacy.cpp --output ./legacy.cpp
  utf8-forge fix     --root D:\server --report fix.json
  utf8-forge diff    --before apply.json --after apply2.json --fail-on-change
```

---

## 10. Dependencies (Latest as of 2026-11)

```toml
[package]
name = "utf8-forge"
version = "0.2.0"
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
toml = { version = "0.9", default-features = false, features = ["parse", "serde"] }  # config file (§5.5)
url = "2.5"              # for path derivation
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
    └── end_to_end.rs
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

CI gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

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

