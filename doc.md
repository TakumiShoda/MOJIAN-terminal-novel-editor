# 终端小说写作器 —— 开发文档

> 项目代号：**墨简（mojian）**，二进制名 `mj`（占位，可改）
> 目标读者：负责实现本项目的编程 agent
> 文档版本：v1.0

---

## 0. 给实现者的使用说明

- 本文档按"需求 → 契约 → 验收"的方式组织。每个模块给出**数据结构、函数签名、行为契约、验收标准**。签名是约束，不是建议；如需偏离，先在 `docs/decisions/` 下写一条 ADR 说明理由。
- 标记含义：
  - `[MUST]` 必须实现，属于验收项。
  - `[SHOULD]` 应当实现，可延后到指定里程碑。
  - `[VERIFY]` 文档中的技术判断需要在实现时用真实环境验证（尤其是终端能力相关），不要直接当事实写死。
- **禁止事项**（违反即视为实现错误）：
  1. 禁止任何可能静默丢失用户正文的路径。所有写盘走"临时文件 + fsync + 原子 rename"。
  2. 禁止在 TUI 运行期间向 stdout/stderr 打印任何内容（会撕裂界面）。日志一律进文件。
  3. 禁止把 SQLite 当作正文的唯一真相来源。正文的真相永远是磁盘上的纯文本文件。
  4. 禁止把"排版""批量替换"这类破坏性操作做成不可撤销、不可预览。
  5. 禁止按字节或 `char` 移动光标；一律按 grapheme cluster。

---

## 1. 产品定位与设计原则

一个**离线优先、纯文本为真相、面向中文长篇创作**的终端写作器。它不是 IDE，不是 vim 插件，不做 Markdown 预览渲染。它做四件事：让你把字写进去、让你随时找回来、让你看清写了多少、让你在改坏之后能回退。

设计原则：

| 原则 | 含义 |
|---|---|
| 纯文本为真相 | 正文是 UTF-8 文本文件，用户用记事本 / git 也能打开、也能救。数据库只是可重建的索引缓存。 |
| 崩溃可恢复 | 任何时刻断电，最多丢失自动保存间隔内的内容，且下次启动能恢复。 |
| 破坏性操作先留痕 | 排版、批量替换、导入、删除，执行前强制打快照。 |
| 中文优先 | 全角宽度、中文标点、中文分词、中文字数口径，都是一等公民而非补丁。 |
| 能力探测而非假设 | 终端能力（字体、键盘协议、剪贴板）一律先探测再降级，永不假设。 |

---

## 2. 三个必须先决的技术现实

这一节是设计约束的来源，实现前必读。

### 2.1 字体切换：终端程序基本无权改字体

TUI 程序运行在宿主终端里，字体由终端模拟器决定。可行性大致如下 `[VERIFY]`：

| 终端 | 可行手段 | 能力 |
|---|---|---|
| xterm / urxvt 系 | OSC 50（`ESC ] 50 ; <font> BEL`） | 改字体族，部分改字号 |
| kitty | 远程控制（`kitty @ set-font-size`），需终端侧开启 `allow_remote_control` | 仅字号 |
| WezTerm | 用户变量 / lua 配置，运行时改字体族需要终端侧配合 | 有限 |
| Alacritty / Windows Terminal / VS Code 内置终端 | 无 | 无 |

**结论**：`字体切换` 不能设计成"一个开关"。设计为 `FontController` 多后端 + 能力位 + 三级降级：

1. **一级（能改）**：探测到支持的终端 → 直接切换字体族/字号，退出时恢复。
2. **二级（不能改字体，但能改观感）**：提供**外观预设（Appearance Preset）**——主题配色、正文栏宽、段间距、左右留白、行内强调渲染（粗/暗/斜）。这是绝大多数用户实际感知到的"换字体"。
3. **三级（什么都不能改）**：一键生成对应终端的配置片段（kitty.conf / wezterm.lua / alacritty.toml 的字体段），提示用户粘贴后重启终端。

UI 上把这个功能命名为 **「外观」** 而不是「字体」，字体只是其中一栏，并在不支持时显示灰态与原因说明。

### 2.2 中文校对：能力分层，不要过度承诺

- **规则引擎能可靠做的**：标点配对（引号/括号/书名号）、标点误用、流水句（连续逗号数）、句长分布、词语重复率、专名一致性、自定义混淆词表命中。
- **规则引擎做不好的**：成分残缺、搭配不当、语义重复、时态/逻辑错误——即用户说的"病句"。规则引擎在这类任务上误报率极高，会导致用户直接关掉整个功能。
- **错别字**：Rust 生态无成熟中文纠错库。方案是 jieba-rs 分词 + 自带混淆集（confusion set）+ 用户词典。角色名、地名、法术名等专名必须注入用户词典，否则会被切碎并大量误报。

**结论**：定义 `Proofreader` trait，三个实现：`RuleProofreader`（本地，默认开）、`ExternalProofreader`（调用外部命令，JSON 契约）、`LlmProofreader`（远程，默认关，需用户填 key）。病句主要依赖后两者。所有 Issue 必须带 `source` 字段，UI 上区分"本地规则"和"模型建议"。

### 2.3 终端输入与宽字符

- **输入法**：中文由终端/系统输入法处理，程序收到的是**上屏后的最终文本**，可能是多字节字符事件或粘贴事件。不要试图自己处理候选框。`[MUST]` 开启 bracketed paste 模式，粘贴走一次性事件而非逐字符事件（否则粘贴 3000 字会触发 3000 次重排）。
- **宽度**：CJK 字符终端宽度为 2。`[MUST]` 所有布局计算用 `unicode-width`，光标移动/删除用 `unicode-segmentation` 的 grapheme cluster。
- **Kitty 键盘协议**：可选开启（能区分 `Ctrl+I` 与 `Tab` 等），但需运行时探测，不支持时静默降级。

---

## 3. 技术栈

MSRV：**Rust 1.88**（ratatui 0.30 要求）`[VERIFY]`

