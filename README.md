# README.md (English — default)

# utf8-forge

> A focused, idempotent, bidirectional charset transcoder for Windows source trees.
> Default output: **UTF-8 without BOM**.

[中文](README.zh-CN.md) · [Design](DESIGN.md) · [Maintainer guide](AGENT.md) · [Rollout playbook](EXECUTION.md) · [Full usage](docs/USAGE.md) · [Roadmap](ROADMAP.md)

---

## Why

Mixed-encoding source trees (GB18030 / UTF-8 / UTF-8 BOM / UTF-16) cause:

- `C4819` on English Windows / CI
- Mojibake in comments and narrow string literals
- Silent corruption when editors re-save with the wrong ACP

`utf8-forge` normalizes the tree in one idempotent pass, so you can safely enable `/utf-8` afterwards.

---

## Features

- ✅ Detect and convert **UTF-8 / UTF-8 BOM / UTF-16 LE / UTF-16 BE / UTF-32 LE / UTF-32 BE / GB18030**
- ✅ Default target: **UTF-8 without BOM** (Strategy A of the Playbook)
- ✅ **Bidirectional** conversion (GB18030 → UTF-8 → GB18030 is byte-lossless)
- ✅ **Idempotent**: second run changes 0 bytes (double-BOM inputs converge in two passes)
- ✅ **Whitelist-driven recursion** — never touches `dep/`, binaries, or build outputs
- ✅ Gitignore-style `.utf8forgeignore` exclusions (auto-discovered)
- ✅ TOML config file (`--config`) with CLI > config > defaults precedence
- ✅ **Atomic writes** — crash-safe, no truncated files
- ✅ **Parallel** per-file processing (`rayon`)
- ✅ **Optional remote download** (`--url`) reuses the same pipeline
- ✅ Human-readable + JSON reports
- ✅ Rust **2024** edition

---

## Install

```bash
# From source
cargo install --path . --locked

# Or build in-tree
cargo build --release
# Binary: target/release/utf8-forge
```

Local-only build (no `reqwest`):

```bash
cargo build --release --no-default-features
```

---

## Quick start

```bash
# 1) Scan encoding distribution
utf8-forge scan --root D:\server --report scan.json

# 2) Dry-run: see what would change
utf8-forge convert --root D:\server --dry-run --report dry.json

# 3) Real conversion with backup
utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge

# 4) Verify everything is UTF-8 without BOM
utf8-forge verify --root D:\server

# Optional: convert + verify in one pass
utf8-forge fix --root D:\server
```

**Safety rule**: `scan` → `--dry-run` → `convert`. Never skip step 2.

---

## Remote download

```bash
utf8-forge convert \
  --url https://example.com/data/legacy.cpp \
  --output ./legacy.cpp \
  --target utf-8
```

`--url` reuses the exact same detection / transcoding pipeline; only the source of bytes changes. Size is capped by
`--max-file-size` (default 200 MiB).

Notes: `--backup-dir` cannot be combined with `--url` (fatal, exit 2). An oversized payload fails with exit 1 and is
not written. When `--output` is omitted, a URL segment with an extension is kept verbatim (`…/a.cpp` → `a.cpp`); an
extension-less segment gets `.utf8` (`…/foo` → `foo.utf8`). Re-fetching identical content reports `unchanged` and skips
the write. The size cap is enforced during download — an oversized payload aborts mid-stream and nothing is written.

---

## Commands

| Command         | Purpose                                               | Writes? |
|-----------------|-------------------------------------------------------|:-------:|
| `scan`          | Encoding distribution report                          |   ❌    |
| `convert`       | Detect + transcode + atomic write                     |   ✅    |
| `convert --url` | Fetch remote file and transcode                       |   ✅    |
| `verify`        | Assert all whitelisted files are UTF-8 no BOM         |   ❌    |
| `fix`           | Convert + verify in one pass (exit 3 on gate failure) |   ✅    |
| `diff`          | Compare two JSON reports per path                     |   ❌    |

See [docs/USAGE.md](docs/USAGE.md) for the full CLI reference and recipes.

---

## Default whitelist (excerpt)

C/C++ (`.c .cc .cpp .cxx .h .hh .hpp .hxx .inl .ipp .tpp`), .NET (`.cs`), JVM (`.java .kt`), Rust (`.rs`), Go (`.go`),
Python (`.py`), JS/TS (`.js .jsx .ts .tsx`), Shell (`.sh .bash .ps1 .bat`), Config (`.json .yaml .toml .ini .xml`), Docs
(`.md .txt .rst`), Protocol (`.proto`).

**Not** included by default: `.rc`, `.sln`, `.vcxproj` — enable with `--include-rc` / `--include-msbuild`.

Excluded by default: `dep/`, `_tools/`, `.svn/`, `.git/`, `build/`, `temp/`, `OutputFile*/`, `node_modules/`,
`__pycache__/`, etc.

---

## License

Apache License, Version 2.0 only.
Copyright © 2026 [houseme](https://github.com/houseme). See [LICENSE](LICENSE) for the full text.