# ADR 0003：确认支持 Windows，并在 M0 处理 CRLF 与路径

日期：2026-07-16
状态：已接受

## 决定

doc.md §12.5 开放问题 2 已确认：**目标平台包含 Windows**。

文档同时警告「包含的话 M0 就要处理 CRLF 与路径，不要留到最后」。故在 M1 的
`Store` 写任何一行存储代码之前，先把行尾与路径的契约钉死——这两项一旦被存储层
依赖，再改就要动所有存储测试与既有用户数据。

## 行尾（EOL）

契约照 doc.md §9：**读入统一 LF，写出按 `[general] line_ending = "lf" | "native"`，默认 `lf`。**

实现在 `mj-text/src/eol.rs`（纯函数，无 IO，符合 §4 分层铁律）。

为什么内存里必须只有 LF：字节偏移贯穿全项目——光标、`Edit::range`、
`Issue::range`、diff hunk、搜索命中。内存里混入 CRLF，同一段文字在 Windows 与
macOS 上偏移就不同，所有跨平台位置计算全部错位。CRLF 只允许存在于
「刚读进的字节」与「即将写出的字节」两个瞬间。

为什么默认 `lf` 而非 `native`：正文要对 git 友好（§5.1 明确把布局做成 git 友好），
`native` 会让同一份稿子在平台间往返时产生「整文件都变了」的假 diff。
Windows 用户需要 CRLF 时显式设 `native`。

配套：`.gitattributes` 强制 `eol=lf`，CI 在 Windows 上关掉 `core.autocrlf`。
否则 git 会替我们改行尾，测试测的就成了 git 的行为而非本项目的代码。

## 路径

§5.1 的目录名是「排序号 + slug」，slug 由**用户输入的标题**派生。Windows 的
文件名限制远严于 Unix，若不处理，用户把章节命名为「第一章：雪夜」就会在
Windows 上创建失败——而这是完全正常的中文标题。

实现在 `mj-core/src/slug.rs`，对所有平台统一施加最严格规则（这样 workspace
能在平台间直接搬运）：

- ASCII 保留字符 `< > : " / \ | ? *`、控制字符、空白 → `-`，连续的压成一个
- 结尾的点与空格剥除（Windows 会静默剥离，导致名字与预期不符）
- 保留设备名（CON/PRN/AUX/NUL/COM1-9/LPT1-9，含 `con.md` 这种带扩展名的形式）加 `_` 前缀
- 按字节截断到 100，且不切开 UTF-8 字符

**保留中文与全角标点**：`第一章：雪夜`（全角冒号 U+FF1A）在所有平台都合法，
且比 `第一章-雪夜` 可读。slug 是给人眼看的（§5.1 明言真相在 toml），
故只替换真正危险的 ASCII 字符。写这条测试时我一度断言全角冒号也该被替换，
是测试失败纠正了我——那会白白牺牲可读性。

## 单实例锁

`process_alive` 原先在非 Unix 平台无条件返回 `true`，意味着 **Windows 上崩溃一次
就永久锁死自己的 workspace**。Windows 既然是目标平台，这就是实打实的缺陷。

改为用 `OpenProcess` + `GetExitCodeProcess` 真实探测。注意：仅判断
`OpenProcess` 是否成功不够——进程结束后只要还有句柄未关闭，它仍会成功。
必须再查退出码是否为 `STILL_ACTIVE`。

## 原子写在 Windows 上是否成立

存疑点：Unix 的 `rename` 覆盖已存在文件是原子的，而「Windows 上 rename 不能覆盖」
是常见说法。若属实，`atomic::write` 在 Windows 上每次保存都会失败。

**核对了 std 源码**（`library/std/src/sys/fs/windows.rs`），Rust 走的是
`MoveFileExW(..., MOVEFILE_REPLACE_EXISTING)`，**确实覆盖**。故无需为 Windows
单开「先删再改名」的分支——那反而会制造一个「文件已删、改名未成」的丢稿窗口。

目录 fsync 在 Windows 上跳过（无法 open 目录），依赖 NTFS 元数据日志。

## 验证状态

- **已本地验证**：EOL 归一化 10 项单元测试 + 5 项 proptest 属性（幂等、无 CR 残留、
  往返稳定、行数守恒、非行尾字符不变）；slug 12 项测试；`line_ending` 配置项解析
  与非法值拒绝。全部在 macOS 上跑通。
- **已交叉验证**：`lock.rs` 的 Windows 分支用 `--target x86_64-pc-windows-msvc`
  单独类型检查通过。此举当场抓到一个真错误：`STILL_ACTIVE` 在 windows-sys 里是
  裸 `i32` 而非 newtype，我原先写的 `.0` 无法编译——只读代码是发现不了的。
- **已由 CI 兑现（2026-07-16）**：windows-latest 上 fmt / clippy / **125 项测试全部通过**。
  本 ADR 原先记录的最大未验证面——「Windows 上的真实行为只能由 CI 证明」——已兑现：
  原子写（`MoveFileExW` 覆盖语义）、`OpenProcess` 陈旧锁探测、EOL 归一化、
  slug 路径安全，均在真实 Windows 上跑通。

### CI 首次运行抓到的问题（值得记录）

首次推送时 windows-latest **失败**，但失败原因不在上述任何一处，而是
clippy `result_large_err`：`Error` 枚举因内嵌 96 字节的 `toml::de::Error`
达到 120 字节，Windows 的 `PathBuf` 更大，把它顶过了 128 字节阈值——
macOS 上不报，Windows 上报。

这不是「Windows 特有问题」：`Result<T>` 至少和 `Error` 一样大，意味着
mj-core 里每一次**成功**返回都在搬运这些字节。成本两个平台都存在，
只是 Windows 先报出来。故装箱（120 → 56 字节）而非放宽 lint，
并把上限固化为 `tests/size.rs`，让这类问题在本机就被拦住。

教训：跨平台的差异不只在 API，也在类型布局；「本机 clippy 干净」不等于
「CI 干净」。另注意 clippy 失败会让 test 步骤被 skip——首次运行时
Windows 测试根本没跑，若只看「CI 红了」而不看是哪一步红的，很容易误判。

## 后续对 M1 的约束

- `Store::load_body` 读入后必须立刻过 `eol::normalize`，再喂给 ropey。
- `Store::save_body` 写出前按 `config.general.line_ending` 过 `eol::denormalize`。
- 一切由用户标题派生的路径分量必须过 `slug::slugify`。
- 集成测试需覆盖「CRLF 文件读入 → 编辑 → 写出」的往返。