```toml
# 版本以实现时 crates.io 最新稳定版为准，下列为已知基线
ratatui              = "0.30"      # TUI 框架
crossterm            = "0.29"      # 终端后端（ratatui 已再导出，勿重复引入不同版本）
ropey                = "1"         # rope 文本缓冲，大章节 O(log n) 编辑
unicode-width        = "0.2"       # 显示宽度
unicode-segmentation = "1"         # grapheme cluster
similar              = "2"         # diff（Myers），支持 char/word/line 粒度
jieba-rs             = "0.7"       # 中文分词
regex                = "1"         # 查找替换（默认引擎）
fancy-regex          = "0.14"      # 可选：需要 lookaround 时
serde                = { version = "1", features = ["derive"] }
toml                 = "0.8"       # 配置与元数据
serde_json           = "1"         # 历史日志、外部校对契约
rusqlite             = { version = "0.32", features = ["bundled"] }  # 索引缓存
zstd                 = "0.13"      # 快照压缩
blake3               = "1"         # 内容寻址哈希
thiserror            = "2"
anyhow               = "1"
tracing              = "0.1"
tracing-appender     = "0.2"
tracing-subscriber   = "0.3"
directories          = "5"         # 跨平台数据目录
ureq                 = "2"         # LLM 后端 HTTP（阻塞，跑在工作线程）
chrono               = "0.4"

[dev-dependencies]
insta     = "1"      # 快照测试
proptest  = "1"      # 属性测试（排版幂等性）
criterion = "0.5"    # 基准
tempfile  = "3"
```

**不引入 tokio**。并发用 `std::thread` + `std::sync::mpsc`，够用且避免运行时复杂度。校对/LLM/索引重建跑在工作线程，通过 channel 把结果送回主循环。

**剪贴板**：优先 OSC 52（支持 SSH 场景），失败时降级到系统剪贴板 crate，再失败则内部剪贴板 `[SHOULD]`。

---

## 4. 工程结构

```
mojian/
├── Cargo.toml                 # workspace
├── crates/
│   ├── mj-core/               # 领域模型、存储、版本历史。不依赖 ratatui
│   │   ├── src/
│   │   │   ├── model.rs       # Book/Volume/Chapter/Character
│   │   │   ├── store.rs       # 磁盘读写、原子写、扫描
│   │   │   ├── index.rs       # SQLite 索引（可重建）
│   │   │   ├── history.rs     # 快照、保留策略、diff
│   │   │   └── id.rs
│   ├── mj-text/               # 纯函数文本处理。不依赖 IO
│   │   ├── src/
│   │   │   ├── count.rs       # 字数统计
│   │   │   ├── format.rs      # 一键排版
│   │   │   ├── search.rs      # 查找替换
│   │   │   ├── proof/         # 校对
│   │   │   │   ├── mod.rs     # Proofreader trait, Issue
│   │   │   │   ├── rules.rs   # 本地规则引擎
│   │   │   │   ├── external.rs
│   │   │   │   └── llm.rs
│   │   │   └── width.rs       # CJK 宽度/grapheme 工具
│   │   └── tests/
│   ├── mj-tui/                # ratatui 界面层
│   │   ├── src/
│   │   │   ├── app.rs         # 应用状态机
│   │   │   ├── event.rs       # 事件循环
│   │   │   ├── keymap.rs
│   │   │   ├── theme.rs
│   │   │   ├── font.rs        # FontController 后端
│   │   │   ├── editor/        # 编辑器组件（视口、光标、软换行、undo）
│   │   │   └── screens/       # shelf / tree / editor / diff / proof / character / settings
│   └── mj-cli/                # 二进制入口 + 少量无头子命令（count/format/export）
└── docs/
    └── decisions/
```

**分层铁律**：`mj-text` 全部是纯函数，输入 `&str`/`&Rope` 输出结果，零 IO、零全局状态——这样才能大规模属性测试。`mj-core` 负责 IO 与领域。`mj-tui` 只做状态与渲染，不含业务算法。

---

## 5. 数据模型与磁盘布局

### 5.1 目录布局

```
<workspace>/                          # 默认 ~/.local/share/mojian（可 --workspace 指定）
├── config.toml                       # 全局配置
├── library.toml                      # 书架索引（书的顺序、置顶、最近打开）
├── dict/
│   ├── user.txt                      # 用户词典（专名，注入 jieba）
│   ├── confusion.tsv                 # 混淆集（可覆盖内置）
│   └── ignore.json                   # 已忽略的校对问题（按 hash）
├── logs/mj.log
└── books/
    └── <book-id>/                    # book-id: 8位 base32 随机，永不变
        ├── book.toml                 # 书元数据
        ├── volumes/
        │   └── 010-diyi-juan/        # 目录名 = 排序号(3位) + slug，仅供人眼；真相在 toml
        │       ├── volume.toml
        │       └── chapters/
        │           ├── 0010-kaipian.md
        │           └── 0020-xiangyu.md
        ├── characters/
        │   └── <char-id>.toml
        ├── history/
        │   ├── objects/ab/cdef...zst # 内容寻址快照（blake3 前两位分桶 + zstd）
        │   └── refs/<chapter-id>.json# 该章的快照链（有序，含元数据）
        ├── trash/                    # 软删除区，30 天后可清理
        └── .index.sqlite             # 可重建缓存，加入 .gitignore
```

**关于 git**：布局刻意做成 git 友好（文本、稳定路径、每章一文件）。不内置 git，但文档里告诉用户可以自己 `git init`。`.index.sqlite` 和 `history/` 默认写入自动生成的 `.gitignore`。

### 5.2 章节文件格式

章节文件是**带 YAML front matter 的纯文本**。正文部分绝不包含任何私有标记——用户拿去别处必须能直接用。

```markdown
---
id: ch_7Q2M4KZA
title: 第一章 雪夜
status: draft        # draft | revised | done
created: 2026-07-16T10:00:00+09:00
updated: 2026-07-16T12:30:00+09:00
words: 3128          # 缓存值，以实际正文为准，不一致时重算
tags: [伏笔]
---
　　雪落了一夜。
```

`[MUST]` 解析器要容忍：无 front matter（视为纯正文，首次保存时补写）、字段缺失（用默认值）、字段多余（保留原样回写，不得丢弃未知字段）。

### 5.3 核心类型

```rust
// mj-core/src/model.rs

/// 稳定 ID，创建后永不变更。重命名/移动/排序都不影响它。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Id<T> { raw: [u8; 8], _t: PhantomData<T> }
pub type BookId = Id<BookTag>;
pub type VolumeId = Id<VolumeTag>;
pub type ChapterId = Id<ChapterTag>;
pub type CharacterId = Id<CharacterTag>;

pub struct Book {
    pub id: BookId,
    pub title: String,
    pub author: String,
    pub synopsis: String,
    pub genre: Vec<String>,
    pub target_words: Option<u64>,     // 全书目标字数
    pub created: DateTime<Local>,
    pub updated: DateTime<Local>,
    pub volumes: Vec<Volume>,          // 有序
    pub extra: toml::Table,            // 未知字段透传，回写时保留
}

pub struct Volume {
    pub id: VolumeId,
    pub title: String,
    pub order: u32,                    // 稀疏排序，步长 10，插入时取中值，避免整体重排
    pub synopsis: String,
    pub chapters: Vec<ChapterMeta>,    // 有序；正文懒加载
}

pub struct ChapterMeta {
    pub id: ChapterId,
    pub title: String,
    pub order: u32,
    pub status: ChapterStatus,
    pub word_count: WordCount,         // 缓存
    pub tags: Vec<String>,
    pub path: PathBuf,
    pub updated: DateTime<Local>,
}

/// 正文按需加载，卸载时释放
pub struct ChapterBody { pub id: ChapterId, pub text: Rope, pub dirty: bool }
```

