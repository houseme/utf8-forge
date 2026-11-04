# docs/USAGE.zh-CN.md（中文版）

# utf8-forge — 完整使用指南

> 完整 CLI 参考、配方与运维注意事项。
> 架构与不变式见 [DESIGN.md](../DESIGN.md)。
> `D:\server` 落地见 [EXECUTION.md](../EXECUTION.md)。

---

## 1. 概要

```
utf8-forge <COMMAND> [OPTIONS]
```

命令：`scan`、`convert`、`verify`、`fix`、`diff`。

---

## 2. 命令

### 2.1 `scan`

仅检测。不写盘。输出摘要 + 可选 JSON 报告。

```bash
utf8-forge scan --root D:\server --report scan.json
```

退出码：

- `0` —— 全部归类；
- `1` —— 存在 `unknown`，或任一文件 I/O 错误；
- `2` —— 致命（root 缺失、参数错误）。

### 2.2 `convert`

检测 + 转码 + 原子写。

```bash
# 默认：→ UTF-8 无 BOM
utf8-forge convert --root D:\server

# 试运行
utf8-forge convert --root D:\server --dry-run --report dry.json

# 带备份
utf8-forge convert --root D:\server --backup-dir D:\backup\utf8-forge

# 自定义目标
utf8-forge convert --root D:\server --target gb18030

# 远程文件
utf8-forge convert --url https://example.com/a.cpp --output ./a.cpp
```

退出码：

- `0` —— 成功（无预期外错误）；
- `1` —— 存在 `unknown` / `decode_error` / `io_error`；`--url` 时还包括 `fetch_error`（HTTP/TLS/超时失败）与超限载荷；
- `2` —— 致命（参数错误、root 缺失、旗标冲突、`--backup-dir` 位于 `--root` 内或与 `--url` 同用）。

### 2.3 `verify`

断言所有白名单文件为 UTF-8 无 BOM。

```bash
utf8-forge verify --root D:\server --report verify.json
```

退出码：

- `0` —— 全部 UTF-8 无 BOM；
- `3` —— 有一个或多个不符。超限或不可读的文件计为失败（fail-closed，宁严勿漏）；
- `2` —— 致命（root 缺失、参数错误）。

### 2.4 `fix`

一次遍历完成转换 + 验证：每个候选文件先走转换流水线，随后对同一份文件列表执行验证门禁 —— 不重走目录树。

```bash
utf8-forge fix --root D:\server --report fix.json

# 预览：转换过程不写盘，但验证门禁仍只读运行 ——
# 脏树会预览为退出码 3（即当前的终态）。
utf8-forge fix --root D:\server --dry-run
```

旗标：完整树旗标集外加 `--target` / `--dry-run` / `--backup-dir`。
不支持 `--url` / `--output`（远程抓取仅限 convert）。

退出码：

- `0` —— 转换完成且所有文件通过验证门禁；
- `3` —— 门禁优先：任一文件未通过验证（与目标编码不符、`unknown`、超限、不可读 —— 全部 fail-closed）。报告中带
  `verify_failed_files`；
- `1` —— 仅有转换侧问题（`unknown` / `decode_error` / `io_error`），且无验证失败；
- `2` —— 致命（参数错误、root 缺失、`--backup-dir` 位于 `--root` 内）。

### 2.5 `diff`

按路径对比两份 utf8-forge JSON 报告。任何带 `files[]` 的报告模式均可
（`fix` 报告暴露其转换记录；verify 结果不参与对比）。

```bash
utf8-forge diff --before apply.json --after apply2.json

# CI 门禁：diff 非空 → 退出码 1
utf8-forge diff --before a.json --after b.json --fail-on-change

# 将 diff JSON 写入文件（stdout 仅输出摘要行）
utf8-forge diff --before a.json --after b.json --report diff.json
```

路径按精确字符串对比 —— 两份报告必须来自同一 root（`root` 字段不同会打印
警告并继续）。

退出码：

- `0` —— 对比完成（存在差异**不**改变退出码）；
- `1` —— 指定 `--fail-on-change` 且 diff 非空（added + removed +
  changed > 0）；
