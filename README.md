# 墨简 · mojian

面向中文长篇创作的终端写作器。离线优先，**纯文本为真相**。

> 开发中。当前进度：M1（能写字）—— 存储层已完成，编辑器 UI 施工中。

## 它做四件事

让你把字写进去、让你随时找回来、让你看清写了多少、让你在改坏之后能回退。

它不是 IDE，不是 vim 插件，不做 Markdown 预览渲染。

## 设计原则

| 原则 | 含义 |
|---|---|
| 纯文本为真相 | 正文是 UTF-8 文本文件，用记事本 / git 也能打开、也能救。数据库只是可重建的索引缓存。 |
| 崩溃可恢复 | 任何时刻断电，最多丢失自动保存间隔内的内容，且下次启动能恢复。 |
| 破坏性操作先留痕 | 排版、批量替换、导入、删除，执行前强制打快照。 |
| 中文优先 | 全角宽度、中文标点、中文分词、中文字数口径，都是一等公民而非补丁。 |
| 能力探测而非假设 | 终端能力（字体、键盘协议、剪贴板）一律先探测再降级，永不假设。 |

## 你的稿子长什么样

磁盘布局刻意做成 git 友好——每章一个文件，路径稳定，纯文本：

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

目录名里的中文与序号**仅供人眼**；真相是 toml 里的 `id`，它创建后永不变更——
重命名、移动、排序都不影响它。

## 构建

需要 Rust 1.88+（MSRV，CI 独立守护）。

```bash
cargo build
cargo test --workspace
cargo run --bin mj -- --help
```

## 开发

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # 零警告是发布门禁
cargo test --workspace
```

工程结构（分层铁律见 [doc.md](doc.md) §4）：

- `mj-text` —— 纯函数文本处理，零 IO、零全局状态，这样才能大规模属性测试
- `mj-core` —— 领域模型与存储，不依赖 ratatui
- `mj-tui` —— 界面层，不含业务算法
- `mj-cli` —— 二进制入口 `mj` 与无头子命令

## 文档

- [doc.md](doc.md) —— 开发文档（需求 / 契约 / 验收）
- [docs/decisions/](docs/decisions/) —— ADR，记录偏离文档的决定与理由

## 许可

MIT OR Apache-2.0