**排序用稀疏 order**：新建/移动章节时不重写整卷的 order，只取相邻两者的中值；中值耗尽时才触发一次整卷 renumber（步长恢复 10）。这避免了拖动一章要改写四百个文件。

### 5.4 索引（SQLite，可重建）

用途：全书搜索、字数汇总、码字量统计、校对结果缓存。**启动时如果 schema 版本不符或文件损坏，直接删掉重建，不得报错阻塞用户**。

```sql
CREATE TABLE chapter_index (
  chapter_id TEXT PRIMARY KEY, book_id TEXT, volume_id TEXT,
  title TEXT, order_key INTEGER, path TEXT,
  content_hash TEXT,               -- blake3，判断是否需要重新索引
  words_with_punct INTEGER, words_no_punct INTEGER, han_chars INTEGER,
  updated INTEGER
);
CREATE VIRTUAL TABLE chapter_fts USING fts5(
  chapter_id UNINDEXED, title, body, tokenize = 'trigram'  -- trigram 对中文可用，无需分词器
);
CREATE TABLE daily_words (              -- 码字量
  book_id TEXT, day TEXT, delta INTEGER, PRIMARY KEY(book_id, day)
);
CREATE TABLE proof_cache (              -- 校对结果缓存，key 为段落哈希
  para_hash TEXT, backend TEXT, issues_json TEXT, created INTEGER,
  PRIMARY KEY(para_hash, backend)
);
```

FTS5 用 `trigram` 分词器处理中文（`unicode61` 会把整段中文当一个 token）。`[VERIFY]` 实现时确认 rusqlite bundled 版本是否启用 FTS5 与 trigram，未启用则改用 `list_tables` 之外的方案：回退为"遍历 + 内存匹配"，全书 100 万字下仍应 < 300ms。

---

## 6. 模块规格

### 6.1 书架

**行为**
- 启动进入书架页，卡片/列表展示：书名、作者、卷数章数、总字数、最近修改、进度条（若设了目标字数）。
- `[MUST]` 新建书（向导：书名/作者/是否创建"第一卷"）、打开、重命名、置顶、归档、删除（进 `trash/`，二次确认，输入书名确认）。
- `[MUST]` 导入：选择一个 `.txt`，按正则识别章节标题（默认 `^\s*第[一二三四五六七八九十百千零〇\d]+章`，可自定义并预览切分结果）后批量建章。
- `[SHOULD]` 导出：txt（单文件/分卷）、markdown、epub（P2）。
- 扫描策略：启动时只读 `library.toml` + 每本的 `book.toml`，**不读正文**。字数取索引缓存。书架页打开必须 < 100ms（50 本书规模）。

**验收**
- 手动往 `books/` 里丢一个符合布局的目录，重启后书架能识别（自愈扫描）。
- `library.toml` 删掉后能从 `books/` 目录重建。

### 6.2 卷 / 章层级

**行为**
- 左侧目录树：书 → 卷 → 章。卷可折叠。显示每章字数与状态色点。
- `[MUST]` 新建卷/章、重命名、上下移动、跨卷移动章、删除（软删除到 trash）。
- `[MUST]` 移动/重命名只改元数据与文件名，**不触碰正文，不重新生成 ID**。
- `[MUST]` 树上多选（Space 勾选）后批量移动/改状态/统计选中字数。
- `[SHOULD]` 卷/章的简介字段，树上按 `i` 展开速览。

**契约**

```rust
impl Store {
    pub fn create_volume(&mut self, book: BookId, title: &str, after: Option<VolumeId>) -> Result<VolumeId>;
    pub fn create_chapter(&mut self, vol: VolumeId, title: &str, after: Option<ChapterId>) -> Result<ChapterId>;
    /// 跨卷移动。必须是原子的：要么元数据和文件都变了，要么都没变。
    pub fn move_chapter(&mut self, ch: ChapterId, to_vol: VolumeId, after: Option<ChapterId>) -> Result<()>;
    pub fn soft_delete_chapter(&mut self, ch: ChapterId) -> Result<()>;
    pub fn load_body(&self, ch: ChapterId) -> Result<ChapterBody>;
    /// 原子写：tmp -> fsync -> rename -> fsync(dir)
    pub fn save_body(&mut self, body: &ChapterBody) -> Result<()>;
}
```

### 6.3 编辑器内核

**要求**
- `[MUST]` ropey 作为缓冲；插入/删除不得整段重建字符串。
- `[MUST]` 软换行按**显示宽度**折行（CJK=2），折行点优先在中文标点之后 / 空格处；`[MUST]` 禁则处理：行首不得出现 `。，、；：？！）》」』…—` 等，行尾不得出现 `（《「『`（悬挂或提前折行）。
- `[MUST]` 光标按 grapheme 移动；`Home/End` 按显示行而非逻辑段；`Ctrl+←/→` 按词（jieba 分词边界，中文按词跳而非按空格跳，这是与通用编辑器的关键差异）。
- `[MUST]` 视口虚拟化：只渲染可见行，10 万字章节滚动不掉帧。
- `[MUST]` 撤销栈：按操作类型 + 时间间隔（默认 500ms）合并成组；排版/批量替换算**一个**撤销组。栈深默认 500。撤销与"版本历史"是两套机制，互不干扰。
- `[MUST]` 自动保存：默认空闲 3 秒或累计变更 200 字触发，写盘 + 更新缓存字数；崩溃恢复文件 `.<chapter>.swp`，启动时检测到则提示恢复。
- `[MUST]` 中文输入辅助：输入 `「` 自动补 `」`、`（` 补 `）`、`《` 补 `》`、成对引号状态感知（可关）。
- `[SHOULD]` 专注模式：隐藏侧栏，正文居中，栏宽固定（默认 40 全角字），上下留白，可开打字机滚动（光标恒定在屏幕中部）。
- `[SHOULD]` `@` 触发角色名补全（数据源见 6.7）。

**键位方案**：默认 **modeless**（无模式，像普通编辑器）。`keymap = "vim"` 可切到 vim 风格。**不要**默认 vim——目标用户是小说家不是程序员。

