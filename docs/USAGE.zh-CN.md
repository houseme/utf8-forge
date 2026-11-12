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

仅检测。不写盘。输出摘要 + 可选 JSON 报告。每个 unknown 文件同样会在 stderr
打印一行问题提示（`unknown <path>`），与其他模式保持一致（退出码仍为 `1`）。

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

远程抓取（`--url`）说明：无法识别的载荷（检测不到匹配编码）按与树模式完全
相同的方式归类 —— action `skipped_unknown`，计入 `summary.unknown`，列入
`unknown_files`（而非 `error_files`），并在 stderr 打印一行问题提示。
写保护的目标文件与树模式一样被拒绝：退出码 1，`error` 记录
`io_error: file is write-protected (read-only); chmod +w to convert`，
目标文件保持不变。

退出码：

- `0` —— 成功（无预期外错误）；
- `1` —— 存在 `unknown` / `decode_error` / `encode_error` / `io_error`；`--url` 时还包括 `fetch_error`（HTTP/TLS/超时失败）
  与超限载荷。每个问题都会在 stderr 打印一行提示；`--url` 时解码错误针对输出路径报告；
- `2` —— 致命（参数错误、root 缺失、旗标冲突、`--backup-dir` 位于 `--root` 内或与 `--url` 同用、`--config` 与 `--url`
  同用、`--report` 位置不可写或被拒绝 —— 已存在的写保护报告文件，或（树模式）
  报告路径与生效的 ignore 文件相同）。`--url` 时
  `--report` 预检在下载开始前执行，失败即不写任何内容。

### 2.3 `verify`

断言所有白名单文件为 UTF-8 无 BOM。

```bash
utf8-forge verify --root D:\server --report verify.json
```

退出码：

- `0` —— 全部 UTF-8 无 BOM；
- `3` —— 有一个或多个不符，或遍历无法检查树的一部分。超限或不可读的文件计为失败
  （fail-closed，宁严勿漏）；即使指定 `--quiet`，遍历警告仍会打印到 stderr；
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
不支持 `--url` / `--output`（远程抓取仅限 convert）。`--target` 只接受默认值
`utf-8`：任何其他取值（CLI 或配置）都会在任何文件被触碰之前致命退出（退出码 2），
因为验证门禁断言的是无 BOM 的 UTF-8。

退出码：

- `0` —— 转换完成且所有文件通过验证门禁；
- `3` —— 门禁优先：任一文件未通过验证，或遍历无法检查树的一部分（不是无 BOM 的
  UTF-8、`unknown`、超限、不可读 —— 全部 fail-closed）。报告中带
  文件失败时报告携带 `verify_failed_files`；仅有警告的门禁失败同样以 exit 3
  结束但不携带该字段；即使指定 `--quiet`，遍历警告仍会打印到 stderr；
- `1` —— 仅有转换侧问题（`unknown` / `decode_error` / `encode_error` / `io_error`），且无验证失败；
- `2` —— 致命（参数错误、root 缺失、`--backup-dir` 位于 `--root` 内、非 `utf-8` 的 `--target`）。

### 2.5 `diff`

按路径对比两份 utf8-forge JSON 报告。输入必须是带 `files[]` 数组的
utf8-forge 报告 —— 其他任何内容（schema 不符、裸数组、非法 JSON）都是致命
退出码 2（`fix` 报告暴露其转换记录；verify 结果不参与对比）。

```bash
utf8-forge diff --before apply.json --after apply2.json

# CI 门禁：diff 非空 → 退出码 1
utf8-forge diff --before a.json --after b.json --fail-on-change

# 将 diff JSON 写入文件（摘要行始终输出到 stderr）
utf8-forge diff --before a.json --after b.json --report diff.json

# 不带 --report 时，stdout 只输出 JSON 文档：`diff … | jq .` 可直接使用
utf8-forge diff --before a.json --after b.json | jq .
```

记录路径按 **各自报告的 `root` 相对化** 后对比：先剥除 root 前缀（分隔符
归一化为 `/`）再比较，因此在不同根目录下扫描的同一棵树不再表现为整体变更。
`root` 字段不同仍会打印警告；diff 文档带有可选的 `before_root` /
`after_root` 字段（源报告没有 `root` 时省略）。