- `2` —— 输入报告缺失或格式错误。

---

## 3. 选项

| 参数                       | 默认           | 说明                                                             |
|----------------------------|----------------|------------------------------------------------------------------|
| `-r, --root <PATH>`        | `.`            | 递归根                                                           |
| `-e, --ext <LIST>`         | 见 DESIGN §5.1 | **替换**默认白名单；自动去除前导点（`.cpp` 等同 `cpp`）          |
| `-x, --exclude <LIST>`     | 见 DESIGN §5.2 | 追加到默认排除                                                   |
| `-X, --exclude-ext <LIST>` | 空             | 追加                                                             |
| `--exclude-glob <GLOB>`    | 空             | 可多次。对相对根的路径组件做大小写不敏感的 glob 匹配；以 `/` 分隔，接受 `\`；`**` 跨组件，`*`/`?` 匹配单组件内；空模式 → 退出码 2 |
| `--include-rc`             | false          | 纳入 `.rc/.rc2/.dlg`                                             |
| `--include-msbuild`        | false          | 纳入 `.sln/.vcxproj/...`                                         |
| `--include-hidden`         | false          | 处理点前缀的文件与目录                                           |
| `--config <PATH>`          | 无             | 提供默认值的 TOML 配置；CLI 旗标覆盖配置值；文件缺失/格式错误/未知键 → 退出码 2（fetch 模式忽略） |
| `--ignore-file <PATH>`     | 自动           | gitignore 风格排除；显式指定但文件缺失 → 致命（退出码 2）；指定后替换 `<root>/.utf8forgeignore` 的自动发现 |
| `--no-ignore-file`         | false          | 完全禁用 ignore 文件，包括自动发现                               |
| `-t, --target <ENC>`       | `utf-8`        | `utf-8-bom`、`gb18030`、`utf-16le`、`utf-16be`（convert、fix）   |
| `--dry-run`                | false          | 不写盘（convert、fix）                                           |
| `--backup-dir <PATH>`      | 无             | 覆盖前备份（convert、fix）                                       |
| `--max-file-size <BYTES>`  | 200 MiB        | 超限跳过                                                         |
| `--threads <N>`            | CPU 核数       | Rayon 池；`0` 视为 `1`                                           |
| `--report <PATH>`          | 无             | JSON 报告；自动创建父目录，报告路径本身会从扫描中排除            |
| `--url <URL>`              | 无             | **远程抓取（仅 convert）**；忽略 `--root`                        |
| `--output <PATH>`          | 派生           | `--url` 的目标路径                                               |
| `-q, --quiet`              | false          | 仅报错误                                                         |
| `-v, --verbose`            | false          | 逐文件细节                                                       |

**子命令旗标面**：`--target`、`--dry-run`、`--backup-dir` 由 `convert` 与
`fix` 支持。`scan` 与 `verify` 会拒绝这些旗标（clap 报错，退出码 2）。
`--url` / `--output` 仅限 convert。`diff` 只接受 `--before`、`--after`、
`--report` 与 `--fail-on-change`。

冲突（致命，退出码 2）：

- `--include-rc` + `--exclude-ext rc|rc2|dlg`
- `--include-msbuild` + `--exclude-ext sln|vcxproj|vcproj|props|targets|filters|user`
- `--ignore-file` + `--no-ignore-file`

**注意**：

- (a) `-e/--ext` **替换**默认白名单；opt-in 旗标（`--include-rc` / `--include-msbuild`）只在默认集合上追加，与 `-e` 同用时无效。
- (b) `--backup-dir` 不能与 `--url` 同用（致命，退出码 2）。
- (c) `--backup-dir` 位于 `--root` 内会被拒绝（致命，退出码 2）；目录已存在时，`--dry-run` 下同样进行位置检查。
- (d) `--threads 0` 视为 `1`。
- (e) `--url` 忽略 `--root`。
- (f) `--report` 自动创建父目录；报告路径本身会从扫描中排除。
- (g) 只读文件会被以 io 错误拒绝，绝不覆盖。
- (h) ignore 文件：显式 `--ignore-file` 缺失即致命（退出码 2）；自动发现
  `<root>/.utf8forgeignore`（未指定旗标时）缺失则静默，不可读则警告后继续；
  `--no-ignore-file` 两者皆禁。ignore 文件是独立的排除层：`!` 否定只解除
  ignore 层，绝不影响白名单 / `--exclude` / `--exclude-glob` 层。ignore 文件
  本身永远不会被扫描。
- (i) 配置文件：优先级 CLI > 配置 > 内置默认；列表值跨层**替换**（绝不拼接）。
  `--target` 与 `--max-file-size` 仅在旗标实际传入时取 CLI 值，否则取配置值，
  否则取默认值。fetch 模式忽略 `--config`。

---

## 4. 编码语义

### 4.1 检测顺序

UTF-8 BOM → UTF-32 LE BOM → UTF-32 BE BOM → UTF-16 LE BOM → UTF-16 BE BOM →
严格 UTF-8 → 严格 GB18030 → `Unknown`。

4 字节 UTF-32 BOM（`FF FE 00 00` LE、`00 00 FE FF` BE）先于 2 字节 UTF-16 BOM 检查
（WHATWG 式最长前缀优先）。已记录的歧义：首字符为 U+0000 的 UTF-16LE 文件与
之共享前导字节；这类文件会被归类为 `utf-32le`，随后在严格 UTF-32 解码时大声
失败（`decode_error`，绝不产生静默乱码）。

UTF-32 为只读：严格解码拒绝代理区码元、大于 `0x10FFFF` 的标量值，以及错位
尾部（BOM 之后载荷长度不是 4 的倍数）。**没有** UTF-32 写入目标
（`--target utf-32le` → 退出码 2），`verify` 会判定 UTF-32 文件失败
（它们不是 UTF-8）。

### 4.2 BOM 策略

- 默认输出 **无 BOM**；
- `--target utf-8-bom` 追加 `EF BB BF`；
- `--target utf-16le/be` 手动前置 `FF FE` / `FE FF`（encoding_rs 不代劳）；
- 输入 BOM 在解码时剥除；
- 仅剥除真正的首个 BOM；第二个 BOM 属于内容（U+FEFF）。

### 4.3 换行

永不归一化。CRLF 保持 CRLF；LF 保持 LF。

---

## 5. 配方

### 5.1 只处理 C/C++

```bash
utf8-forge convert --root D:\server --ext h,hpp,c,cpp,cc,inl
```

### 5.2 纳入 MSBuild

```bash
utf8-forge convert --root D:\server --include-msbuild
```

### 5.3 排除某模块

```bash
utf8-forge scan --root D:\server --exclude "GameServer201,Test_Client"
```

### 5.4 glob 排除

```bash
utf8-forge convert --root D:\server --exclude-glob "**/Generated/**"
```

### 5.5 远程 GBK → UTF-8

```bash
utf8-forge convert \
  --url https://example.com/legacy_server.cpp \
  --output D:\server\external\legacy_server.cpp \
  --target utf-8 \
  --report fetch-report.json