### 6.4 字数统计

**口径定义**（必须写进 UI 的说明浮层，避免用户与平台字数对不上时困惑）

```rust
// mj-text/src/count.rs
#[derive(Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WordCount {
    /// 含标点：去除所有换行、制表、行首缩进空白后，按 grapheme cluster 计数。
    /// 半角空格计入（英文语境需要），全角空格 U+3000 不计入（视为缩进符）。
    pub with_punct: usize,
    /// 不含标点：在 with_punct 基础上，再排除 Unicode 类别 P*（标点）、S*（符号）、Z*（分隔符）。
    pub no_punct: usize,
    /// 纯汉字数：CJK 统一汉字及扩展区。
    pub han: usize,
    /// 英文单词数：按 \b[A-Za-z']+\b。
    pub latin_words: usize,
    pub paragraphs: usize,
    pub sentences: usize,   // 以 。！？…… 及其后引号收尾
}

pub fn count(text: &str) -> WordCount;
/// 增量统计：只对受影响的段落重算，与全量结果必须严格一致（属性测试保证）。
pub fn count_incremental(prev: WordCount, old_paras: &[&str], new_paras: &[&str]) -> WordCount;
```

**UI**
- 状态栏常驻：`本章 3,128 / 净 2,904 | 本卷 4.2万 | 全书 21.7万 | 今日 +1,240`。
- `[MUST]` 选中文本时状态栏切为"选中 N 字"。
- `[MUST]` 统计面板：按卷/章列出双口径字数，可导出 CSV。
- `[MUST]` 今日码字量：`daily_words` 表记录每日净增（以每次保存时的 delta 累加，删改为负）。跨零点按本地时区切分，可配置"一天从凌晨 4 点开始"（写作者常见作息）。

**性能**：100 万字全书统计（冷启动，索引失效）< 1s；热路径（编辑时）单次 < 1ms。

### 6.5 一键排版

**核心约束（三条全是验收项）**
1. **幂等**：`format(format(x)) == format(x)`，用 proptest 对随机中文文本验证。
2. **可预览**：执行前弹出 diff 预览面板，显示将改动的位置与条数，可逐条取消。
3. **可撤销**：一次排版 = 一个撤销组；且执行前强制打快照。

**规则集**（每条独立开关，配置于 `[format]`）

| 规则 id | 说明 | 默认 |
|---|---|---|
| `trim_trailing` | 删除行尾空白 | 开 |
| `collapse_blank` | 连续空行压缩为 1 | 开 |
| `paragraph_indent` | 段首缩进：`full_width_two`（两个 U+3000）/ `none` / `keep` | full_width_two |
| `unify_ellipsis` | `...` `。。。` `···` `…` → `……`（成对） | 开 |
| `unify_dash` | `--` `—` `－－` → `——`（成对） | 开 |
| `punct_to_full_width` | 半角标点→全角，**仅当前后至少一侧为 CJK 字符时**（保护 `v2.0`、`a, b`、URL） | 开 |
| `unify_quotes` | 直引号 `"` `'` → 弯引号 `""` `''`（配对状态机，跨段落时按段重置） | 开 |
| `cjk_latin_space` | 中英/中数之间加空格 | **关**（中文网文习惯不加） |
| `full_width_digits` | 全角数字/字母 → 半角 | 开 |
| `strip_inline_space` | 删除 CJK 字符之间的多余空格 | 开 |
| `repeat_punct` | `！！！`→`！` / `。。`→`。` | 关 |
| `line_join` | 合并段内软换行（导入外部文本时常用） | 关 |

**契约**

```rust
// mj-text/src/format.rs
pub struct FormatOptions { /* 上表各字段 */ }

pub struct Edit { pub range: Range<usize>, pub new: String, pub rule: &'static str }

/// 返回编辑列表而非新字符串——这样才能预览、逐条取消、精确映射到光标位置。
pub fn plan(text: &str, opts: &FormatOptions) -> Vec<Edit>;

/// 应用编辑（按 range 倒序应用，避免偏移失效）
pub fn apply(text: &str, edits: &[Edit]) -> String;

#[inline]
pub fn format(text: &str, opts: &FormatOptions) -> String { apply(text, &plan(text, opts)) }
```

**实现要点**
- `plan` 返回的 `Edit` 之间不得区间重叠；重叠时按规则优先级（表中从上到下）裁决，并记 warning 日志。
- 引号配对状态机：遇到孤立引号（段内奇数个）时**不改**，转而产出一条校对 Issue（`punct.unpaired_quote`）。排版规则的原则是：拿不准就不动。
- `[MUST]` 支持范围：当前章 / 当前卷 / 全书。全书排版必须显示进度条且可中断；中断时已完成的章保留（每章独立事务）。

### 6.6 查找替换

**行为**
- 范围：`当前选区 / 当前章 / 当前卷 / 全书 / 整个书架`。
- 模式：`普通 / 全词 / 正则`（regex crate；勾选"扩展语法"时切 fancy-regex 以支持 lookaround）。
- 选项：忽略大小写、**忽略全半角差异**、忽略中文标点差异（如 `,` 与 `，` 视为同一）。
- 结果面板：按 书 → 卷 → 章 分组树，每条显示前后各 15 字上下文并高亮命中；`Enter` 跳转，`Space` 勾选，`r` 替换该条，`A` 替换全部勾选。
- `[MUST]` 全书替换前强制打快照（每章一条），且提供"撤销本次批量替换"（一次性回滚所有受影响章节到快照）。
- `[MUST]` 正则替换支持 `$1` 捕获组引用；非法正则实时提示，不得 panic。

**全半角折叠的实现难点**：不能直接 NFKC 归一化后匹配，因为归一化会改变字节长度，导致命中位置无法映射回原文。做法是自建折叠表（全角↔半角一对一映射，长度不变的字符才纳入），在**逐字符比较层**折叠，位置天然对齐。

```rust
// mj-text/src/search.rs
pub struct Query { pub pattern: String, pub mode: MatchMode, pub flags: MatchFlags }
pub struct Hit { pub chapter: ChapterId, pub range: Range<usize>, pub line: usize, pub context: String }

pub fn search_text(text: &str, q: &Query) -> Result<Vec<Range<usize>>>;
pub fn replace_preview(text: &str, q: &Query, to: &str) -> Result<Vec<Edit>>;
```

**性能**：全书 100 万字普通模式搜索 < 200ms（用 memchr / 索引 FTS 预筛）；正则模式 < 1s。

### 6.7 角色设定

**数据**