退出码：

- `0` —— 对比完成（存在差异 **不**改变退出码）；
- `1` —— 指定 `--fail-on-change` 且 diff 非空（added + removed +
  changed > 0）；
- `2` —— 输入报告缺失，输入不是带 `files[]` 数组的 utf8-forge 报告，或 `--report`
  路径被拒绝（已存在的写保护报告文件；diff 不读取 ignore 文件，因此该条规则
  在此处不适用）。

---

## 3. 选项

| 参数                       | 默认           | 说明                                                                                                                              |
|----------------------------|----------------|-----------------------------------------------------------------------------------------------------------------------------------|
| `-r, --root <PATH>`        | `.`            | 递归根                                                                                                                            |
| `-e, --ext <LIST>`         | 见 DESIGN §5.1 | **替换**默认白名单；自动去除前导点（`.cpp` 等同 `cpp`）；归一化后为空（`--ext ""`）→ 致命（退出码 2）                              |
| `-x, --exclude <LIST>`     | 见 DESIGN §5.2 | 追加到默认排除                                                                                                                    |
| `-X, --exclude-ext <LIST>` | 空             | 追加                                                                                                                              |
| `--exclude-glob <GLOB>`    | 空             | 可多次。对相对根的路径组件做大小写不敏感（仅 ASCII 折叠——非 ASCII 字节按字面比较）的 glob 匹配；以 `/` 分隔，接受 `\`；`**` 跨组件，`*`/`?` 匹配单组件内；空模式 → 退出码 2 |
| `--include-rc`             | false          | 纳入 `.rc/.rc2/.dlg`                                                                                                              |
| `--include-msbuild`        | false          | 纳入 `.sln/.vcxproj/...`                                                                                                          |
| `--include-hidden`         | false          | 处理点前缀的文件与目录                                                                                                            |
| `--config <PATH>`          | 无             | 提供默认值的 TOML 配置；CLI 旗标覆盖配置值；文件缺失/格式错误/未知键 → 退出码 2；与 `--url` 同用为致命（退出码 2，fetch 不读取配置） |
| `--ignore-file <PATH>`     | 自动           | gitignore 风格排除；显式指定但文件缺失 → 致命（退出码 2）；指定后替换 `<root>/.utf8forgeignore` 的自动发现                        |
| `--no-ignore-file`         | false          | 完全禁用 ignore 文件，包括自动发现                                                                                                |
| `-t, --target <ENC>`       | `utf-8`        | `utf-8-bom`、`gb18030`、`utf-16le`、`utf-16be`（convert；fix 仅接受默认值 `utf-8`）                                               |
| `--dry-run`                | false          | 不写盘（convert、fix）                                                                                                            |
| `--backup-dir <PATH>`      | 无             | 覆盖前备份（convert、fix）                                                                                                        |
| `--max-file-size <BYTES>`  | 200 MiB        | 超限跳过                                                                                                                          |
| `--threads <N>`            | CPU 核数       | Rayon 池；小于 `1` 的值视为 `1`；大于 `1024` 的值作为致命输入错误拒绝（退出码 2）                                                                                                |
| `--report <PATH>`          | 无             | JSON 报告；自动创建父目录，报告路径本身会从扫描中排除；报告目录不可写、报告路径是已存在目录、已存在的报告文件写保护，或报告路径与生效的 ignore 文件（`--ignore-file` / 自动发现的 `<root>/.utf8forgeignore`）相同，均会在任何处理之前致命退出（退出码 2） |
| `--url <URL>`              | 无             | **远程抓取（仅 convert）**；忽略 `--root`                                                                                         |
| `--output <PATH>`          | 派生           | `--url` 的目标路径（必须与 `--url` 同用）                                                                                         |
| `-q, --quiet`              | false          | 仅报错误；`verify` / `fix` 中，无法检查的子树产生的遍历警告仍会打印到 stderr                                                                                                                          |
| `-v, --verbose`            | false          | 逐文件细节                                                                                                                        |

> `--exclude-glob` 语义：模式必须匹配条目完整的根相对组件列表（末尾的
> `**` 吸收其余组件，也包括零个组件）——`--exclude-glob src`、`src/` 与
> `src/**` 都会排除整个 `src/` 子树（匹配到的目录连同内容一起剪枝）；
> `**/name/**` 在任意深度排除 `name/`。

**子命令旗标面**：`--target`、`--dry-run`、`--backup-dir` 由 `convert` 与
`fix` 支持（`fix` 仅接受 `--target utf-8`）。`scan` 与 `verify` 会拒绝这些
旗标（clap 报错，退出码 2）。`--url` / `--output` 仅限 convert；`--output`
必须与 `--url` 同用（clap 报错，退出码 2）。`diff` 只接受 `--before`、
`--after`、`--report` 与 `--fail-on-change`。

> `--help` 中的取值占位符是选项的字段名（如 `<ROOT>`、`<EXCLUDE_EXT>`）；
> 本文档对相同的值使用描述性名称。

冲突（致命，退出码 2）：

- `--include-rc` + `--exclude-ext rc|rc2|dlg`
- `--include-msbuild` + `--exclude-ext sln|vcxproj|vcproj|props|targets|filters|user`
- `--ignore-file` + `--no-ignore-file`
- `--config` + `--url`（fetch 不读取配置）

**注意**：

- (a) `-e/--ext` **替换**默认白名单；opt-in 旗标（`--include-rc` / `--include-msbuild`）只在默认集合上追加，与 `-e` 同用时无效。
- (b) `--backup-dir` 不能与 `--url` 同用（致命，退出码 2）。
- (c) `--backup-dir` 位于 `--root` 内会被拒绝（致命，退出码 2）；目录已存在时，`--dry-run` 下同样进行位置检查。
- (d) `--threads` 小于 `1` 的值视为 `1`；大于 `1024` 的值作为致命输入错误
  拒绝（退出码 2）。
- (e) `--url` 忽略 `--root`。
- (f) `--report` 自动创建父目录；报告路径本身会从扫描中排除。已存在的写保护
  报告文件会被拒绝（退出码 2，预检阶段，不触碰任何内容 —— 与写保护源文件同一
  哲学）；报告路径与生效的 ignore 文件（`--ignore-file` 或自动发现的
  `<root>/.utf8forgeignore`）相同样被拒绝：规则文件绝不会被报告 JSON 覆盖。
- (g) 只读文件会被以 io 错误拒绝，绝不覆盖。
- (h) ignore 文件：显式 `--ignore-file` 缺失即致命（退出码 2）；自动发现
  `<root>/.utf8forgeignore`（未指定旗标时）缺失则静默，不可读则警告后继续；
  `--no-ignore-file` 两者皆禁。ignore 文件是独立的排除层：`!` 否定只解除
  ignore 层，绝不影响白名单 / `--exclude` / `--exclude-glob` 层。ignore 文件
  本身永远不会被扫描。
- (i) 配置文件：优先级 CLI > 配置 > 内置默认；列表值跨层 **替换**（绝不拼接）。
  `--target` 与 `--max-file-size` 仅在旗标实际传入时取 CLI 值，否则取配置值，
  否则取默认值。配置中出现非法的 `[convert].target` 取值在任何树模式下都是
  致命（退出码 2）—— 包括 `scan` / `verify` —— 且错误信息会指明配置键；
  `fix` 中任何非 `utf-8` 的目标（CLI 或配置）均为致命。fetch 模式不读取
  配置：`--config` 与 `--url` 同用为致命（退出码 2）。

---

## 4. 编码语义

### 4.1 检测顺序

UTF-8 BOM → UTF-32 BE BOM（`00 00 FE FF`）→ UTF-32 LE / UTF-16 LE（共享
`FF FE` 前缀；紧随其后的两字节 `00 00` 决定 UTF-32 LE）→ UTF-16 BE →
严格 UTF-8 → 严格 GB18030 → `Unknown`。

`00 00 FE FF`（UTF-32 BE）先于 `FF FE` 检查 —— WHATWG 式最长前缀优先。
推论：以 `FF FE 00 00` 开头的文件总是按 UTF-32LE 解码 —— 首字符为 U+0000
的 UTF-16LE 编码在此处与 UTF-32LE 真正无法区分。若载荷并非同时也是合法的
UTF-32，严格 UTF-32 解码会大声失败（`decode_error`，绝不产生静默乱码）；
若是，则文件按 UTF-32 正常读取。正是这一歧义使得 `convert --target utf-16le`
拒绝首字符为 U+0000 的内容：输出会以 `FF FE 00 00` 开头，与 UTF-32LE BOM
逐字节相同，下一次运行会把文件误读为 UTF-32LE 并造成损坏。这类文件会被记录
为带 `encode_error` 原因的 `error` 行（退出码 1；原字节不受影响），计入汇总中的 `encode_errors`。
`--target utf-16be` 是安全的（`FE FF 00 00` 会按 UTF-16BE 重新读取）。

UTF-32 为只读：严格解码拒绝代理区码元、大于 `0x10FFFF` 的标量值，以及错位
尾部（BOM 之后载荷长度不是 4 的倍数）。 **没有** UTF-32 写入目标
（`--target utf-32le` → 退出码 2），`verify` 会判定 UTF-32 文件失败
（它们不是 UTF-8）。

### 4.2 BOM 策略

- 默认输出 **无 BOM**；
- `--target utf-8-bom` 追加 `EF BB BF`；
- `--target utf-16le/be` 手动前置 `FF FE` / `FE FF`（encoding_rs 不代劳）；
  `utf-16le` 拒绝首字符为 U+0000 的内容（见 §4.1 —— 输出会与 UTF-32LE BOM 撞型）；
- 输入 BOM 在解码时剥除；
- 仅剥除真正的首个 BOM；第二个 BOM 属于内容（U+FEFF）。

### 4.3 换行

永不归一化。CRLF 保持 CRLF；LF 保持 LF。

### 4.4 GB18030 目标

`--target gb18030` 使用 WHATWG 的 gb18030 编码器，它恰好有一个无法映射的
Unicode 标量： **U+E5E5**（遗留文件有时包含的私用区字符）。包含 U+E5E5 的内容
会被拒绝：文件记录为带 `encode_error` 原因的 `error` 行（退出码 1，原字节不受
影响）—— 与 UTF-16LE NUL 守卫（§4.1）是同一种拒绝模式。

WHATWG 映射同时也是规范形：对已是 GB18030 的文件执行 `--target gb18030` 会把
遗留字节写法重写为规范的 WHATWG 形式 —— 字节 `0x80` → `A2 E3`（欧元符号）、
`A3 A0` → `A1 A1`（表意空格），以及 18 个其他标量——CJK 汉字 U+9FB4–U+9FBB 与
竖排形式 U+FE10–U+FE19（其遗留字节写法曾编码私用区字符）——合并到其既定等价
字符上。
这类文件字节不同但内容等价：第一遍报告为 `converted`，从第二遍起幂等。

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

> 包含 U+E5E5 的内容会被拒绝（`error` 行，退出码 1）；已是 GB18030 的文件会在
> 第一遍被重写为规范的 WHATWG 字节形式（见 §4.4）。

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

语法是 gitignore 的子集：大小写不敏感（仅 ASCII 折叠——非 ASCII 字节按字面
比较），无转义（反斜杠是字面字符），并且与 git 一致 —— 父目录被排除后，
其中的文件无法被重新包含。折叠由测试断言固定为仅覆盖 ASCII 字母：模式中的
非 ASCII 字符只匹配其字面本身。否定只解除 ignore 层；白名单 / `--exclude` /
`--exclude-glob` 层仍然生效。

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

优先级为 CLI > 配置 > 默认。列表值跨层 **替换**（绝不拼接）。文件缺失、格式
错误或含未知键均为致命（退出码 2）—— 错误信息携带 TOML 行/列。取值校验与 CLI
一致：`[filter].extensions = []`（键存在但为空）与 `--ext ""` 一样致命
（退出码 2）—— 键缺省时保留默认白名单；`[convert].backup_dir = ""` 为致命，
错误信息会指明该键。
`root`、`report`、`dry-run`、`url`、`output`、`ignore_file` 与 quiet/verbose
被有意排除在 schema 之外（`ignore_file` 出现在 TOML 中即致命未知键）；
`--config` 与 `--url` 同用同样致命 —— fetch 模式不读取配置。

---

## 6. 报告

JSON schema 见 `EXECUTION.md §12`。

关键字段：

- `summary.total_files` —— 产生了记录的白名单文件（walker 只会发出白名单文件）；
- `summary.candidates` —— 实际处理的文件数：scan / convert / fetch / fix 中为
  `total_files − too_large`；verify 中等于 `total_files`（超限文件由门禁判为
  `failed`，从不做减法）；
- `files[].action` —— 完整列表：`detected` | `passed` | `failed` | `would_convert` | `converted` | `unchanged` |
  `skipped_unknown` | `skipped_too_large` | `error`；
- `unknown_files` —— 检测为 `unknown`（任何模式）或 convert、fetch、fix 中的 `skipped_unknown`；`error_files` —— action 为 `error` 或
  verify `failed`（同一文件可能同时出现在两者中；fix 模式下 verify 失败仅记录在 `verify_failed_files` 中）；
- `verify_failed_files` —— 仅 fix 模式：verify 记录为 `failed` 的路径；仅在有文件实际失败时携带（仅有警告的门禁失败以 exit 3 结束但不携带）；
- `warnings` —— 顶层字符串数组，携带遍历/ignore 警告（例如无法进入的目录、不可读
  的 ignore 文件）；为空时省略；
- `options.ignore_file` —— 生效的 ignore 规则（显式路径原样记录，或自动发现的 `<root>/.utf8forgeignore`）；无规则生效时省略；
- `options.config_file` —— `--config` 的值原样记录；未指定时省略。

`summary` 按模式区分：

- `scan` —— 编码分布：`utf8` / `utf8_bom` / `utf16le` / `utf16be` / `utf32le` / `utf32be` / `gb18030` / `unknown` /
  `errors` / `too_large`（外加 `total_files` / `candidates`；`errors` 统计不可读文件，因此满足
  `buckets + errors == candidates`（等价于 `buckets + errors + too_large == total_files`））；
- `verify` —— 门禁结果：`passed` / `failed`；
- `convert` / `fetch` —— 结果计数：`already_utf8` / `converted` / `unknown` / `decode_errors` / `encode_errors` /
  `io_errors` / `too_large` / `fetch_error`；
- `fix` —— 转换计数（同上，但无 `fetch_error`）外加对同一棵树验证得到的 `passed` / `failed`。

`already_utf8` 统计在 **目标**编码下字节已完全一致的文件：使用 `--target gb18030` 时，未改动的纯 ASCII 文件也计入。携带遗留字节写法的 GB18030 文件 **并非**字节一致：第一遍 `--target gb18030` 会将其规范化（§4.4）并报告 `converted`；从第二遍起才视为无变化。

---

## 7. 退出码

| 码 | 含义                                                                                                                                                  |
|----|-------------------------------------------------------------------------------------------------------------------------------------------------------|
| 0  | 成功（`diff`：对比完成 —— 除非指定 `--fail-on-change`，差异不改变退出码）                                                                             |
| 1  | 非致命单文件问题（`unknown`、`decode_error`、`encode_error`、`io_error`；convert 还包括 `fetch_error`、超限 `--url` 载荷）；`diff --fail-on-change`：两份报告存在差异 |
| 2  | 所有命令的致命错误（参数错误、root 缺失、旗标冲突、I/O 初始化失败）；clap 也会产生；`diff`：输入报告缺失或格式错误                                    |
| 3  | `verify` 发现白名单内非 UTF-8 文件（或无法检查树的一部分）；`fix`：任一文件未通过验证门禁（超限/不可读计为失败 —— fail-closed；门禁优先于转换侧问题）                         |

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