```

### 5.6 反向（仅遗留）

```bash
utf8-forge convert --root D:\server\legacy\scripts --target gb18030 --ext bat,cmd
```

### 5.7 ignore 文件

```bash
# 一次性：从显式文件读取 gitignore 风格排除（缺失 → 退出码 2）
utf8-forge convert --root D:\server --ignore-file D:\server\my.ignore

# 长期方案：在树根提交 .utf8forgeignore，会被自动发现
# （缺失 → 静默，不可读 → 警告后继续）
utf8-forge convert --root D:\server

# 关闭所有 ignore 来源
utf8-forge convert --root D:\server --no-ignore-file
```

`.utf8forgeignore` 示例（每行一个模式；`#` 只能整行注释）：

```text
# 注释与空行会被跳过
Generated/
legacy.cpp
/tools/tmp*
build/**/auto*.h
!keep.gen
```

- `Generated/` —— 尾部 `/` → 仅匹配目录。
- `legacy.cpp` —— 无斜杠 → 在任意深度按基名匹配。
- `/tools/tmp*` —— 前导 `/`（或任意内部 `/`）→ 锚定到相对根的路径。
- `build/**/auto*.h` —— `**` 跨组件；`*` / `?` 匹配单组件内。
- `!keep.gen` —— `!` 否定；最后命中的模式获胜。