```toml
# characters/<id>.toml
id = "cr_3XK9PA2M"
name = "沈砚"
aliases = ["沈公子", "小砚"]        # 用于一致性检查与 @ 补全
role = "主角"                       # 主角/配角/反派/龙套（可自定义）
gender = "男"
age = "二十四"
background = """..."""
personality = """..."""
appearance = """..."""              # 外貌
habits = """..."""                  # 习惯
speech = """口头禅、语言风格..."""
relations = [ { target = "cr_...", label = "师兄" } ]
first_appearance = "ch_7Q2M4KZA"
notes = """..."""
[custom]                            # 用户自定义字段，任意键值
"武器" = "青玉刀"
```

**功能**
- `[MUST]` 角色列表页（按书隔离）：新建/编辑/删除/搜索。编辑用表单式界面，长文本字段进多行编辑框。
- `[MUST]` 编辑器侧栏速查（`Alt+C`）：不离开正文即可翻看角色卡，支持在侧栏内搜索。
- `[MUST]` 角色名与别名**自动注入 jieba 用户词典**（词频给高值），这是校对模块不误报的前提。
- `[MUST]` **专名一致性检查**：扫描正文中与已知角色名编辑距离为 1 且不在词典中的 token，报 `name.suspect`（如「沈研」vs「沈砚」）。这是长篇最实用的检查项之一。
- `[SHOULD]` `@` 触发补全插入角色名。
- `[SHOULD]` 角色出场统计：每个角色在各章的提及次数（复用搜索索引），列表展示；能直观看出谁"消失"了太久。
- `[SHOULD]` 关系以列表呈现即可，不做 ASCII 关系图（终端渲染成本高、收益低）。

### 6.8 错别字 / 病句 / 文风检查

**统一契约**

```rust
// mj-text/src/proof/mod.rs
#[derive(Clone, Serialize, Deserialize)]
pub struct Issue {
    pub range: Range<usize>,       // 相对章节正文的字节区间
    pub severity: Severity,        // Error | Warning | Hint
    pub category: Category,        // Typo | Grammar | Punct | Style | Consistency
    pub rule_id: String,           // "typo.de_di_de" / "style.comma_chain" / "llm.grammar"
    pub message: String,
    pub suggestions: Vec<String>,  // 可一键应用
    pub source: Source,            // Rule | External | Llm —— UI 必须区分展示
    pub confidence: f32,           // 0..1，低于阈值默认折叠
}

pub trait Proofreader: Send {
    fn id(&self) -> &'static str;
    /// 按段落切分后调用，便于缓存与增量。必须可中断（检查 cancel token）。
    fn check(&self, paragraphs: &[&str], ctx: &ProofContext, cancel: &CancelToken) -> Result<Vec<Issue>>;
}
```

**三个后端**

1. `RuleProofreader`（本地，默认开，同步，快）
   - **混淆集**：内置 `confusion.tsv`（`词\t正确词\t触发上下文正则\t说明`），覆盖高频项：的/地/得、在/再、做/作、他/她/它、以/已、既/即、需/须、辨/辩/辫、账/帐、身份/身分、其他/其它、部署/布署、按耐/按捺、如火如荼/如火如茶。用户可在 `dict/confusion.tsv` 追加与覆盖。
   - **分词**：jieba-rs + 用户词典（角色名/专名）。未登录词 + 混淆集 + 上下文正则 三者共同判定。
   - **标点规则**：引号/括号/书名号未配对；句末缺标点；同一段中英标点混用；省略号非偶数个。
   - **文风规则**（category = Style，各自可配阈值，默认开的仅前两条）：
     | rule_id | 检测 | 默认阈值 |
     |---|---|---|
     | `style.comma_chain` | 流水句：连续逗号数超过 N 未见句号 | N=6 |
     | `style.long_sentence` | 单句字数超过 N | N=60 |
     | `style.short_burst` | 连续 N 句均短于 M 字（短句堆砌） | 关（N=5, M=8） |
     | `style.pattern_repeat` | 同一句式在窗口内重复（如「不是……而是……」「与其说……不如说……」），可配句式表 | 关（窗口 800 字，重复≥3） |
     | `style.word_repeat` | 同一实词在窗口内高频重复 | 关（窗口 300 字，≥4 次） |
     | `style.split_clause` | 疑似被拆散的单句（相邻短段落主语相同且无独立完整信息） | 关 |
     - 句式表放在 `dict/patterns.tsv`，用户可增删。默认关的规则要在设置页显式列出，让用户按自己的文风取用。
   - **一致性**：角色名近似（见 6.7）、专名在全书中的用字统计（同一概念两种写法时提示）。

2. `ExternalProofreader`（默认关）
   - 调用用户配置的命令：`command = ["python", "-m", "pycorrector_server"]`，stdin 送 JSON，stdout 读 JSON。
   - 契约（版本化，`"v": 1`）：
     ```json
     // stdin
     {"v":1,"paragraphs":["……","……"],"lang":"zh"}
     // stdout
     {"v":1,"issues":[{"para":0,"start":12,"end":14,"category":"Typo",
                       "message":"…","suggestions":["…"],"confidence":0.8}]}
     ```
   - 超时（默认 30s）、非零退出、非法 JSON → 记日志 + UI 提示，**绝不影响编辑**。

3. `LlmProofreader`（默认关，病句主力）
   - `[MUST]` 用户显式配置 endpoint + api key（key 从环境变量或系统钥匙串读，**不得明文写进 config.toml**）。
   - `[MUST]` 首次开启时弹出说明：正文将被发送到第三方服务。需明确同意。
   - 请求策略：按段落分批（默认 8 段/批，或 2000 字上限），跑在工作线程，串行 + 退避重试；`proof_cache` 按 `blake3(段落 + 后端 + prompt版本)` 缓存，未改动的段落不重复请求。
   - 输出要求模型只返回 JSON（`{"issues":[...]}`），解析失败重试一次后丢弃该批并记日志，不弹错。
   - `[MUST]` 提供"仅检查当前段落 / 当前章"的手动触发入口，**不做全书自动扫描**（成本与延迟不可控）。

**UI**
- 右侧校对面板：按严重度分组，`Enter` 跳转，`a` 应用建议，`i` 忽略本次，`I` 永久忽略（写入 `dict/ignore.json`，key = `blake3(rule_id + 命中文本 + 前后各 10 字)`，这样文本轻微移动不会导致忽略失效）。
- 正文中命中处以下划线/波浪线着色（Error 红、Warning 黄、Hint 暗）。
- `[MUST]` 校对是**手动触发**（`F7` 或 `:proof`）+ 可选的"停止输入 2 秒后跑本地规则"。绝不在打字过程中同步跑，绝不阻塞输入。

