# utf8-forge

[![Crates.io](https://img.shields.io/crates/v/utf8-forge.svg)](https://crates.io/crates/utf8-forge)
[![Documentation](https://docs.rs/utf8-forge/badge.svg)](https://docs.rs/utf8-forge)
[![CI](https://github.com/houseme/utf8-forge/actions/workflows/ci.yml/badge.svg)](https://github.com/houseme/utf8-forge/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](#rust-工具链兼容性)

> 面向 Windows 源码树的 **聚焦、幂等、双向**字符集转换器。
> 默认输出： **UTF-8 无 BOM**。

[English](README.md) · [架构设计](DESIGN.md) · [维护指南](AGENT.md) · [落地手册](EXECUTION.md) · [完整用法](docs/USAGE.zh-CN.md) · [路线图](ROADMAP.md)

---

## 为什么需要它

混杂编码的源码树（GB18030 / UTF-8 / UTF-8 BOM / UTF-16）会带来：

- 英文 Windows / CI 上出现 `C4819`；
- 注释与窄字符串字面量乱码；
- 编辑器按错误 ACP 保存导致的静默损坏。

`utf8-forge` 一次幂等转换把整棵树统一，之后即可安全打开 `/utf-8`。

---

## 特性

- ✅ 识别并转换 **UTF-8 / UTF-8 BOM / UTF-16 LE / UTF-16 BE / GB18030**（UTF-32 LE/BE：仅识别与读取，不可作为写入目标）
- ✅ 默认目标： **UTF-8 无 BOM**（Playbook 策略 A）
- ✅ **双向**转换（GB18030 → UTF-8 → GB18030 字节无损；`--target utf-16le` 拒绝首字符为 U+0000 的内容 —— 其输出会与
  UTF-32LE BOM 逐字节相同，永远无法再按 UTF-16 读取）
- ✅ **幂等**：第二次运行改动 0 字节（双 BOM 输入需两次转换才收敛）
- ✅ **白名单驱动递归** —— 绝不触碰 `dep/`、二进制、构建产物
- ✅ gitignore 风格的 `.utf8forgeignore` 排除（自动发现）
- ✅ TOML 配置文件（`--config`），优先级 CLI > 配置 > 默认
- ✅ **原子写** —— 崩溃安全，不产生半截文件
- ✅ **并行**文件处理（`rayon`）
- ✅ **可选远程下载**（`--url`）复用同一流水线
- ✅ 人类可读 + JSON 双报告
- ✅ Rust **2024** edition

---

## 安装

```bash
# 从 crates.io 安装
cargo install utf8-forge

# 从源码安装
cargo install --path . --locked

# 或在项目内构建
cargo build --release
# 产物：target/release/utf8-forge
```

纯本地版（不编译 `reqwest`）：

```bash
cargo build --release --no-default-features
```

---

## 快速开始

```bash
# 1) 盘点编码分布
utf8-forge scan --root D:\server --report scan.json

# 2) 试运行：看会改哪些文件
utf8-forge convert --root D:\server --dry-run --report dry.json

# 3) 正式转换（带备份）
utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge

# 4) 验证全部为 UTF-8 无 BOM
utf8-forge verify --root D:\server

# 可选：一次遍历完成转换 + 验证
utf8-forge fix --root D:\server
```

**安全规则**：`scan` → `--dry-run` → `convert`。第 2 步不可跳过。

---

## 远程下载

```bash
utf8-forge convert \
  --url https://example.com/data/legacy.cpp \
  --output ./legacy.cpp \
  --target utf-8
```

`--url` 完全复用检测 / 转码流水线，仅字节来源不同。下载大小由 `--max-file-size` 兜底（默认 200 MiB）。

注意：`--backup-dir` 不能与 `--url` 同用（致命，退出码 2）。超限载荷以退出码 1 失败，且不会写盘。省略 `--output`
时，URL 末段若带扩展名则原样保留（`…/a.cpp` → `a.cpp`）；不带扩展名的末段追加 `.utf8`（`…/foo` → `foo.utf8`）。
重复抓取相同内容会报告 `unchanged` 并跳过写盘。大小上限在下载过程中逐块强制执行 —— 超限载荷会在流式中途中止，
不写任何内容。

---

## 命令一览

| 命令            | 作用                                    | 写盘 |
|-----------------|-----------------------------------------|:----:|
| `scan`          | 编码分布报告                            |  ❌  |
| `convert`       | 检测 + 转码 + 原子写                    |  ✅  |
| `convert --url` | 抓取远程文件并转码                      |  ✅  |
| `verify`        | 断言所有白名单文件为 UTF-8 无 BOM       |  ❌  |
| `fix`           | 一遍完成转换 + 验证（门禁失败退出 3）   |  ✅  |
| `diff`          | 按路径对比两份 JSON 报告                |  ❌  |

完整 CLI 参考与配方见 [docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md)。

---

## 默认白名单（节选）

C/C++（`.c .cc .cpp .cxx .h .hh .hpp .hxx .inl .ipp .tpp`）、.NET（`.cs`）、JVM（`.java .kt`）、Rust（`.rs`）、Go（`.go`）、Python（
`.py`）、JS/TS（`.js .jsx .ts .tsx`）、Shell（`.sh .bash .ps1 .bat`）、配置（`.json .yaml .toml .ini .xml`）、文档（`.md .txt .rst`
）、协议（`.proto`）。

**默认不含**：`.rc`、`.sln`、`.vcxproj` —— 用 `--include-rc` / `--include-msbuild` 显式开启。

默认排除：`dep/`、`_tools/`、`.svn/`、`.git/`、`build/`、`temp/`、`OutputFile*/`、`node_modules/`、`__pycache__/` 等。

---

## Rust 工具链兼容性

`utf8-forge` 跟随 **Rust stable**（edition 2024）。实现使用了需要较新工具链的稳定语言特性 —— **stable Rust 1.88
及以上**均可构建。不固定 MSRV：跟随当前 stable 而非维护最低版本保证，CI 在每次推送时以最新 stable 工具链验证。

---

## 许可证

本项目仅基于 **Apache License, Version 2.0** 发布。
版权所有 © 2026 [houseme](https://github.com/houseme)。完整许可文本见 [LICENSE](LICENSE)。