语法是 gitignore 的子集：大小写不敏感，无转义（反斜杠是字面字符），并且与
git 一致 —— 父目录被排除后，其中的文件无法被重新包含。否定只解除 ignore
层；白名单 / `--exclude` / `--exclude-glob` 层仍然生效。

### 5.8 配置文件

```bash
# 默认值来自 TOML；传入 CLI 旗标时覆盖配置值
utf8-forge convert --root D:\server --config D:\server\forge.toml
```

`forge.toml` 示例：

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

优先级为 CLI > 配置 > 默认。列表值跨层**替换**（绝不拼接）。文件缺失、格式
错误或含未知键均为致命（退出码 2）—— 错误信息携带 TOML 行/列。
`root`、`report`、`dry-run`、`url`、`output` 与 quiet/verbose 被有意排除在
schema 之外；fetch 模式忽略 `--config`。

---

## 6. 报告

JSON schema 见 `EXECUTION.md §12`。

关键字段：

- `summary.total_files` —— 遍历到的全部文件；
- `summary.candidates` —— 命中白名单；
- `files[].action` —— 完整列表：`detected` | `passed` | `failed` | `would_convert` | `converted` | `unchanged` | `skipped_unknown` | `skipped_too_large` | `error`；
- `unknown_files` —— 检测为 `unknown`（任何模式）或 convert 中的 `skipped_unknown`；`error_files` —— action 为 `error` 或 verify `failed`（同一文件可能同时出现在两者中；fix 模式下 verify 失败仅记录在 `verify_failed_files` 中）；
- `verify_failed_files` —— 仅 fix 模式：verify 记录为 `failed` 的路径；门禁干净时省略；
- `options.ignore_file` —— 生效的 ignore 规则（显式路径原样记录，或自动发现的 `<root>/.utf8forgeignore`）；无规则生效时省略；
- `options.config_file` —— `--config` 的值原样记录；未指定时省略。

`summary` 按模式区分：

- `scan` —— 编码分布：`utf8` / `utf8_bom` / `utf16le` / `utf16be` / `utf32le` / `utf32be` / `gb18030` / `unknown` / `errors` / `too_large`（外加 `total_files` / `candidates`；`errors` 统计不可读文件，满足 buckets + errors + too_large == candidates）；
- `verify` —— 门禁结果：`passed` / `failed`；
- `convert` / `fetch` —— 结果计数：`already_utf8` / `converted` / `unknown` / `decode_errors` / `io_errors` / `too_large` / `fetch_error`；
- `fix` —— 转换计数（同上，但无 `fetch_error`）外加对同一棵树验证得到的 `passed` / `failed`。

`already_utf8` 统计在**目标**编码下字节已完全一致的文件：使用 `--target gb18030` 时，未改动的纯 ASCII 文件也计入。

---

## 7. 退出码

| 码 | 含义                                                                                      |
|----|-------------------------------------------------------------------------------------------|
| 0  | 成功（`diff`：对比完成 —— 除非指定 `--fail-on-change`，差异不改变退出码）                  |
| 1  | 非致命单文件问题（`unknown`、`decode_error`、`io_error`；convert 还包括 `fetch_error`、超限 `--url` 载荷）；`diff --fail-on-change`：两份报告存在差异 |
| 2  | 所有命令的致命错误（参数错误、root 缺失、旗标冲突、I/O 初始化失败）；clap 也会产生；`diff`：输入报告缺失或格式错误 |
| 3  | `verify` 发现白名单内非 UTF-8 文件；`fix`：任一文件未通过验证门禁（超限/不可读计为失败 —— fail-closed；门禁优先于转换侧问题） |

---

## 8. CI 集成

```yaml
- name: Verify encoding
  run: utf8-forge verify --root . --report encoding-report.json
```

`verify` 退出 3 会使流水线失败 —— 这就是设计意图。

---

## 9. 故障排查

见 `AGENT.md §9` 与 `EXECUTION.md §11`。

---