### 6.9 版本历史与差分回溯

**模型**

```rust
// mj-core/src/history.rs
pub struct Snapshot {
    pub id: SnapshotId,             // = blake3(content) 前 16 字节
    pub chapter: ChapterId,
    pub created: DateTime<Local>,
    pub trigger: Trigger,           // Manual | Auto | BeforeFormat | BeforeReplace | BeforeImport | BeforeDelete
    pub label: Option<String>,      // 用户命名的里程碑，如 "投稿版"
    pub pinned: bool,               // 钉住则永不淘汰
    pub words: WordCount,
    pub blob: PathBuf,              // history/objects/<xx>/<hash>.zst
    pub parent: Option<SnapshotId>,
}
```

**快照触发**
- 手动：`Ctrl+Shift+S`，可填标签。
- 自动：每 N 分钟（默认 10）**且**自上次快照后净变更 ≥ M 字（默认 300）。两个条件都满足才打，避免刷屏。
- 强制：排版前、批量替换前、导入前、删除章节前（`Trigger::Before*`），无视上述阈值。
- 去重：内容 blake3 与上一条相同则不新建，只更新上一条的时间戳。

**保留策略（上限 40 / 每章）**

不要用纯 FIFO。纯 FIFO 下，一个下午的密集自动快照会把上个月的手稿全部挤掉——而用户想回退的恰恰是上个月那版。默认 `retention = "thinned"`：

```
保留优先级（从高到低）：
1. pinned / 有 label 的      —— 永不淘汰，不占 40 的额度（另设上限 20，满则提示用户手动清理）
2. 最近 10 条                —— 全保留
3. 最近 24 小时内            —— 每小时保留最新 1 条
4. 最近 30 天内              —— 每天保留最新 1 条
5. 更早                      —— 每周保留最新 1 条
超出 40 时，从优先级最低的桶里淘汰最旧的
```
配置 `retention = "fifo"` 可退回简单 FIFO。淘汰快照时，若其 blob 不再被任何快照引用，才删除 blob（内容寻址天然去重：反复保存相同内容不额外占空间）。

**差分预览（用户明确要求的核心项）**

```rust
pub enum DiffGranularity { Paragraph, Char }

pub struct DiffHunk {
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
    pub ops: Vec<DiffOp>,           // Equal(&str) | Insert(&str) | Delete(&str)
}

/// 两级 diff：先按段落（行）做 Myers 找出变动块，再对变动块内部做字符级 diff。
/// 直接对十万字做字符级 diff 会慢且噪声大。
pub fn diff(old: &str, new: &str) -> Vec<DiffHunk>;
```

**Diff 界面** `[MUST]`
- 入口：历史面板选中任一快照 → `Enter` 打开 diff，默认对比**该快照 vs 当前版本**（用户原话："与现版本相比哪里做了改动"）；也支持选中两条快照互比（`Space` 选第二条）。
- 布局：宽度 ≥ 100 列时左右分栏，否则统一为 inline 视图（增行绿底、删行红底、行内字符级高亮用反色）。
- 顶部摘要：`+312 字 / -87 字 / 3 处改动`，`n`/`p` 在改动块间跳转。
- `[MUST]` 三种恢复粒度：
  1. **整章恢复**（恢复前自动给当前版本打一次快照 —— 回退本身也可回退）
  2. **单块恢复**（把某个 hunk 的旧内容应用回当前版本）
  3. **复制旧内容**到剪贴板，不改当前版本
- 中文 diff `[MUST]` 按 grapheme 而非 byte 切分，否则会切出乱码。

**验收**
- 连续保存同样内容 100 次，`history/objects` 下只有 1 个 blob。
- 打满 40 条后继续保存，pinned 的快照仍在，且时间跨度覆盖仍在（thinned 策略生效）。
- 对 5 万字章节做整章 diff，渲染 < 500ms。

### 6.10 外观与字体

```rust
// mj-tui/src/font.rs
bitflags! {
    pub struct FontCap: u8 {
        const SET_FAMILY = 0b001;
        const SET_SIZE   = 0b010;
        const RESET      = 0b100;
    }
}

pub trait FontController: Send {
    fn id(&self) -> &'static str;
    fn caps(&self) -> FontCap;
    fn set_family(&mut self, family: &str) -> Result<()>;
    fn set_size(&mut self, pt: f32) -> Result<()>;
    fn reset(&mut self) -> Result<()>;
}

/// 依次探测，返回第一个可用后端；全部不可用返回 NoopFont。
/// 探测依据：TERM_PROGRAM / TERM / KITTY_WINDOW_ID / WEZTERM_PANE 等环境变量。[VERIFY]
pub fn detect() -> Box<dyn FontController>;
```

后端：`Osc50Font`（xterm/urxvt 系）、`KittyFont`（仅 SET_SIZE，走远程控制）、`WezTermFont` `[SHOULD]`、`NoopFont`。

`[MUST]` 退出时恢复原字体，并挂 panic hook：panic 时先恢复终端（离开 alternate screen、关 raw mode、重置字体）再打印 backtrace。否则崩溃后用户终端字体永久变形。

**外观预设**（不支持改字体时的主路径，也是独立功能）
```toml
[appearance]
theme = "sepia"              # dark | light | sepia | high_contrast | 自定义
column_width = 40            # 正文栏宽，单位「全角字」；0 = 撑满
paragraph_spacing = 0        # 段间额外空行
margin = 4                   # 左右留白列数
line_number = false
font_family = "Source Han Serif"   # 仅支持的终端生效
font_size = 14
```
`[MUST]` 设置页在字体不可用时显示灰态 + 一句话原因（"当前终端（Alacritty）不支持运行时更改字体"）+ 按钮「生成配置片段」，输出可粘贴的 `alacritty.toml` / `kitty.conf` / `wezterm.lua` 字体段。这是把"做不到"变成"帮你做到"的关键，不要省。

主题定义为 TOML，放 `themes/*.toml`，用户可自建。所有颜色走 ratatui `Style`，`[MUST]` 探测 truecolor（`COLORTERM`）并在仅 256 色时自动降级取近似色。

---

## 7. TUI 设计

### 7.1 屏幕状态机

```
Shelf(书架) ──open──> Workspace(工作区) ──Esc──> Shelf
Workspace = Tree | Editor 双焦点 + 可选右侧面板 + 底部状态栏
浮层（Modal，压栈，Esc 逐层弹出）：
  CommandPalette / Find / History / Diff / CharacterCard / Settings
  / Confirm / Input / Toast
```

`[MUST]` 用显式的 `Vec<Modal>` 栈管理浮层，不要用一堆 bool 标志位——后者在第三个浮层出现时必然失控。

