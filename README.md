# 墨简 · mojian

面向中文长篇创作的终端写作器。离线优先，**纯文本为真相**。

让你把字写进去、随时找回来、看清写了多少、在改坏之后能回退——它只做这四件事，
围绕这四件事做到位。它不是 IDE，不是 vim 插件，不做 Markdown 预览渲染。

> 它是**终端程序**：跑在你的终端模拟器里，用键盘操作。这是它最大的取舍——
> 换来的是离线、快、纯文本、可 git。如果你不在终端里工作，这个工具大概不适合你。

---

## 装上

**下载即用（推荐）。** 到 [Releases](https://github.com/TakumiShoda/MOJIAN-terminal-novel-editor/releases)
下载对应你系统的压缩包，解开就是一个 `mj`：

| 系统 | 下载 | 之后 |
|---|---|---|
| macOS（Apple Silicon） | `mj-*-aarch64-apple-darwin.tar.gz` | `tar xzf` 后把 `mj` 放进 `PATH` |
| macOS（Intel） | `mj-*-x86_64-apple-darwin.tar.gz` | 同上 |
| Linux | `mj-*-x86_64-unknown-linux-gnu.tar.gz` | 同上 |
| Windows | `mj-*-x86_64-pc-windows-msvc.zip` | 解压，运行 `mj.exe` |

> ⚠️ 二进制**未签名**。macOS 首次运行若提示「已损坏」，执行
> `xattr -d com.apple.quarantine ./mj` 解除隔离；Windows 弹 SmartScreen 时选「仍要运行」。

**有 Rust 工具链的话**，一行装：

```bash
cargo install --git https://github.com/TakumiShoda/MOJIAN-terminal-novel-editor mj-cli
```

装好后二进制叫 `mj`。

---

## 上手三步

```bash
mj doctor   # ① 先看你的终端能力（真彩色 / 键盘协议 / 剪贴板 / 字体），当场探测不假设
mj          # ② 进书架。空的？按 n 新建一本书，Enter 打开
            # ③ 开始写。随时 F1 看全部快捷键
```

工作区默认建在系统的数据目录下（`mj doctor` 会打印具体路径），
也可以 `mj --workspace ./我的书` 指到任意目录——一个目录就是一个独立工作区。

### 常用键（完整列表按 F1）

| 键 | 做什么 | 键 | 做什么 |
|---|---|---|---|
| `Ctrl+S` | 保存 | `F7` | 校对当前章 |
| `Ctrl+N` | 新建章 | `F8` | 版本历史 / 回退 |
| `F9` | 打快照 | `Alt+C` | 角色卡 |
| `F5` | 一键排版 | `F3` | 字数统计 |
| `Ctrl+F` / `Ctrl+H` | 查找 / 替换 | `F2` | 换主题 |
| `Ctrl+Z` / `Ctrl+Y` | 撤销 / 重做 | `F11` | 专注模式 |
| `Ctrl+P` | 命令面板（找得到所有功能） | `F1` | 帮助 |

记不住键就按 `Ctrl+P`——所有功能都能从命令面板搜到并触发。

### 无头子命令（不进界面，给脚本 / CI 用）

```bash
mj count [--book <id>] [--json]         # 统计字数（含标点 / 不含标点 / 纯汉字…）
mj format <path> [--check]              # 排版一个文件；--check 只报告不改，需排版则退出码非零
mj export <书名或id> --format txt|md|epub -o out.epub   # 导出
mj import 稿子.md                        # 从 Markdown 导入成一本新书
mj history list <章名或id>               # 列一章的快照链
mj config check                          # 校验配置文件
mj doctor                                # 探测终端能力
```

`mj format --check` 照 rustfmt 的规矩——干净就退 0、需要排版就退非零，直接塞进
提交钩子或 CI 就能守住全书的排版规范。指向带 `+++` 头的章节文件时只排正文、
头部原样不动。

---

## 你的稿子长什么样

磁盘布局刻意做成 **git 友好**——每章一个纯文本文件，路径稳定，没有私有二进制格式：

```
books/bk_AC48RQH5/
├── book.toml
└── volumes/010-第一卷-风起/
    ├── volume.toml
    └── chapters/0010-第一章-雪夜.md
```

章节文件（`+++` 之下逐字都是你的正文，无任何私有标记）：

```markdown
+++
id = "ch_7MCKQY74"
title = "第一章 雪夜"
status = "draft"
words = 37
+++
　　雪落了一夜。

　　他推开门，风裹着雪灌进来，冷得刺骨。
```

目录名里的中文与序号**仅供人眼**；真相是 toml 里的 `id`，创建后永不变更——
重命名、移动、排序都不影响它。哪天不想用墨简了，稿子还是一堆能用记事本打开的
`.md` 文件，谁也没绑架你。

---

## 设计原则

| 原则 | 含义 |
|---|---|
| 纯文本为真相 | 正文是 UTF-8 文本文件，用记事本 / git 也能打开、也能救。数据库只是可重建的索引缓存。 |
| 崩溃可恢复 | 任何时刻断电，最多丢失自动保存间隔内的内容，且下次启动能从崩溃转储恢复未存的缓冲。 |
| 破坏性操作先留痕 | 排版、批量替换、导入、删除，执行前强制打快照或进回收站，都能撤销。 |
| 中文优先 | 全角宽度、中文标点、中文分词、双口径字数、段首全角缩进——都是一等公民而非补丁。 |
| 能力探测而非假设 | 终端能力（字体、键盘协议、剪贴板）一律先探测再降级，永不假设；`mj doctor` 让你看得见。 |

### 关于模型校对的一点说明

校对（F7）默认只跑**本地规则**：错别字、标点、的地得、句式重复、角色名一致性——
全在你机器上，不联网。另有一个**默认关闭**的「模型校对」，会把当前章正文发给你
自己配置的大模型查病句；开启前会弹说明并要你明确同意，密钥只从环境变量读、绝不
写进配置文件。要不要用、用哪家，完全是你的选择。

---

## 从源码构建 / 参与开发

需要 Rust 1.88+（MSRV，CI 独立守护）。

```bash
cargo run --bin mj            # 直接跑
cargo build --release         # 出 release 二进制（target/release/mj）

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # 零警告是发布门禁
cargo test --workspace
```

工程结构（分层铁律见 [doc.md](doc.md) §4）：

- `mj-text` —— 纯函数文本处理，零 IO、零全局状态，这样才能大规模属性测试
- `mj-core` —— 领域模型与存储，不依赖 ratatui
- `mj-tui` —— 界面层，不含业务算法
- `mj-cli` —— 二进制入口 `mj` 与无头子命令

打标签 `v*` 会触发 [release workflow](.github/workflows/release.yml)，四平台原生编译并发布。

---

## 文档

- [doc.md](doc.md) —— 开发文档（需求 / 契约 / 验收）
- [docs/decisions/](docs/decisions/) —— ADR，记录偏离文档的决定与理由

## 许可

MIT OR Apache-2.0（见 [LICENSE-MIT](LICENSE-MIT) 与 [LICENSE-APACHE](LICENSE-APACHE)）