### 7.2 布局

```
┌ 墨简 · 《雪夜行》 ────────────────────────── 沈砚 · 21.7万字 ┐
│┌─目录─────┐┌─正文──────────────────────┐┌─校对/角色─┐│
││ ▾ 第一卷 ││                                    ││          ││
││   ● 第一章││    　　雪落了一夜。                ││          ││
││   ○ 第二章││                                    ││          ││
││ ▸ 第二卷  ││                                    ││          ││
│└──────────┘└───────────────────────────┘└──────────┘│
│ 插入 │ 本章 3,128/净 2,904 │ 今日 +1,240 │ ●未保存 │ F1 帮助 │
└──────────────────────────────────────────────────────────┘
```
- 目录树宽度默认 24 列，可拖/可配置，`Ctrl+B` 折叠。
- 右侧面板默认隐藏，宽度 32 列。
- 终端宽度 < 80 列时：自动隐藏侧栏，只留正文（并在状态栏提示）。`[MUST]` 支持窄至 60 列不崩。

### 7.3 键位表（modeless 默认）

| 键 | 动作 |
|---|---|
| `Ctrl+P` | 命令面板（所有功能都必须能从这里触达 —— 这是最重要的一条） |
| `Ctrl+S` | 保存 |
| `Ctrl+Shift+S` | 打快照（可加标签） |
| `Ctrl+B` | 切换目录树 |
| `Ctrl+F` / `Ctrl+H` | 查找 / 查找替换 |
| `Ctrl+Shift+F` | 全书查找 |
| `F5` | 一键排版（当前章，弹预览） |
| `F7` | 校对当前章 |
| `F8` | 历史面板 |
| `Alt+C` | 角色速查侧栏 |
| `Ctrl+Z` / `Ctrl+Y` | 撤销 / 重做 |
| `Ctrl+N` | 新建章（当前卷末） |
| `Ctrl+Tab` | 上/下一章 |
| `F11` | 专注模式 |
| `F1` | 帮助（键位总表） |
| `Esc` | 弹出当前浮层 / 回书架 |
| 树内：`Space` 勾选，`J/K` 移动条目，`m` 移动到…，`r` 重命名，`d` 删除 | |
| Diff 内：`n/p` 跳改动，`u` 恢复此块，`U` 恢复整章，`y` 复制旧内容 | |

`[MUST]` 键位全部可在 `[keymap]` 里重绑定；`[MUST]` 冲突检测（启动时校验，冲突则报警并用默认值）。

### 7.4 事件循环

```rust
enum AppEvent {
    Term(crossterm::event::Event),
    Tick,                          // 100ms，驱动自动保存计时/动画
    Proof(ProofResult),            // 工作线程回传
    Index(IndexProgress),
    Font(FontResult),
}
```
- 主线程：`recv_timeout(16ms)` → 处理事件 → 若 dirty 则渲染。**不要固定 60fps 空转重绘**，只在状态变化时绘制（省电，且 SSH 下体感更好）。
- 长任务（全书排版/索引/LLM）一律进工作线程 + `CancelToken` + 进度条 + `Esc` 取消。
- `[MUST]` 输入延迟 p99 < 16ms（10 万字章节内）。

---

## 8. 配置

`config.toml`，缺失字段用默认值，**多余字段保留不报错**（前向兼容）。`[MUST]` 提供 `mj config check` 校验并打印生效值。

```toml
[general]
workspace = "~/.local/share/mojian"
day_starts_at = 4              # 码字量按凌晨4点切日
keymap = "modeless"            # modeless | vim

[editor]
autosave_idle_ms = 3000
autosave_words = 200
undo_depth = 500
auto_pair = true
word_nav = "jieba"             # jieba | space
focus_column_width = 40

[count]
count_halfwidth_space = true

[format]                       # 见 6.5 规则表
paragraph_indent = "full_width_two"
cjk_latin_space = false

[proofread]
on_idle_local = true
idle_ms = 2000
backends = ["rule"]            # rule | external | llm
[proofread.style]
comma_chain = 6
long_sentence = 60
pattern_repeat = false

[proofread.llm]
endpoint = "https://api.anthropic.com/v1/messages"
model = "claude-sonnet-4-6"
api_key_env = "ANTHROPIC_API_KEY"   # 只存环境变量名，不存 key
batch_paragraphs = 8

[history]
max_per_chapter = 40
retention = "thinned"          # thinned | fifo
auto_interval_min = 10
auto_min_words = 300

[appearance]
theme = "sepia"
column_width = 40
font_family = "Source Han Serif"
font_size = 14
```

---

## 9. 非功能需求

**性能预算**（在 2020 年代中端笔记本、100 万字全书、10 万字单章下）

| 场景 | 预算 |
|---|---|
| 冷启动到书架可交互 | < 150ms |
| 打开 10 万字章节 | < 200ms |
| 按键到屏幕更新 p99 | < 16ms |
| 全书普通搜索 | < 200ms |
| 全书字数重算（索引失效） | < 1s |
| 5 万字章节 diff 渲染 | < 500ms |
| 常驻内存（单书打开） | < 150MB |

**数据安全**
- `[MUST]` 原子写：`write(tmp)` → `fsync(tmp)` → `rename(tmp, target)` → `fsync(dir)`。
- `[MUST]` 单实例锁：workspace 下 `.lock`（含 pid），已被占用时提示而非覆盖。陈旧锁（进程不存在）可自动清理。
- `[MUST]` 删除一律软删除到 `trash/`，保留原路径信息，可还原。
- `[MUST]` panic hook 恢复终端 + 把当前未保存缓冲 dump 到 `crash/<ts>-<chapter>.txt`。

**日志**：`tracing` → `logs/mj.log`（按天轮转，保留 7 天）。`RUST_LOG` 可调级别。TUI 期间零 stdout 输出。

**跨平台**：Linux / macOS 一等支持；Windows（Windows Terminal）尽力支持，路径与 CRLF 需处理（`[MUST]` 读入时统一 LF，写出按配置 `line_ending = "lf" | "native"`）。

**可访问性**：`[SHOULD]` 高对比主题；`[MUST]` 所有功能不依赖鼠标；`[SHOULD]` 鼠标可选支持（点击树、拖分隔条、滚轮）。

---

## 10. 测试策略

| 层 | 手段 | 关键用例 |
|---|---|---|
| `mj-text` 单元 | 常规 + `insta` 快照 | 每条排版规则的正反例；标点边界（`v2.0`、`a, b`、URL 不被全角化）；字数各口径 |
| `mj-text` 属性 | `proptest`（生成含中英标点/emoji/组合字符的随机文本） | **排版幂等**：`format(format(x)) == format(x)`；**统计一致**：`count_incremental` == `count` 全量；**搜索位置**：所有 Hit range 落在 char boundary 上 |
| `mj-core` 集成 | `tempfile` 建临时 workspace | 移动章跨卷后重启一致；索引删除后重建等价；40 条上限 + pinned 保留；断电模拟（写到一半 kill）后无损坏 |
| `mj-tui` | ratatui `TestBackend` + `insta` | 各屏幕在 60/80/120/200 列宽下的渲染快照；浮层栈进出；键位冲突检测 |
| 基准 | `criterion` | 打开 10 万字章、全书搜索、5 万字 diff、增量统计 |

`[MUST]` CI 跑 `cargo clippy -- -D warnings` 与 `cargo test --all`。`[MUST]` 排版与统计的属性测试是**发布门禁**，不允许标 ignore。

---

## 11. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **M0 骨架** | workspace 建好；ratatui 起窗；panic hook 恢复终端；日志；config 加载 | 能开能关，崩溃不留残터端 |
| **M1 能写字** | Store + 原子写 + 章节格式；ropey 编辑器（grapheme 光标、宽度折行、禁则、undo、自动保存、swp 恢复）；书架 + 卷/章树 | 能新建书→建卷→建章→写 3000 字→重启内容完好；拔电测试不损坏 |
| **M2 数得清** | 字数四口径 + 增量 + 状态栏 + 统计面板 + 今日码字；SQLite 索引 | 与人工计数逐字符一致；性能达标 |
| **M3 能改** | 一键排版（plan/apply/预览/撤销/幂等）+ 查找替换（含全书、正则、全半角折叠、批量撤销） | 幂等属性测试通过；全书替换可一键回滚 |
| **M4 回得去** | 快照 + 内容寻址去重 + thinned 保留 + 两级 diff + diff 界面 + 三种恢复粒度 | 6.9 全部验收项通过 |
| **M5 帮着看** | 角色设定（含词典注入、一致性检查、侧栏、@补全）；RuleProofreader（错别字/标点/文风）+ 校对面板 + 忽略机制 | 在一部真实 10 万字稿上跑，误报率可接受（人工抽检 100 条 ≥ 70% 有效） |
| **M6 好看好用** | 外观预设 + 主题 + FontController 多后端 + 配置片段生成；专注模式；命令面板；帮助页；导入导出 | 在 kitty/alacritty/WT 三种终端下行为正确且降级合理 |
| **M7 可选** | ExternalProofreader、LlmProofreader、epub 导出、角色出场统计、鼠标支持 | —— |

**实现顺序建议**：严格按 M0→M6。特别是**不要**先做校对或字体——那是最容易做出 demo、也最容易做废的两块；先把"写-存-数-改-回退"这条主链打通，工具就已经能用了。

---

## 12. 附录

### 12.1 关键决策速查

| 决策 | 选择 | 理由 |
|---|---|---|
| 正文存储 | 纯文本文件 | 可救、可 git、可外部编辑 |
| 数据库 | SQLite，仅索引 | 可重建，坏了删掉即可 |
| 缓冲 | ropey | 大章节编辑 O(log n) |
| 排序 | 稀疏 order（步长 10） | 移动一章不重写全卷 |
| 快照 | 内容寻址 + zstd + thinned 保留 | 去重 + 时间跨度不塌缩 |
| diff | 段落级 Myers → 变动块内字符级 | 十万字直接字符 diff 太慢且噪声大 |
| 异步 | std thread + mpsc | 不引入 tokio 复杂度 |
| 默认键位 | modeless | 目标用户是作者不是程序员 |
| 字体 | 能力探测 + 三级降级 | 终端多数不支持运行时改字体 |
| 病句 | 可插拔后端，本地不硬做 | 规则引擎在语义任务上误报率不可接受 |

### 12.2 无头子命令（便于脚本与测试）

```
mj                          # 启动 TUI
mj count [--book <id>] [--json]
mj format <path> [--check]  # --check 只报告不改，退出码非零表示需要排版
mj export <book> --format txt|md|epub -o <out>
mj history list <chapter>
mj config check
mj doctor                   # 探测终端能力（truecolor/字体/键盘协议/剪贴板）并打印报告
```

`mj doctor` 是排查用户环境问题的第一手段，别省。

### 12.3 内置混淆集起始条目（`dict/confusion.tsv`）

格式：`错误形\t建议\t上下文正则(可空)\t说明`

```
的确良	的确良		（示例：白名单条目，防止「的/地/得」规则误报固定词）
按耐	按捺		「按捺不住」
如火如茶	如火如荼
迫不急待	迫不及待
不径而走	不胫而走
一如继往	一如既往
张慌	张皇
心急如粪	心急如焚
渡过难关	度过难关	难关	「度过」用于时间/难关，「渡过」用于水面
即使	既使		常见误写
其它	其他	(?<![人事物])	非严格，默认 Hint 级
```
`[MUST]` 「的/地/得」不做无条件替换，只在高置信上下文（如「地」+ 名词、「的」+ 动词补语结构）给 Hint 级建议，且默认 confidence < 0.6 折叠。这条规则最容易惹恼用户，宁保守勿激进。

### 12.4 Diff 界面线框

```
┌ 与「2026-07-14 22:10 · 投稿版」比较 ─── +312 / -87 / 3 处改动 ──┐
│ 12 │   　　雪落了一夜。                                          │
│ 13 │ - 　　他推开门，风灌进来，很冷。                            │
│ 13 │ + 　　他推开门，风裹着雪灌进来，冷得刺骨。                  │
│    │                       ^^^^        ^^^^^^                    │
│ 14 │   　　院里那株梅树…                                        │
├──────────────────────────────────────────────────────────────┤
│ n/p 跳转改动 │ u 恢复此块 │ U 恢复整章 │ y 复制旧内容 │ Esc 关闭│
└──────────────────────────────────────────────────────────────┘
```

### 12.5 待你确认的开放问题

1. 是否需要**大纲/伏笔追踪**（章节级的伏笔埋设与回收标记 + 未回收提醒）？这是长篇最常见的缺失项，但会引入新的数据结构与视图。
2. 目标平台是否包含 Windows？包含的话 M0 就要处理 CRLF 与路径，不要留到最后。
3. 云同步的期待是什么？当前设计对 git 友好但不内置 git；若需要开箱即用的同步，需要在 M6 后追加一个 `mj sync` 模块。
4. 导出格式的优先级：投稿 txt / 发布平台粘贴 / epub 自阅，哪个先做？
