//! 应用状态机与运行入口。见 doc.md §7。
//!
//! 屏幕状态机（§7.1）：
//! ```text
//! Shelf(书架) ──open──> Workspace(工作区) ──Esc──> Shelf
//! Workspace = Tree | Editor 双焦点 + 底部状态栏
//! ```

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId, VolumeId};
use mj_core::model::{Book, ChapterBody};
use mj_core::store::Store;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::batch::{BatchJob, BatchKind, BatchUndo, Scope};
use crate::commands::Command;
use crate::editor::{Action, AutoSave, Buffer, Viewport};
use crate::event::{AppEvent, EventLoop, LlmProofDone};
use crate::screens::character_form::CharacterForm;
use crate::screens::character_panel::CharacterPanel;
use crate::screens::command_palette::CommandPalette;
use crate::screens::completion::Completion;
use crate::screens::confirm::Confirm;
use crate::screens::consent::Consent;
use crate::screens::format_preview::{self, FormatPreview};
use crate::screens::help::{self, Help};
use crate::screens::history_panel::{DiffView, HistoryPanel, LineKind};
use crate::screens::input::{Input, InputIntent};
use crate::screens::modal::{Modal, ModalKind, ModalStack};
use crate::screens::proof_panel::{self, ProofPanel};
use crate::screens::search_panel::{self, SearchPanel};
use crate::screens::settings::{self, Settings};
use crate::screens::shelf::{Shelf, format_words};
use crate::screens::stats::{self, Stats};
use crate::screens::tree::{Row, Tree};
use crate::theme::{ColorDepth, Theme};

/// §7.2：目录树宽度默认 24 列。
const TREE_WIDTH: u16 = 24;
/// 侧栏能被拖到的最窄宽度。再窄连「第一章」加边框都放不下，
/// 拖成一条缝之后用户只能靠 Ctrl+B 收起再展开才找得回来。
const TREE_MIN_WIDTH: u16 = 12;

/// 侧栏能被拖到的最宽宽度：不许越过一半，正文才是主角。
fn tree_max_width(total: u16) -> u16 {
    (total / 2).max(TREE_MIN_WIDTH)
}
/// §7.2：终端宽度 < 80 列时自动隐藏侧栏。
const NARROW_THRESHOLD: u16 = 80;

/// 当前屏幕（§7.1）。
///
/// `Workspace` 装着整本书的元数据与打开的缓冲，比 `Shelf` 大得多。
/// 装箱让 `Screen` 本身保持小尺寸——它在事件循环里被反复 match，
/// 且同一时刻只存在一个，堆分配的代价可忽略。
enum Screen {
    Shelf(Shelf),
    Workspace(Box<Workspace>),
}

/// 工作区：树 + 编辑器双焦点。
struct Workspace {
    book: Book,
    tree: Tree,
    editor: Option<OpenChapter>,
    focus: Focus,
    /// 侧栏是否显示（Ctrl+B 切换，§7.3）。
    show_tree: bool,
    /// 浮层栈（§7.1 [MUST]）：统计/排版预览/查找/历史/diff/确认/校对/角色/角色表单。
    /// 谁在栈顶谁吃键，Esc 逐层弹出。
    modals: ModalStack,
    /// 正在跑的批量作业（全卷/全书排版或替换，§6.5/§6.6）。
    ///
    /// **不进浮层栈**：它不是用户可关的浮层，Esc 的语义是「中断作业」而非
    /// 「关闭窗口」；§7.1 列的浮层里也没有它。
    batch: Option<BatchJob>,
    /// 最近一次校对的问题（整章坐标），供正文下划线着色（§6.8 [MUST]）。
    /// 关面板后仍留着，好让 Enter 跳过去时看得见；正文一改就清空——坐标失效了。
    proof_issues: Vec<mj_text::proof::Issue>,
    /// 正文里 `@` 触发的角色名补全（§6.7 [SHOULD]）。
    ///
    /// **不进浮层栈**：键仍然打进缓冲，它不夺焦点，只是跟随光标的候选框。
    completion: Option<Completion>,
    /// 上一帧画出来的命中区域（§13 鼠标支持）。
    hit: Hit,
    /// 侧栏宽度（§13：可拖分隔条）。只活在本次会话——每拖一格就写一次
    /// config.toml 太吵，而这本就是随手调的东西，不值当为它反复动用户的配置文件。
    tree_width: u16,
}

/// 鼠标要落到哪块区域，只能照**上一帧**的版面判——事件到达时这一帧早画完了。
/// 这不是将就：用户看着那一帧点的，就该按那一帧算。
#[derive(Default, Clone, Copy)]
struct Hit {
    /// 目录树的外框，以及它上一帧滚到了第几行（树是虚拟化的，不记就换算不回行号）。
    tree: Option<(Rect, usize)>,
    /// 正文那一侧的整块（含边框留白），用来判「滚轮是不是在正文上」。
    editor: Rect,
    /// 正文**真正**落笔的那块。定位光标必须用它——用外框会差出边框和留白。
    body: Rect,
}

impl Workspace {
    /// 当前打开的章所属卷的字数（§6.4 状态栏「本卷 4.2万」）。
    fn current_volume_words(&self) -> Option<u64> {
        let ch = self.editor.as_ref()?.id;
        let (vol, _) = self.book.find_chapter(ch)?;
        Some(vol.chapters.iter().filter_map(|c| c.word_count).sum())
    }
}

struct OpenChapter {
    id: ChapterId,
    buffer: Buffer,
    viewport: Viewport,
    autosave: AutoSave,
    /// 本章字数缓存。
    ///
    /// 必须缓存而非每帧重算：§6.4 要求编辑时单次统计 < 1ms，
    /// 但状态栏每帧都要显示——10 万字章节下每帧全量统计会直接吃掉
    /// §9 的 16ms 按键预算。故只在编辑后更新。
    word_count: mj_text::count::WordCount,
    /// 上次落盘时的字数，用于算今日码字量的增量（§6.4）。
    saved_words: usize,
    /// 章节文件的绝对路径。缓存它是因为 swp 每 500ms 就要写一次，
    /// 而由 id 反查路径要扫整本书的目录——那太贵了。
    /// 代价：用户在程序运行期间从外部移动了文件，swp 会写到旧位置。
    /// 可接受：正文的保存路径仍走 Store 的实时反查，swp 只是保险丝。
    path: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Editor,
}

pub struct App {
    store: Store,
    config: Config,
    index: Option<mj_core::index::Index>,
    screen: Screen,
    should_quit: bool,
    /// 仅在状态变化时重绘（§7.4：不要固定 60fps 空转）。
    dirty: bool,
    /// 底部提示语，操作后给一句反馈。
    toast: Option<String>,
    /// 今日净增字数（§6.4）。
    today_words: i64,
    /// 上次快照的时刻与当时的字数。自动快照要「每 N 分钟**且**净变更 ≥ M 字」，
    /// 两个条件都得记着。切章时重置——那是另一条历史线。
    last_snapshot: Option<(std::time::Instant, usize)>,
    /// 上一次批量操作的回滚记录（§6.6 [MUST]「撤销本次批量替换」）。
    batch_undo: Option<BatchUndo>,
    /// 已忽略的校对问题（§6.8）。懒加载：首次 F7 时从 dict/ignore.json 读。
    ignore: Option<mj_core::proofing::IgnoreSet>,
    /// 当前配色（§6.10）。启动时按 config.appearance.theme + 终端色深解析。
    theme: Theme,
    /// 专注模式（F11，§7.3）：收起目录树、按 focus_column_width 收窄正文。
    focus_mode: bool,
    /// 键位表（§7.3 [MUST] 可重绑定）。启动时由 config 的 [keymap] 构建。
    keymap: crate::keymap::Keymap,
    /// 正在跑的模型校对（§6.8）。同时只允许一趟——那是要花钱的请求，
    /// 连按两下命令不该变成两趟。
    llm_job: Option<LlmJob>,
    /// 工作线程的回传端。`App::new` 时还没有事件循环，故由 `run_loop` 装上。
    events_tx: Option<std::sync::mpsc::Sender<AppEvent>>,
    /// 正按着分隔条拖（§13）。
    dragging_divider: bool,
    /// 活动中的单行输入框（改名，§6.1/§6.2）。
    ///
    /// 放 App 顶层而非浮层栈：改书名是在**书架**上做的，而书架没有浮层栈；
    /// 一个顶层字段能同时盖住书架与工作区，比给两处各接一套输入省事得多。
    /// 它一旦是 Some，就抢在所有屏幕之前吃键（见 `on_key`）。
    input: Option<Input>,
}

/// 在跑的那趟模型校对。
struct LlmJob {
    /// Esc 用它掐掉（§7：长任务要能取消）。
    cancel: mj_text::proof::CancelToken,
    chapter: ChapterId,
}

impl App {
    pub fn new(store: Store, config: Config) -> anyhow::Result<Self> {
        let books = store.list_books()?;
        let theme = Self::resolve_theme(&store, &config);

        // 键位表：冲突要报警（§7.3 [MUST]）。这里只记日志——起窗之后不能往
        // stdout 打字（§0 禁令 2），提示改由状态栏 toast 给。
        let (keymap, problems) = crate::keymap::Keymap::from_config(&config.keymap);
        for p in &problems {
            tracing::warn!("{}", p.message());
        }
        // 首屏给一条提示：光写日志等于没报警——用户不会去翻日志，
        // 只会觉得「我配的键位怎么不管用」。
        let toast = problems.first().map(|p| {
            if problems.len() > 1 {
                format!(
                    "{}（另有 {} 处键位问题，详见日志）",
                    p.message(),
                    problems.len() - 1
                )
            } else {
                p.message()
            }
        });

        Ok(Self {
            store,
            config,
            // 索引按书打开（§5.1：books/<id>/.index.sqlite），故此处为空。
            index: None,
            screen: Screen::Shelf(Shelf::new(books)),
            should_quit: false,
            dirty: true,
            toast,
            today_words: 0,
            last_snapshot: None,
            batch_undo: None,
            ignore: None,
            theme,
            focus_mode: false,
            keymap,
            llm_job: None,
            events_tx: None,
            dragging_divider: false,
            input: None,
        })
    }

    /// 解析当前配色（§6.10）。
    ///
    /// 用户 `themes/<name>.toml` 优先于同名内置主题——这样用户能覆盖内置的
    /// sepia 而不必另起名字。色深按终端探测，仅 256 色时主题自动降级取近似。
    fn resolve_theme(store: &Store, config: &Config) -> Theme {
        let depth = ColorDepth::detect();
        let name = &config.appearance.theme;
        match store.workspace().read_theme(name) {
            Some(text) => Theme::from_toml(&text, depth, "sepia"),
            None => Theme::load_builtin(name, depth),
        }
    }

    /// 当前配色，供渲染。
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// 告知已开启 kitty 键盘协议——首屏提示一句，好让用户知道那两个键现在能用了。
    #[doc(hidden)]
    pub fn note_keyboard_protocol(&mut self) {
        if self.toast.is_none() {
            self.toast =
                Some("已开启 kitty 键盘协议：Ctrl+Shift+S 打快照、Ctrl+Tab 换章可用".into());
        }
    }

    /// 打开某书的索引。
    ///
    /// 索引是缓存不是真相（§0 禁令 3）：连重建都失败（磁盘满/只读）时
    /// 降级为 None 继续跑——字数改从元数据现算，搜索慢一点，
    /// 但绝不能因为一个缓存打不开就让人写不了字。
    fn open_index(&mut self, book: BookId) {
        let path = self.store.workspace().book_index_file(book);
        self.index = match mj_core::index::Index::open(&path) {
            Ok(i) => Some(i),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "索引不可用，降级运行");
                None
            }
        };
        self.reindex_book(book);
        self.refresh_today_words(book);
    }

    /// 补齐索引：正文变了（或索引刚重建）的章重新统计。
    ///
    /// 靠 `content_hash` 判断（§5.4）：哈希没变就跳过，故这不是每次开书都
    /// 全量重算——只有第一次、或用户在外部改过文件时才读正文。
    /// 这也是「索引删掉能自动重建」的实现（§6.1 验收：手动丢进 books/ 的书能识别）。
    fn reindex_book(&mut self, book: BookId) {
        let Some(idx) = &self.index else { return };
        let Ok(b) = self.store.load_book(book) else {
            return;
        };

        let mut changed = 0usize;
        for v in &b.volumes {
            for c in &v.chapters {
                // 受损章不读（ADR 0004）。
                if c.damaged.is_some() {
                    continue;
                }
                let Ok(body) = self.store.load_body(book, c.id) else {
                    continue;
                };
                let text = body.text.to_string();
                let hash = mj_core::index::content_hash(&text);

                // 哈希一致 → 索引是新的，跳过。
                if idx.chapter_hash(c.id).ok().flatten().as_deref() == Some(hash.as_str()) {
                    continue;
                }

                let wc = mj_text::count::count(&text);
                let entry = mj_core::index::ChapterEntry {
                    chapter: c.id,
                    book,
                    volume: v.id.to_string(),
                    title: c.title.clone(),
                    order: c.order,
                    path: c.path.clone(),
                    content_hash: hash,
                    words_with_punct: wc.with_punct as u64,
                    words_no_punct: wc.no_punct as u64,
                    han_chars: wc.han as u64,
                    updated: chrono::Local::now().timestamp(),
                };
                if let Err(e) = idx.upsert_chapter(&entry) {
                    tracing::warn!(error = %e, "索引写入失败");
                }
                changed += 1;
            }
        }
        if changed > 0 {
            tracing::info!(chapters = changed, "已补齐索引");
        }
    }

    /// 从索引读今日码字量。
    fn refresh_today_words(&mut self, book: BookId) {
        let day =
            mj_core::index::writing_day(chrono::Local::now(), self.config.general.day_starts_at);
        self.today_words = self
            .index
            .as_ref()
            .and_then(|i| i.daily_delta(book, &day).ok())
            .unwrap_or(0);
    }

    /// 主循环。
    pub fn run_loop(
        &mut self,
        term: &mut DefaultTerminal,
        events: &EventLoop,
    ) -> anyhow::Result<()> {
        // 工作线程要有路把结果送回来（§7）。
        self.events_tx = Some(events.sender());
        while !self.should_quit {
            if self.dirty {
                term.draw(|f| self.render(f))?;
                self.dirty = false;
            }

            // 批量作业进行中：不能停在 recv 上干等，否则进度条不动、Esc 按不了。
            // 干一小块活、瞄一眼按键，如此往复（§6.5 [MUST] 可中断）。
            if matches!(&self.screen, Screen::Workspace(ws) if ws.batch.is_some()) {
                if let Some(AppEvent::Term(Event::Key(k))) = events.try_next()
                    && k.kind == KeyEventKind::Press
                    && matches!(k.code, KeyCode::Esc)
                    && let Screen::Workspace(ws) = &mut self.screen
                    && let Some(job) = &mut ws.batch
                {
                    job.cancel();
                }
                self.step_batch()?;
                continue;
            }

            match events.next()? {
                AppEvent::Term(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    self.on_key(k.code, k.modifiers)?;
                    self.dirty = true;
                }
                AppEvent::Term(Event::Resize(_, _)) => self.dirty = true,
                // 鼠标只在配置开了时才被捕获，走到这里就说明用户要它（§13）。
                AppEvent::Term(Event::Mouse(m)) => {
                    self.toast = None;
                    self.on_mouse(m)?;
                    self.dirty = true;
                }
                AppEvent::Term(_) => {}
                // 自动保存的心跳（§7.4：Tick 驱动自动保存计时）。
                AppEvent::Tick => self.on_tick()?,
                AppEvent::LlmProof(done) => {
                    self.on_llm_proof_done(*done);
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        self.toast = None;

        // Ctrl+C 任何时候都退出。
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Ok(());
        }

        // 输入框活着时，它抢在所有屏幕之前吃键——正在改名，`d`/`r`/`j` 都该是
        // 往名字里打的字，而不是触发别的动作。
        if self.input.is_some() {
            return self.input_handles_key(code).map(|_| ());
        }

        // 模型校对在飞的时候，Esc 就是「掐掉它」——弹出的提示是这么许诺的（§7）。
        // 掐完 Esc 恢复原义（关浮层 / 取消选区 / 回书架）。
        if code == KeyCode::Esc
            && let Some(job) = self.llm_job.take()
        {
            job.cancel.cancel();
            self.toast = Some("已取消模型校对".into());
            return Ok(());
        }

        match &mut self.screen {
            Screen::Shelf(_) => self.on_key_shelf(code, mods),
            Screen::Workspace(_) => self.on_key_workspace(code, mods),
        }
    }

    // ---- 书架 ----

    fn on_key_shelf(&mut self, code: KeyCode, _mods: KeyModifiers) -> anyhow::Result<()> {
        let Screen::Shelf(shelf) = &mut self.screen else {
            return Ok(());
        };

        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => shelf.move_down(),
            KeyCode::Up | KeyCode::Char('k') => shelf.move_up(),
            KeyCode::Enter => {
                if let Some(id) = shelf.selected_id() {
                    self.open_book(id)?;
                }
            }
            KeyCode::Char('n') => self.new_book()?,
            // `r`：给选中的书改名（§6.1 [MUST]）。
            KeyCode::Char('r') => {
                if let Some(book) = shelf.selected() {
                    let (title, id) = (book.title.clone(), book.id);
                    self.start_rename(title, InputIntent::RenameBook(id));
                }
            }
            // `d`：删选中的书。§6.1 [MUST]：必须输入书名确认——一本书是几十万字，
            // 敲个 y 太轻。
            KeyCode::Char('d') => {
                if let Some(book) = shelf.selected() {
                    let (title, id) = (book.title.clone(), book.id);
                    self.start_delete_confirm(
                        format!("删除《{title}》？请输入完整书名以确认"),
                        InputIntent::DeleteBook(id),
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 新建书。M1 用固定标题——建书向导（§6.1）需要输入浮层，属 M6 的命令面板范畴。
    fn new_book(&mut self) -> anyhow::Result<()> {
        let book = self.store.create_book("新书", "佚名")?;
        // 建一卷一章，否则新书打开是空的，用户无处下笔。
        let vol = self.store.create_volume(book.id, "第一卷", None)?;
        self.store.create_chapter(book.id, vol, "第一章", None)?;

        let books = self.store.list_books()?;
        if let Screen::Shelf(shelf) = &mut self.screen {
            shelf.reload(books, Some(book.id));
        }
        self.toast = Some("已新建《新书》".into());
        Ok(())
    }

    fn open_book(&mut self, id: BookId) -> anyhow::Result<()> {
        let book = self.store.load_book(id)?;
        self.open_index(id);
        let mut ws = Workspace {
            book,
            tree: Tree::new(),
            editor: None,
            focus: Focus::Tree,
            show_tree: true,
            modals: ModalStack::default(),
            batch: None,
            proof_issues: Vec::new(),
            completion: None,
            hit: Hit::default(),
            tree_width: TREE_WIDTH,
        };
        // 打开首章，让用户直接能写——而不是对着空白发呆。
        if let Some(first) = ws.book.volumes.iter().flat_map(|v| &v.chapters).next() {
            let first_id = first.id;
            ws.tree.focus_chapter(&ws.book, first_id);
            self.screen = Screen::Workspace(Box::new(ws));
            self.open_chapter(first_id)?;
        } else {
            self.screen = Screen::Workspace(Box::new(ws));
        }
        Ok(())
    }

    // ---- 工作区 ----

    fn on_key_workspace(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // 浮层：**栈顶那层**吃掉所有按键（§7.1）。
        //
        // 从前这里是一条手写死的优先级链（先查 confirm、再 diff、再 history……），
        // 那实质上是把 z 序编码进了 if 的顺序——加一层就得记得插对位置。
        // 现在顺序由栈本身决定：谁最后压进来谁在上面。
        let top = match &self.screen {
            Screen::Workspace(ws) => ws.modals.top_kind(),
            _ => None,
        };
        if let Some(kind) = top {
            return match kind {
                ModalKind::Confirm => self.on_key_confirm(code),
                ModalKind::Consent => self.on_key_consent(code),
                ModalKind::Diff => self.on_key_diff(code),
                ModalKind::History => self.on_key_history(code),
                ModalKind::Search => self.on_key_search(code, mods),
                ModalKind::FormatPreview => self.on_key_format_preview(code),
                ModalKind::Proof => self.on_key_proof(code),
                ModalKind::CharacterForm => self.on_key_character_form(code, mods),
                ModalKind::Characters => self.on_key_character(code, mods),
                ModalKind::Stats => self.on_key_stats(code),
                ModalKind::Palette => self.on_key_palette(code, mods),
                ModalKind::Help => self.on_key_help(code),
                ModalKind::Settings => self.on_key_settings(code),
            };
        }

        // Ctrl+P 命令面板：**先于键位表**判，且不进键位表。
        //
        // 它是通往所有命令的入口。若允许被重绑覆盖，用户一旦把别的命令绑到 Ctrl+P，
        // 就再也没有地方能找回其余命令了——那是个自锁。故这一个键留死。
        if code == KeyCode::Char('p') && mods.contains(KeyModifiers::CONTROL) {
            if let Screen::Workspace(ws) = &mut self.screen {
                ws.modals.push(Modal::Palette(Box::default()));
            }
            return Ok(());
        }

        // 其余全局键一律走键位表（§7.3 [MUST] 可重绑定）。
        //
        // 从前这里是一长串写死的 `KeyCode::F(7) => ...`，那样键位就绑死在代码里，
        // `[keymap]` 配了也不生效。现在按键先查表拿到命令，再交给 run_command——
        // 于是「命令表 → 键位表 → 执行」是一条链，帮助页上写的键就是真按得出效果的键。
        if let Some(cmd) = self.keymap.lookup(code, mods) {
            return self.run_command(cmd);
        }

        // Ctrl+Shift+S 打快照：§7.3 指定的键，但**传统键盘模式下根本到不了**
        // （终端对 Ctrl+S 与 Ctrl+Shift+S 发同一个字节，Shift 没编码进去）。
        // 留着这条分支，开了 kitty 键盘协议它自然就活；实际入口是 F9。
        if code == KeyCode::Char('S') && mods.contains(KeyModifiers::CONTROL) {
            return self.manual_snapshot();
        }

        // Tab 切焦点：上下文键，不占命令表。
        if code == KeyCode::Tab {
            if let Screen::Workspace(ws) = &mut self.screen {
                ws.focus = match ws.focus {
                    Focus::Tree => Focus::Editor,
                    Focus::Editor => Focus::Tree,
                };
            }
            return Ok(());
        }

        let focus = match &self.screen {
            Screen::Workspace(ws) => ws.focus,
            _ => return Ok(()),
        };

        match focus {
            Focus::Tree => self.on_key_tree(code, mods),
            Focus::Editor => self.on_key_editor(code, mods),
        }
    }

    // ---- 批量作业（§6.5 全卷/全书排版、§6.6 全卷/全书替换）----

    /// 收集范围内的章。
    fn chapters_in_scope(&self, scope: Scope) -> Vec<ChapterId> {
        let Screen::Workspace(ws) = &self.screen else {
            return Vec::new();
        };
        let current = ws.editor.as_ref().map(|o| o.id);

        // 受损章一律跳过（ADR 0004）：它的 front matter 读不出来，
        // 硬写回去等于把还能人工救的内容覆盖掉。
        let ok = |c: &&mj_core::model::ChapterMeta| c.damaged.is_none();

        match scope {
            Scope::Chapter => current.into_iter().collect(),
            Scope::Volume => current
                .and_then(|ch| ws.book.find_chapter(ch))
                .map(|(v, _)| v.chapters.iter().filter(ok).map(|c| c.id).collect())
                .unwrap_or_default(),
            Scope::Book => ws
                .book
                .volumes
                .iter()
                .flat_map(|v| &v.chapters)
                .filter(ok)
                .map(|c| c.id)
                .collect(),
        }
    }

    /// 宽范围作业先过一道确认；当前章范围直接开跑。
    fn confirm_batch(&mut self, kind: BatchKind, scope: Scope) -> anyhow::Result<()> {
        if !scope.is_wide() {
            return self.start_batch(kind, scope);
        }
        let n = self.chapters_in_scope(scope).len();
        if n == 0 {
            self.toast = Some("范围内没有可处理的章节".into());
            return Ok(());
        }
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::Confirm(Box::new(Confirm::new(kind, scope, n))));
        }
        Ok(())
    }

    /// 确认框上的按键。
    fn on_key_confirm(&mut self, code: KeyCode) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(c) = ws.modals.confirm_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => c.toggle(),
            KeyCode::Esc | KeyCode::Char('n') => {
                ws.modals.close_kind(ModalKind::Confirm);
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                // 'y' 是明示的「就是要」，不看光标停在哪。
                let go = code == KeyCode::Char('y') || c.is_yes();
                let Some(c) = ws.modals.take_confirm() else {
                    return Ok(());
                };
                if go {
                    return self.start_batch(c.kind, c.scope);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 启动一个批量作业。
    fn start_batch(&mut self, kind: BatchKind, scope: Scope) -> anyhow::Result<()> {
        // 先把当前章存了——批量作业直接读写磁盘，缓冲里没落盘的字会被绕过去。
        self.save_current()?;

        let chapters = self.chapters_in_scope(scope);
        if chapters.is_empty() {
            self.toast = Some("范围内没有可处理的章节".into());
            return Ok(());
        }
        let job = BatchJob::new(kind, scope, chapters);
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.close_kind(ModalKind::FormatPreview);
            ws.modals.close_kind(ModalKind::Search);
            ws.batch = Some(job);
        }
        Ok(())
    }

    /// 推进批量作业。
    ///
    /// 每次只干一小会儿（`BATCH_SLICE`）就返回，让主循环有机会重绘进度条、
    /// 响应 Esc（§6.5 `[MUST]` 可中断）。**每章独立事务**：快照 → 改 → 存，
    /// 一章一轮；中途中断的话，做完的那些章各自都是完整的。
    fn step_batch(&mut self) -> anyhow::Result<()> {
        const BATCH_SLICE: std::time::Duration = std::time::Duration::from_millis(30);
        let deadline = std::time::Instant::now() + BATCH_SLICE;

        loop {
            let Screen::Workspace(ws) = &mut self.screen else {
                return Ok(());
            };
            let Some(job) = &mut ws.batch else {
                return Ok(());
            };
            if job.is_done() {
                return self.finish_batch();
            }
            let Some(ch) = job.next_chapter() else {
                return self.finish_batch();
            };

            let book = ws.book.id;
            if let Err(e) = self.process_one(book, ch) {
                // 一章出错不该让整本书的操作前功尽弃。
                if let Screen::Workspace(ws) = &mut self.screen
                    && let Some(job) = &mut ws.batch
                {
                    job.record_failure(ch, e.to_string());
                }
                tracing::warn!(chapter = %ch, error = %e, "批量作业跳过一章");
            }
            self.dirty = true;

            if std::time::Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    /// 处理一章：快照 → 改 → 存。
    fn process_one(&mut self, book: BookId, ch: ChapterId) -> anyhow::Result<()> {
        let (kind, retention) = {
            let Screen::Workspace(ws) = &self.screen else {
                return Ok(());
            };
            let Some(job) = &ws.batch else { return Ok(()) };
            (job.kind.clone(), self.config.history.retention)
        };

        let body = self.store.load_body(book, ch)?;
        let text = body.text.to_string();

        // 算出新正文。
        let new_text = match &kind {
            BatchKind::Format(opts) => mj_text::format::format(&text, opts),
            BatchKind::Replace { query, to } => {
                let edits = mj_text::search::replace_preview(&text, query, to)?;
                if edits.is_empty() {
                    text.clone()
                } else {
                    mj_text::format::apply(&text, &edits)
                }
            }
        };

        if new_text == text {
            if let Screen::Workspace(ws) = &mut self.screen
                && let Some(job) = &mut ws.batch
            {
                job.record_unchanged();
            }
            return Ok(());
        }

        // §6.6 [MUST]：执行前每章各打一条快照。
        let snap = self
            .history_of(book)
            .snapshot(ch, &text, kind.trigger(), None, retention)?;
        // 去重时 snapshot 返回 None——那说明链上最后一条就是当前内容，
        // 拿它的 id 即可（内容寻址：id 就是内容的哈希）。
        let before = snap
            .map(|s| s.id)
            .unwrap_or_else(|| mj_core::history::SnapshotId::of(&text));

        self.store
            .save_body(book, &mj_core::model::ChapterBody::new(ch, &new_text))?;

        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(job) = &mut ws.batch
        {
            job.record_change(ch, before);
        }
        Ok(())
    }

    /// 作业收工。
    fn finish_batch(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(job) = ws.batch.take() else {
            return Ok(());
        };
        let summary = job.summary();
        let undo = BatchUndo::from_job(&job);
        let book = ws.book.id;

        self.batch_undo = undo;
        // 元数据变了（字数），重载让树跟上。
        if let Ok(b) = self.store.load_book(book)
            && let Screen::Workspace(ws) = &mut self.screen
        {
            ws.book = b;
        }
        // 当前章的正文可能被改过了——重新载入，否则缓冲还是旧的，
        // 一保存就把批量的结果覆盖回去。
        let current = match &self.screen {
            Screen::Workspace(ws) => ws.editor.as_ref().map(|o| o.id),
            _ => None,
        };
        if let Some(id) = current {
            self.open_chapter(id)?;
        }
        self.toast = Some(summary);
        self.dirty = true;
        Ok(())
    }

    /// Alt+U：撤销本次批量操作（§6.6 `[MUST]`）。
    ///
    /// 一次性把所有受影响的章回滚到操作前的快照。
    fn undo_batch(&mut self) -> anyhow::Result<()> {
        if self.batch_undo.is_none() {
            self.toast = Some("没有可撤销的批量操作".into());
            return Ok(());
        }

        // 撤销本身也是破坏性的：它拿快照盖掉磁盘上现在的内容。
        //
        // 批量跑完之后用户完全可能又在当前章写了几段——那些字不在任何快照里，
        // 直接回滚就是 §0 的「静默丢稿」。故先落盘 + 打一条快照，
        // 撤销之后仍能从 F8 里把它捞回来。别的章没开编辑器，动不了，
        // 它们的操作前状态本来就在快照里。
        self.save_current()?;
        self.take_snapshot(
            mj_core::history::Trigger::Manual,
            Some("撤销批量操作前".into()),
        );

        let Some(undo) = self.batch_undo.take() else {
            return Ok(());
        };
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        let h = self.history_of(book);

        let mut ok = 0usize;
        let mut failed = 0usize;
        for (ch, snap) in &undo.entries {
            match h.read(snap) {
                Ok(old) => {
                    match self
                        .store
                        .save_body(book, &mj_core::model::ChapterBody::new(*ch, &old))
                    {
                        Ok(()) => ok += 1,
                        Err(e) => {
                            failed += 1;
                            tracing::warn!(chapter = %ch, error = %e, "批量撤销：写回失败");
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!(chapter = %ch, error = %e, "批量撤销：读不出快照");
                }
            }
        }

        if let Ok(b) = self.store.load_book(book)
            && let Screen::Workspace(ws) = &mut self.screen
        {
            ws.book = b;
        }
        let current = match &self.screen {
            Screen::Workspace(ws) => ws.editor.as_ref().map(|o| o.id),
            _ => None,
        };
        if let Some(id) = current {
            self.open_chapter(id)?;
        }

        self.toast = Some(if failed == 0 {
            format!("已撤销本次{}，{ok} 章回到操作前", undo.kind_label)
        } else {
            // 据实说哪几章没退回去——用户得知道去历史面板手动处理。
            format!("撤销了 {ok} 章，{failed} 章失败（见日志，可在 F8 历史里手动恢复）")
        });
        Ok(())
    }

    // ---- 版本历史（§6.9）----

    /// 当前书的历史库。
    fn history_of(&self, book: BookId) -> mj_core::history::History {
        mj_core::history::History::new(&self.store.workspace().books_dir().join(book.to_string()))
    }

    /// 打一条快照。
    ///
    /// 快照失败**绝不打断用户的操作**：历史是附加价值，正文才是命根子。
    /// 记日志 + 提示一句就够了——不能因为磁盘满就让人排不了版。
    fn take_snapshot(&mut self, trigger: mj_core::history::Trigger, label: Option<String>) {
        let retention = self.config.history.retention;
        let Screen::Workspace(ws) = &self.screen else {
            return;
        };
        let (Some(open), book) = (&ws.editor, ws.book.id) else {
            return;
        };
        let (ch, text) = (open.id, open.buffer.contents());

        match self
            .history_of(book)
            .snapshot(ch, &text, trigger, label, retention)
        {
            Ok(Some(_)) => {}
            // 去重：内容与上一条相同，没新建。这是正常的。
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, ?trigger, "打快照失败");
                self.toast = Some(format!("快照失败（{e}），但正文不受影响"));
            }
        }
    }

    /// Ctrl+Shift+S：手动快照（§6.9）。
    ///
    /// §6.9 说「可填标签」，而输入浮层属 M6——故先按触发来源存，
    /// 标签留到历史面板里补（F8 内可加）。给个能用的，而不是干等 M6。
    fn manual_snapshot(&mut self) -> anyhow::Result<()> {
        if !matches!(&self.screen, Screen::Workspace(ws) if ws.editor.is_some()) {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        }
        self.take_snapshot(mj_core::history::Trigger::Manual, None);
        self.last_snapshot = Some((std::time::Instant::now(), self.current_words()));
        self.toast = Some("已打快照，F8 查看历史".into());
        Ok(())
    }

    fn current_words(&self) -> usize {
        match &self.screen {
            Screen::Workspace(ws) => ws.editor.as_ref().map(|o| o.word_count.with_punct),
            _ => None,
        }
        .unwrap_or(0)
    }

    /// 自动快照（§6.9）：每 N 分钟**且**自上次快照后净变更 ≥ M 字。
    ///
    /// 「且」不是「或」——两个条件都满足才打，否则历史会被刷屏，
    /// 而刷屏的历史等于没有历史（用户翻不动）。
    fn maybe_auto_snapshot(&mut self) {
        let interval = std::time::Duration::from_secs(self.config.history.auto_interval_min * 60);
        let min_words = self.config.history.auto_min_words;
        let now = std::time::Instant::now();
        let words = self.current_words();

        let Some((last_at, last_words)) = self.last_snapshot else {
            // 还没打过：以当前状态为基准，别一开章就打一条。
            self.last_snapshot = Some((now, words));
            return;
        };
        if now.duration_since(last_at) < interval {
            return;
        }
        if words.abs_diff(last_words) < min_words {
            return;
        }

        self.take_snapshot(mj_core::history::Trigger::Auto, None);
        self.last_snapshot = Some((now, words));
    }

    /// F8：打开历史面板。
    fn open_history(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let (Some(open), book) = (&ws.editor, ws.book.id) else {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        };
        let snaps = self.history_of(book).list(open.id);

        if snaps.is_empty() {
            // 这里从前写的是 Ctrl+Shift+S——而那个键在传统键盘模式下**根本按不出来**
            // （§7.3 的注）。提示里给一个按不了的键，等于让用户以为功能坏了。
            self.toast = Some("本章还没有快照（Ctrl+S 保存，或 F9 手动打一条）".into());
            return Ok(());
        }
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::History(Box::new(HistoryPanel::new(snaps))));
        }
        Ok(())
    }

    /// 历史面板的按键。
    fn on_key_history(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // Enter 要读 blob，先脱离对 ws 的借用。
        if code == KeyCode::Enter {
            return self.open_diff();
        }
        if matches!(code, KeyCode::Char('P')) {
            return self.toggle_pin();
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.history_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::F(8) | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::History);
            }
            KeyCode::Down | KeyCode::Char('j') => p.move_down(),
            KeyCode::Up | KeyCode::Char('k') => p.move_up(),
            KeyCode::Char(' ') => p.toggle_compare(),
            _ => {}
        }
        Ok(())
    }

    /// `P`：钉住/取消当前快照。钉住的永不淘汰（§6.9）。
    fn toggle_pin(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let (Some(p), Some(open), book) = (ws.modals.history(), &ws.editor, ws.book.id) else {
            return Ok(());
        };
        let Some(s) = p.selected() else { return Ok(()) };
        let (id, now_pinned, ch) = (s.id.clone(), s.pinned, open.id);

        let h = self.history_of(book);
        if let Err(e) = h.set_pinned(ch, &id, !now_pinned) {
            self.toast = Some(format!("操作失败：{e}"));
            return Ok(());
        }
        // 重载面板，让标记跟上。
        let snaps = h.list(ch);
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::History(Box::new(HistoryPanel::new(snaps))));
        }
        self.toast = Some(if now_pinned {
            "已取消钉住".into()
        } else {
            "已钉住，此快照永不淘汰".to_string()
        });
        Ok(())
    }

    /// 历史面板里 Enter：打开 diff（§6.9）。
    ///
    /// 默认对比**该快照 vs 当前版本**——用户原话「与现版本相比哪里做了改动」。
    /// 若用 Space 选了第二条，则两条快照互比。
    fn open_diff(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let (Some(p), Some(open), book) = (ws.modals.history(), &ws.editor, ws.book.id) else {
            return Ok(());
        };
        let Some(sel) = p.selected() else {
            return Ok(());
        };

        let h = self.history_of(book);
        let old_text = match h.read(&sel.id) {
            Ok(t) => t,
            Err(e) => {
                self.toast = Some(format!("读不出这条快照：{e}"));
                return Ok(());
            }
        };
        let old_title = snapshot_title(sel);

        // Space 选了对照条 → 两条快照互比；否则与当前版本比。
        let (new_title, new_text) = match p.compare_target() {
            Some(other) => match h.read(&other.id) {
                Ok(t) => (snapshot_title(other), t),
                Err(e) => {
                    self.toast = Some(format!("读不出对照的快照：{e}"));
                    return Ok(());
                }
            },
            None => ("当前版本".to_string(), open.buffer.contents()),
        };

        let view = DiffView::new(old_title, old_text, new_title, new_text);
        if let Screen::Workspace(ws) = &mut self.screen {
            // 压在历史面板**之上**，不关它：Esc 弹掉 diff 就回到历史（§7.1）。
            ws.modals.push(Modal::Diff(Box::new(view)));
        }
        Ok(())
    }

    /// diff 界面的按键（§6.9、§12.4）。
    fn on_key_diff(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // 会改正文的先脱离借用。
        match code {
            KeyCode::Char('u') => return self.restore_hunk(),
            KeyCode::Char('U') => return self.restore_whole_chapter(),
            KeyCode::Char('y') => return self.copy_old_content(),
            _ => {}
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(v) = ws.modals.diff_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::Diff);
            }
            KeyCode::Char('n') => v.next_hunk(),
            KeyCode::Char('p') => v.prev_hunk(),
            KeyCode::Down | KeyCode::Char('j') => v.scroll_down(),
            KeyCode::Up | KeyCode::Char('k') => v.scroll_up(),
            _ => {}
        }
        Ok(())
    }

    /// `u`：单块恢复（§6.9 恢复粒度 2）。
    fn restore_hunk(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let Some(v) = ws.modals.diff() else {
            return Ok(());
        };
        let Some(edit) = v.restore_hunk_edit() else {
            self.toast = Some("没有可恢复的改动块".into());
            return Ok(());
        };

        // 恢复前先给当前版本打一条快照——回退本身也可回退（§6.9）。
        self.take_snapshot(mj_core::history::Trigger::Manual, None);

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };
        open.buffer.replace_ranges(&[edit]);
        let text = open.buffer.contents();
        open.word_count = mj_text::count::count(&text);
        open.autosave.on_edit(std::time::Instant::now());
        // 恢复是终结动作：diff 与它底下的历史面板一起收掉，让用户看见正文。
        // （历史列表此刻也已过期——刚才恢复前又打了一条快照。）
        ws.modals.clear();

        self.toast = Some("已恢复此块，Ctrl+Z 可撤销".into());
        Ok(())
    }

    /// `U`：整章恢复（§6.9 恢复粒度 1）。
    fn restore_whole_chapter(&mut self) -> anyhow::Result<()> {
        let old = match &self.screen {
            Screen::Workspace(ws) => ws.modals.diff().map(|v| v.old_text().to_string()),
            _ => None,
        };
        let Some(old) = old else { return Ok(()) };

        // §6.9 明言：恢复前自动给当前版本打一次快照——**回退本身也可回退**。
        // 这条不是锦上添花：用户点 U 的时候正慌，而慌的时候最容易点错。
        self.take_snapshot(mj_core::history::Trigger::Manual, None);

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };
        // 整章替换算一个撤销组。
        let all = 0..open.buffer.len_bytes();
        open.buffer.replace_ranges(&[(all, old)]);
        let text = open.buffer.contents();
        open.word_count = mj_text::count::count(&text);
        open.autosave.on_edit(std::time::Instant::now());
        // 同上：恢复完把浮层全收掉，回到正文。
        ws.modals.clear();

        self.toast = Some("已恢复整章。恢复前的版本已存为快照，可再退回去".into());
        Ok(())
    }

    /// `y`：复制旧内容到剪贴板，不改当前版本（§6.9 恢复粒度 3）。
    fn copy_old_content(&mut self) -> anyhow::Result<()> {
        let text = match &self.screen {
            Screen::Workspace(ws) => ws.modals.diff().map(|v| v.copy_text()),
            _ => None,
        };
        let Some(text) = text else { return Ok(()) };

        let n = mj_text::count::count_with_punct(&text);
        crate::clipboard::copy(&text);
        self.toast = Some(format!("已复制 {n} 字到剪贴板"));
        Ok(())
    }

    // ---- 查找替换（§6.6）----

    /// Ctrl+F / Ctrl+H。
    fn open_search(&mut self, replace_mode: bool) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        if ws.editor.is_none() {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        }
        ws.modals
            .push(Modal::Search(Box::new(SearchPanel::new(replace_mode))));
        Ok(())
    }

    /// 查找面板的按键。
    fn on_key_search(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // 会改正文的两个动作要先脱离对 ws 的借用。
        match code {
            KeyCode::Char('r') if mods.contains(KeyModifiers::ALT) => return self.replace_one(),
            KeyCode::Char('a') if mods.contains(KeyModifiers::ALT) => {
                return self.replace_checked();
            }
            _ => {}
        }

        // F4 切范围（§6.6）。
        if code == KeyCode::F(4) {
            if let Screen::Workspace(ws) = &mut self.screen
                && let Some(p) = ws.modals.search_mut()
            {
                p.scope = p.scope.next();
            }
            return Ok(());
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.search_mut() else {
            return Ok(());
        };
        let text = ws.editor.as_ref().map(|o| o.buffer.contents());

        let mut need_refresh = false;
        match code {
            KeyCode::Esc => {
                ws.modals.close_kind(ModalKind::Search);
                return Ok(());
            }
            KeyCode::Tab => p.next_field(),
            // Enter：跳到当前命中处（§6.6）。
            KeyCode::Enter => {
                let target = p.current_hit().map(|h| h.range.start);
                if let Some(pos) = target {
                    ws.modals.close_kind(ModalKind::Search);
                    if let Some(open) = &mut ws.editor {
                        open.buffer.move_to(pos);
                        open.viewport.scroll_to_cursor(&open.buffer);
                        ws.focus = Focus::Editor;
                    }
                }
                return Ok(());
            }
            KeyCode::Down => p.move_down(),
            KeyCode::Up => p.move_up(),
            KeyCode::Char(' ') if p.field() == search_panel::Field::Results => p.toggle_check(),
            KeyCode::Backspace => need_refresh = p.backspace(),
            KeyCode::F(2) => {
                p.cycle_mode();
                need_refresh = true;
            }
            KeyCode::F(6) => {
                p.flags.ignore_case = !p.flags.ignore_case;
                need_refresh = true;
            }
            KeyCode::F(7) => {
                p.flags.fold_width = !p.flags.fold_width;
                need_refresh = true;
            }
            KeyCode::F(8) => {
                p.flags.fold_cjk_punct = !p.flags.fold_cjk_punct;
                need_refresh = true;
            }
            KeyCode::Char(c) => need_refresh = p.input_char(c),
            _ => {}
        }

        // §6.6：非法正则**实时**提示——故每次改动都重查。
        if need_refresh && let Some(t) = &text {
            p.refresh(t);
        }
        Ok(())
    }

    /// Alt+R：只替换当前这一条。
    fn replace_one(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.search_mut() else {
            return Ok(());
        };
        let Some(edit) = p.current_edit() else {
            self.toast = Some("没有可替换的命中".into());
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };

        open.buffer.replace_ranges(&[edit]);
        let text = open.buffer.contents();
        open.word_count = mj_text::count::count(&text);
        open.autosave.on_edit(std::time::Instant::now());

        // 正文变了，命中的坐标全失效——必须立刻重查，
        // 否则下一次替换会砍在错的位置上。
        p.refresh(&text);
        self.toast = Some("已替换 1 处".into());
        Ok(())
    }

    /// Alt+A：替换全部勾选（§6.6）。
    fn replace_checked(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.search_mut() else {
            return Ok(());
        };

        // 范围超出当前章：走批量作业，勾选与否不作数。
        //
        // 结果列表里只有当前章的命中，勾选是**这一章**的事；全卷/全书的命中
        // 从没在界面上出现过，拿当前章的勾选去推别的章是没有依据的。所以宽范围
        // 一律「全换」，并在确认框里把话说明白。
        if p.scope.is_wide() {
            if p.query.is_empty() {
                self.toast = Some("先输入要查找的内容".into());
                return Ok(());
            }
            let kind = BatchKind::Replace {
                query: mj_text::search::Query {
                    pattern: p.query.clone(),
                    mode: p.mode,
                    flags: p.flags,
                },
                to: p.replace_with.clone(),
            };
            let scope = p.scope;
            return self.confirm_batch(kind, scope);
        }

        let edits = p.checked_edits();
        if edits.is_empty() {
            self.toast = Some("没有勾选任何命中".into());
            return Ok(());
        }

        // §6.6 [MUST]：替换前强制打快照。
        //
        // 批量替换是最容易一把毁掉一章的操作——一个写错的正则，几十处一起改。
        // 撤销栈只活在本次会话里，快照才是关掉程序之后还在的后路。
        self.take_snapshot(mj_core::history::Trigger::BeforeReplace, None);

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.search_mut() else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };

        // 一次批量替换 = 一个撤销组（与排版同理，§6.3）。
        let n = edits.len();
        open.buffer.replace_ranges(&edits);
        let text = open.buffer.contents();
        open.word_count = mj_text::count::count(&text);
        open.autosave.on_edit(std::time::Instant::now());

        p.refresh(&text);
        self.toast = Some(format!("已替换 {n} 处，Ctrl+Z 撤销 / F8 看快照"));
        Ok(())
    }

    // ---- 命令面板与帮助（§7.3）----

    fn on_key_palette(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // Enter 要先把面板关掉再执行命令——否则命令若自己开浮层，
        // 会叠在一个马上就要消失的面板上。
        if code == KeyCode::Enter {
            let cmd = match &self.screen {
                Screen::Workspace(ws) => ws.modals.palette().and_then(|p| p.selected()),
                _ => None,
            };
            if let Screen::Workspace(ws) = &mut self.screen {
                ws.modals.close_kind(ModalKind::Palette);
            }
            if let Some(cmd) = cmd {
                return self.run_command(cmd);
            }
            return Ok(());
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.palette_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Esc => {
                ws.modals.close_kind(ModalKind::Palette);
            }
            KeyCode::Down => p.move_down(),
            KeyCode::Up => p.move_up(),
            KeyCode::Backspace => p.backspace(),
            KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => p.input_char(c),
            _ => {}
        }
        Ok(())
    }

    fn on_key_help(&mut self, code: KeyCode) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(h) = ws.modals.help_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::Help);
            }
            KeyCode::Down | KeyCode::Char('j') => h.scroll_down(),
            KeyCode::Up | KeyCode::Char('k') => h.scroll_up(),
            _ => {}
        }
        Ok(())
    }

    /// 执行一条命令（§7.3：所有功能都要能从命令面板触达）。
    ///
    /// 这里必须覆盖 `commands::COMMANDS` 的每一条——表里有而这里没有的分支，
    /// 就是一个在面板里点了没反应的命令。`command_coverage` 测试盯着这件事。
    pub fn run_command(&mut self, cmd: Command) -> anyhow::Result<()> {
        match cmd {
            Command::Save => self.save_current(),
            Command::Snapshot => self.manual_snapshot(),
            Command::NewChapter => self.new_chapter(),
            Command::BackToShelf => self.back_to_shelf(),
            Command::Quit => {
                self.save_current()?;
                self.should_quit = true;
                Ok(())
            }
            Command::Undo => {
                self.editor_undo_redo(true);
                Ok(())
            }
            Command::Redo => {
                self.editor_undo_redo(false);
                Ok(())
            }
            Command::Find => self.open_search(false),
            Command::Replace => self.open_search(true),
            Command::Format => self.open_format_preview(),
            Command::UndoBatch => self.undo_batch(),
            Command::Proof => self.open_proof(),
            Command::ProofLlm => self.start_llm_proof(),
            Command::History => self.open_history(),
            Command::Characters => self.open_characters(),
            Command::Stats => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.modals.push(Modal::Stats(Box::new(Stats::new())));
                }
                Ok(())
            }
            Command::ToggleTree => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.show_tree = !ws.show_tree;
                    if !ws.show_tree {
                        ws.focus = Focus::Editor;
                    }
                }
                Ok(())
            }
            Command::Help => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.modals.push(Modal::Help(Box::default()));
                }
                Ok(())
            }
            Command::Appearance => self.open_settings(),
            Command::Export => self.export_book(),
            Command::NextChapter => self.step_chapter(1),
            Command::PrevChapter => self.step_chapter(-1),
            Command::FocusMode => {
                self.focus_mode = !self.focus_mode;
                if let Screen::Workspace(ws) = &mut self.screen {
                    // 专注模式收起目录树并把焦点交给正文——留着树却不显示，
                    // 会出现「按键打不进正文」的怪状态。
                    ws.show_tree = !self.focus_mode;
                    if self.focus_mode {
                        ws.focus = Focus::Editor;
                    }
                }
                self.toast = Some(
                    if self.focus_mode {
                        "专注模式：开（F11 退出）"
                    } else {
                        "专注模式：关"
                    }
                    .into(),
                );
                Ok(())
            }
        }
    }

    /// 打开外观设置（§6.10）。
    fn open_settings(&mut self) -> anyhow::Result<()> {
        // 可选主题 = 内置 + 用户自建，去重。用户自建的同名主题会覆盖内置，
        // 故只留一份名字。
        let mut themes: Vec<String> = crate::theme::builtin_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        for t in self.store.workspace().list_user_themes() {
            if !themes.contains(&t) {
                themes.push(t);
            }
        }
        let a = &self.config.appearance;
        let s = Settings::new(
            themes,
            &a.theme,
            crate::font::TerminalKind::detect(),
            a.column_width,
            a.margin,
            a.paragraph_spacing,
            a.line_number,
            a.font_family.clone(),
            a.font_size,
        );
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.push(Modal::Settings(Box::new(s)));
        }
        Ok(())
    }

    fn on_key_settings(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // 复制片段与关页面都要脱离对 ws 的借用。
        match code {
            KeyCode::Char('y') => {
                let snip = match &self.screen {
                    Screen::Workspace(ws) => ws
                        .modals
                        .settings()
                        .filter(|s| s.current_row() == settings::Row::Snippet)
                        .and_then(|s| s.snippet().map(|x| x.to_string())),
                    _ => None,
                };
                if let Some(snip) = snip {
                    crate::clipboard::copy(&snip);
                    self.toast = Some("配置片段已复制（终端不回话，粘贴一下确认）".into());
                }
                return Ok(());
            }
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => return self.close_settings(),
            _ => {}
        }

        // 换主题要立刻生效——当场看得到，才谈得上「挑」主题。
        let changed = {
            let Screen::Workspace(ws) = &mut self.screen else {
                return Ok(());
            };
            let Some(s) = ws.modals.settings_mut() else {
                return Ok(());
            };
            match code {
                KeyCode::Down | KeyCode::Char('j') => {
                    s.move_down();
                    false
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    s.move_up();
                    false
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => s.cycle_theme(true),
                KeyCode::Left | KeyCode::Char('h') => s.cycle_theme(false),
                _ => false,
            }
        };
        if changed {
            let name = match &self.screen {
                Screen::Workspace(ws) => ws.modals.settings().map(|s| s.theme().to_string()),
                _ => None,
            };
            if let Some(name) = name {
                self.config.appearance.theme = name;
                self.theme = Self::resolve_theme(&self.store, &self.config);
            }
        }
        Ok(())
    }

    /// 关设置页。主题改过就写回 config.toml。
    fn close_settings(&mut self) -> anyhow::Result<()> {
        let dirty = match &self.screen {
            Screen::Workspace(ws) => ws.modals.settings().is_some_and(|s| s.is_dirty()),
            _ => false,
        };
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.close_kind(ModalKind::Settings);
        }
        if dirty {
            let path = self.store.workspace().config_file();
            match self.config.save(&path) {
                Ok(()) => {
                    self.toast = Some(format!("主题已存为「{}」", self.config.appearance.theme))
                }
                Err(e) => self.toast = Some(format!("主题已切换，但写盘失败：{e}")),
            }
        }
        Ok(())
    }

    /// 撤销/重做当前正文。
    fn editor_undo_redo(&mut self, undo: bool) {
        let Screen::Workspace(ws) = &mut self.screen else {
            return;
        };
        let Some(open) = &mut ws.editor else {
            self.toast = Some("没有打开的章节".into());
            return;
        };
        if undo {
            open.buffer.undo();
        } else {
            open.buffer.redo();
        }
        open.word_count = mj_text::count::count(&open.buffer.contents());
        open.autosave.on_edit(std::time::Instant::now());
        ws.proof_issues.clear();
        open.viewport.scroll_to_cursor(&open.buffer);
    }

    /// Ctrl+N：在当前卷末尾新建一章。
    ///
    /// 沿用新建书的做法给占位标题——输入浮层（§7.1 的 `Input`）还没做，
    /// 而「不能新建章」比「章名要事后改」更挡路。
    fn new_chapter(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        // 优先落在当前章所属的卷，其次末卷。
        let vol = ws
            .editor
            .as_ref()
            .and_then(|o| ws.book.find_chapter(o.id))
            .map(|(v, _)| v.id)
            .or_else(|| ws.book.volumes.last().map(|v| v.id));
        let Some(vol) = vol else {
            self.toast = Some("这本书还没有卷，无处新建章".into());
            return Ok(());
        };
        let last = ws
            .book
            .volumes
            .iter()
            .find(|v| v.id == vol)
            .and_then(|v| v.chapters.last())
            .map(|c| c.id);

        let id = self.store.create_chapter(book, vol, "新章", last)?;
        // 重载书树，让新章出现在目录里。
        let reloaded = self.store.load_book(book)?;
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.book = reloaded;
        }
        self.open_chapter(id)?;
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.focus = Focus::Editor;
        }
        self.toast = Some("已新建「新章」".into());
        Ok(())
    }

    /// 上/下一章（§7.3 的 Ctrl+Tab）。`delta` 为 +1/-1。
    ///
    /// 按**全书阅读顺序**跨卷走，而不是只在当前卷里打转——读者读的是一条线，
    /// 翻到卷末自然该进下一卷。
    fn step_chapter(&mut self, delta: i32) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let all: Vec<ChapterId> = ws
            .book
            .volumes
            .iter()
            .flat_map(|v| &v.chapters)
            .map(|c| c.id)
            .collect();
        let Some(current) = ws.editor.as_ref().map(|o| o.id) else {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        };
        let Some(idx) = all.iter().position(|c| *c == current) else {
            return Ok(());
        };
        let next = idx as i32 + delta;
        if next < 0 || next as usize >= all.len() {
            self.toast = Some(if delta > 0 {
                "已经是最后一章".into()
            } else {
                "已经是第一章".into()
            });
            return Ok(());
        }
        self.open_chapter(all[next as usize])?;
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.focus = Focus::Editor;
        }
        Ok(())
    }

    /// 导出全书为 Markdown（§12.2）。
    ///
    /// 路径不问用户：输入浮层（§7.1 的 `Input`）还没做，而「导出到哪」这种事
    /// 给个确定的默认位置再把路径**告诉他**，比弹一个填路径的框更省事。
    /// 想要别的位置和格式，`mj export` 有完整参数。
    fn export_book(&mut self) -> anyhow::Result<()> {
        // 先保存，免得导出的是磁盘上的旧版本（§0 禁令 1 的精神）。
        self.save_current()?;
        let Screen::Workspace(ws) = &self.screen else {
            self.toast = Some("先打开一本书".into());
            return Ok(());
        };
        let (id, title) = (ws.book.id, ws.book.title.clone());
        let name = format!("{}.md", mj_core::slug::slugify(&title));
        let path = self.store.workspace().root().join(&name);
        match mj_core::export::export_to_file(&self.store, id, mj_core::export::Format::Md, &path) {
            Ok(()) => self.toast = Some(format!("已导出到 {}", path.display())),
            Err(e) => self.toast = Some(format!("导出失败：{e}")),
        }
        Ok(())
    }

    /// 回书架（先保存，§0 禁令 1）。
    fn back_to_shelf(&mut self) -> anyhow::Result<()> {
        self.save_current()?;
        let books = self.store.list_books()?;
        self.screen = Screen::Shelf(Shelf::new(books));
        Ok(())
    }

    // ---- 角色（§6.7）----

    /// Alt+C：载入本书角色，弹速查侧栏。
    fn open_characters(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        let chars = self.store.list_characters(book).unwrap_or_default();
        let n = chars.len();
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::Characters(Box::new(CharacterPanel::new(chars))));
        }
        if n == 0 {
            self.toast = Some("还没有角色，按 n 新建".into());
        }
        Ok(())
    }

    fn on_key_character(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // 会写盘/扫盘的动作先脱离对 ws 的借用。
        match code {
            KeyCode::Char('n') if !self.character_searching() => {
                return self.new_character();
            }
            KeyCode::Char('d') if !self.character_searching() => {
                return self.delete_character();
            }
            KeyCode::Char('e') if !self.character_searching() => {
                return self.edit_character();
            }
            // t：出场统计。已在统计视图则收起；否则扫全书算一次（§6.7 [SHOULD]）。
            KeyCode::Char('t') if !self.character_searching() => {
                return self.toggle_appearance_stats();
            }
            _ => {}
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.characters_mut() else {
            return Ok(());
        };

        // 搜索输入模式：字符进搜索框。
        if p.is_searching() {
            match code {
                KeyCode::Esc | KeyCode::Enter => p.end_search(),
                KeyCode::Backspace => p.backspace(),
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => p.input_char(c),
                KeyCode::Down => p.move_down(),
                KeyCode::Up => p.move_up(),
                _ => {}
            }
            return Ok(());
        }

        match code {
            // 统计视图里 Esc/q 先回到卡片列表，再按才关面板。
            KeyCode::Esc | KeyCode::Char('q') if p.show_stats() => p.clear_stats(),
            KeyCode::Esc | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::Characters);
            }
            KeyCode::Char('/') if !p.show_stats() => p.start_search(),
            KeyCode::Down | KeyCode::Char('j') => p.move_down(),
            KeyCode::Up | KeyCode::Char('k') => p.move_up(),
            _ => {}
        }
        Ok(())
    }

    /// `t`：出场统计视图开关。开时扫全书数每个角色的提及次数。
    fn toggle_appearance_stats(&mut self) -> anyhow::Result<()> {
        let showing = matches!(&self.screen, Screen::Workspace(ws) if ws.modals.characters().is_some_and(|p| p.show_stats()));
        if showing {
            if let Screen::Workspace(ws) = &mut self.screen
                && let Some(p) = ws.modals.characters_mut()
            {
                p.clear_stats();
            }
            return Ok(());
        }
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        let stats = mj_core::appearance::count_appearances(&self.store, book).unwrap_or_default();
        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(p) = ws.modals.characters_mut()
        {
            p.set_stats(stats);
        }
        Ok(())
    }

    fn character_searching(&self) -> bool {
        matches!(&self.screen, Screen::Workspace(ws) if ws.modals.characters().is_some_and(|p| p.is_searching()))
    }

    /// `n`：新建角色。沿用新建书的做法——先给占位名，之后再改（输入浮层属 M6）。
    fn new_character(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        self.store.create_character(book, "新角色")?;
        // 重新载入面板。
        let chars = self.store.list_characters(book).unwrap_or_default();
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::Characters(Box::new(CharacterPanel::new(chars))));
        }
        self.toast = Some("已新建「新角色」".into());
        Ok(())
    }

    /// `d`：删除当前角色（移入 trash，§0 可撤销）。
    fn delete_character(&mut self) -> anyhow::Result<()> {
        let (book, id, name) = {
            let Screen::Workspace(ws) = &self.screen else {
                return Ok(());
            };
            let Some(p) = ws.modals.characters() else {
                return Ok(());
            };
            match p.current() {
                Some(c) => (ws.book.id, c.id, c.name.clone()),
                None => return Ok(()),
            }
        };
        self.store.delete_character(book, id)?;
        let chars = self.store.list_characters(book).unwrap_or_default();
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals
                .push(Modal::Characters(Box::new(CharacterPanel::new(chars))));
        }
        self.toast = Some(format!("已删除「{name}」（在 trash 内，可找回）"));
        Ok(())
    }

    /// `e`：编辑当前角色，打开表单（§6.7 [MUST] 表单式编辑）。
    fn edit_character(&mut self) -> anyhow::Result<()> {
        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(c) = ws.modals.characters().and_then(|p| p.current()).cloned()
        {
            // 压在列表**之上**，不关列表：Esc 弹掉表单就回到列表（§7.1）。
            ws.modals
                .push(Modal::CharacterForm(Box::new(CharacterForm::new(c))));
        }
        Ok(())
    }

    fn on_key_character_form(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // Ctrl+S 存盘要脱离对 ws 的借用。
        if code == KeyCode::Char('s') && mods.contains(KeyModifiers::CONTROL) {
            return self.save_character_form();
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(form) = ws.modals.character_form_mut() else {
            return Ok(());
        };

        if form.is_editing() {
            match code {
                KeyCode::Esc => form.stop_editing(),
                KeyCode::Enter => {
                    if form.focused_field().key.is_multiline() {
                        form.input_newline();
                    } else {
                        form.stop_editing();
                    }
                }
                KeyCode::Backspace => form.backspace(),
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => form.input_char(c),
                _ => {}
            }
            return Ok(());
        }

        match code {
            // 退表单：改了没存要给个提醒，但不拦——列表还能再进来（M6 浮层栈里再做确认）。
            KeyCode::Esc | KeyCode::Char('q') => {
                let dirty = form.is_dirty();
                self.close_character_form()?;
                if dirty {
                    self.toast = Some("已放弃未保存的改动".into());
                }
                return Ok(());
            }
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => form.next_field(),
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => form.prev_field(),
            KeyCode::Enter | KeyCode::Char('i') => form.start_editing(),
            _ => {}
        }
        Ok(())
    }

    /// Ctrl+S：把表单写回角色卡。
    fn save_character_form(&mut self) -> anyhow::Result<()> {
        let (book, character) = {
            let Screen::Workspace(ws) = &self.screen else {
                return Ok(());
            };
            let Some(form) = ws.modals.character_form() else {
                return Ok(());
            };
            (ws.book.id, form.to_character())
        };
        self.store.save_character(book, &character)?;
        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(form) = ws.modals.character_form_mut()
        {
            form.mark_saved();
        }
        self.toast = Some(format!("已保存「{}」", character.name));
        Ok(())
    }

    /// 关表单，回到角色列表（重新载入，反映刚存的改动）。
    fn close_character_form(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let book = ws.book.id;
        let chars = self.store.list_characters(book).unwrap_or_default();
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.close_kind(ModalKind::CharacterForm);
            // 底下那层列表要用刚存的数据刷新——名字/身份可能just改过。
            match ws.modals.characters_mut() {
                Some(p) => *p = CharacterPanel::new(chars),
                None => ws
                    .modals
                    .push(Modal::Characters(Box::new(CharacterPanel::new(chars)))),
            }
        }
        Ok(())
    }

    // ---- 校对（§6.8）----

    /// 忽略表，懒加载并缓存。
    fn ignore_set(&mut self) -> &mut mj_core::proofing::IgnoreSet {
        let path = self.store.workspace().ignore_file();
        self.ignore
            .get_or_insert_with(|| mj_core::proofing::IgnoreSet::load(&path))
    }

    /// F7：校对当前章。
    ///
    /// 手动触发，就地同步跑——本地规则对单章是亚毫秒级，远在 §9 的 16ms 预算内，
    /// 且这不是打字过程（§6.8 [MUST] 说的「绝不在打字时同步跑」指的是自动模式）。
    fn open_proof(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let Some(open) = &ws.editor else {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        };
        let book = ws.book.id;
        let text = open.buffer.contents();

        // 专名上下文（角色名 + 用户词典）——校对不误报的前提（§6.7）。
        let ctx = mj_core::proofing::build_context(&self.store, self.store.workspace(), book)
            .unwrap_or_default();
        let proofer =
            mj_core::proofing::Proofer::from_workspace(self.store.workspace(), &self.config);
        let ignore = {
            let path = self.store.workspace().ignore_file();
            self.ignore
                .get_or_insert_with(|| mj_core::proofing::IgnoreSet::load(&path))
        };
        let result = proofer
            .check_chapter(&text, &ctx, ignore, &mj_text::proof::CancelToken::new())
            .unwrap_or_default();

        let fold = self.config.proof.fold_below;
        let n = result.issues.len();
        let warning = result.warning.clone();
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.proof_issues = result.issues.clone();
            ws.modals
                .push(Modal::Proof(Box::new(ProofPanel::new(result.issues, fold))));
        }
        // 外部后端出了岔子要说一声，但校对本身照常完成（§6.8「绝不影响编辑」）。
        self.toast = Some(match warning {
            Some(w) => format!("校对完成：{n} 处待看（外部后端：{w}）"),
            None if n == 0 => "校对完成：未发现问题".into(),
            None => format!("校对完成：{n} 处待看"),
        });
        Ok(())
    }

    // ---- 鼠标（§13 [SHOULD]）----
    //
    // 一条总原则：鼠标**不新增语义**。滚轮 = 那块区域本来就有的滚动，点击 = 把
    // 选中挪过去再做 Enter 会做的事。这样「§13 [MUST] 所有功能不依赖鼠标」自动成立
    // ——鼠标能做的键盘全都能做，而且两边行为不可能走偏，因为走的是同一段代码。

    /// 滚轮一格滚几行。三行是通例。
    const WHEEL_LINES: usize = 3;

    /// 配置里开没开鼠标（§13）。`run` 据此决定要不要捕获。
    pub fn mouse_enabled(&self) -> bool {
        self.config.input.mouse
    }

    pub(crate) fn on_mouse(&mut self, m: MouseEvent) -> anyhow::Result<()> {
        match m.kind {
            MouseEventKind::ScrollUp => self.wheel(true, m.column, m.row),
            MouseEventKind::ScrollDown => self.wheel(false, m.column, m.row),
            MouseEventKind::Down(MouseButton::Left) => return self.mouse_click(m.column, m.row),
            MouseEventKind::Drag(MouseButton::Left) => self.drag_divider(m.column),
            MouseEventKind::Up(_) => self.dragging_divider = false,
            // 其余（中键、右键、移动）暂不接管，留给终端自己。
            _ => return Ok(()),
        }
        Ok(())
    }

    /// 滚轮。有浮层就滚浮层，否则按落点滚树或正文。
    fn wheel(&mut self, up: bool, col: u16, row: u16) {
        // 浮层：直接喂它本来就吃的上下键——列表怎么滚由它自己说了算。
        if matches!(&self.screen, Screen::Workspace(ws) if !ws.modals.is_empty()) {
            let code = if up { KeyCode::Up } else { KeyCode::Down };
            for _ in 0..Self::WHEEL_LINES {
                // 走键盘那条路：栈顶浮层吃键，滚轮于是天然等于按上下键。
                let _ = self.on_key_workspace(code, KeyModifiers::NONE);
            }
            return;
        }

        match &mut self.screen {
            Screen::Shelf(shelf) => {
                for _ in 0..Self::WHEEL_LINES {
                    if up {
                        shelf.move_up();
                    } else {
                        shelf.move_down();
                    }
                }
            }
            Screen::Workspace(ws) => {
                let over_tree = ws.hit.tree.is_some_and(|(r, _)| Self::within(r, col, row));
                if over_tree {
                    // 树没有独立的滚动位置，滚动就是移选中——键盘上也是这样。
                    for _ in 0..Self::WHEEL_LINES {
                        if up {
                            ws.tree.move_up();
                        } else {
                            ws.tree.move_down(&ws.book);
                        }
                    }
                } else if let Some(open) = &mut ws.editor {
                    // 正文滚的是**视口**，不动光标——滚轮看两眼别处不该把光标带走。
                    if up {
                        open.viewport.scroll_up(Self::WHEEL_LINES);
                    } else {
                        open.viewport.scroll_down(Self::WHEEL_LINES, &open.buffer);
                    }
                }
            }
        }
    }

    /// 左键按下。
    fn mouse_click(&mut self, col: u16, row: u16) -> anyhow::Result<()> {
        // 浮层开着时不接管点击：浮层盖住的坐标算不清，点错比不响应更糟。
        if matches!(&self.screen, Screen::Workspace(ws) if !ws.modals.is_empty()) {
            return Ok(());
        }
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };

        // 分隔条要在选行**之前**判：它就是侧栏的右边框，那一列同时落在树的
        // 命中框里。先判行的话，想拖分隔条会变成选中最后点到的那一章。
        if let Some((area, _)) = ws.hit.tree
            && col == area.x + area.width.saturating_sub(1)
        {
            self.dragging_divider = true;
            return Ok(());
        }

        if let Some((area, top)) = ws.hit.tree
            && Self::within(area, col, row)
        {
            // 边框各占一行/一列，内容从 area.y + 1 起。
            let Some(offset) = row.checked_sub(area.y + 1) else {
                return Ok(());
            };
            let idx = top + offset as usize;
            if idx >= ws.tree.rows(&ws.book).len() {
                return Ok(()); // 点在空白处
            }
            ws.focus = Focus::Tree;
            let book = &ws.book;
            ws.tree.set_cursor(idx, book);
            // 之后交给 Enter 的那条路：章就打开，卷就折叠/展开。
            return self.on_key_tree(KeyCode::Enter, KeyModifiers::NONE);
        }

        if Self::within(ws.hit.editor, col, row)
            && let Some(open) = &mut ws.editor
        {
            ws.focus = Focus::Editor;
            if let Some(byte) = Self::byte_at(open, ws.hit.body, col, row) {
                open.buffer.clear_selection();
                open.buffer.move_to(byte);
            }
        }
        Ok(())
    }

    /// 屏幕坐标 → 缓冲字节位置。落在行尾之后就贴到行尾。
    ///
    /// 照 `visible_lines` 的排版结果反查，而不是自己再算一遍换行——正文按显示宽度
    /// 折行、CJK 占两列、段间距还会撑出不对应任何字节的空行，重算一遍必然对不上。
    fn byte_at(open: &OpenChapter, area: Rect, col: u16, row: u16) -> Option<usize> {
        use unicode_segmentation::UnicodeSegmentation as _;
        use unicode_width::UnicodeWidthStr as _;

        let lines = open.viewport.visible_lines(&open.buffer);
        let line = lines.get(row.checked_sub(area.y)? as usize)?;
        let text = open
            .buffer
            .text()
            .byte_slice(line.range.clone())
            .to_string();
        let text = text.as_str();
        // 目标列减去渲染时补的缩进；点在缩进里就算行首。
        let target = usize::from(col.saturating_sub(area.x)).saturating_sub(line.indent);

        // 按显示宽度往前走，走过目标列就是它。字素簇为单位（§0：光标按字素簇动）。
        let mut used = 0usize;
        for (off, g) in text.grapheme_indices(true) {
            let w = g.width().max(1);
            if used + w > target {
                return Some(line.range.start + off);
            }
            used += w;
        }
        // 点在行尾之后：贴到行尾。
        Some(line.range.end)
    }

    /// 拖分隔条（§13）。松手前每动一列就跟着走一列。
    fn drag_divider(&mut self, col: u16) {
        if !self.dragging_divider {
            return;
        }
        let Screen::Workspace(ws) = &mut self.screen else {
            return;
        };
        let Some((area, _)) = ws.hit.tree else { return };
        // 鼠标在哪一列，右边框就到哪一列——宽度是「从侧栏左沿到这里」再加 1。
        let total = area.width + ws.hit.editor.width;
        ws.tree_width =
            (col.saturating_sub(area.x) + 1).clamp(TREE_MIN_WIDTH, tree_max_width(total));
    }

    fn within(r: Rect, col: u16, row: u16) -> bool {
        col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
    }

    // ---- 模型校对（§6.8 第 3 条）----

    /// 正文指纹。工作线程回来时用它判断结果还作不作数。
    ///
    /// 借 mj-core 的哈希而不在这里算：§4 分层里 blake3 只归 mj-core。
    fn text_fingerprint(text: &str) -> String {
        mj_core::index::content_hash(text)
    }

    /// 「模型校对当前章」。§6.8 `[MUST]` 只手动触发，不做全书自动扫描。
    ///
    /// 三道闸：没开就说怎么开、开了没同意就先弹同意框、都齐了才发。
    fn start_llm_proof(&mut self) -> anyhow::Result<()> {
        if self.llm_job.is_some() {
            self.toast = Some("模型校对正在跑，Esc 可取消".into());
            return Ok(());
        }
        let cfg = &self.config.proof.llm;
        if !cfg.enabled {
            self.toast =
                Some("模型校对没开。在 config.toml 里设 [proof.llm] enabled = true".into());
            return Ok(());
        }
        // §6.8 [MUST]：首次开启必须弹说明并获得明确同意。
        if !cfg.consented {
            let c = Consent::new(
                cfg.endpoint.clone(),
                cfg.model.clone(),
                cfg.api_key_env.clone(),
            );
            if let Screen::Workspace(ws) = &mut self.screen {
                ws.modals.push(Modal::Consent(Box::new(c)));
            }
            return Ok(());
        }
        self.spawn_llm_proof()
    }

    /// 真的把请求发出去。调用前必须已确认 enabled + consented。
    fn spawn_llm_proof(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let Some(open) = &ws.editor else {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        };
        let (book, chapter, text) = (ws.book.id, open.id, open.buffer.contents());

        let proofer =
            mj_core::proofing::Proofer::from_workspace(self.store.workspace(), &self.config);
        // 配齐没有（密钥、环境变量、明文密钥），在发之前问清楚。
        if let Some(problem) = proofer.llm_setup_problem() {
            self.toast = Some(problem);
            return Ok(());
        }
        let Some(tx) = self.events_tx.clone() else {
            // 只有测试里的 App 没接事件循环。
            self.toast = Some("事件循环未就绪".into());
            return Ok(());
        };

        let ctx = mj_core::proofing::build_context(&self.store, self.store.workspace(), book)
            .unwrap_or_default();
        let ignore = {
            let path = self.store.workspace().ignore_file();
            self.ignore
                .get_or_insert_with(|| mj_core::proofing::IgnoreSet::load(&path))
                .clone()
        };
        let cancel = mj_text::proof::CancelToken::new();
        let text_hash = Self::text_fingerprint(&text);

        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("mj-llm-proof".into())
            .spawn(move || {
                let out = proofer.check_chapter_llm(&text, &ctx, &ignore, &worker_cancel);
                // 送不回去说明主循环已经退了，丢掉即可。
                let _ = tx.send(AppEvent::LlmProof(Box::new(LlmProofDone {
                    chapter,
                    text_hash,
                    issues: out.issues,
                    warning: out.warning,
                })));
            })?;

        self.llm_job = Some(LlmJob { cancel, chapter });
        self.toast = Some("正在请求模型校对……（Esc 取消）".into());
        Ok(())
    }

    /// 工作线程回来了。
    ///
    /// **先验指纹再用**：请求跑了好几秒，这期间用户可能改了正文或换了章，而
    /// `issues` 里是当时那份文本的字节偏移。对不上就整份丢掉——拿旧坐标往新正文
    /// 上画，下划线会落在毫不相干的字上，比不显示更糟。
    fn on_llm_proof_done(&mut self, done: LlmProofDone) {
        let job = self.llm_job.take();
        if job.is_none_or(|j| j.chapter != done.chapter) {
            return; // 已取消，或早就换了章
        }
        let Screen::Workspace(ws) = &self.screen else {
            return;
        };
        let Some(open) = &ws.editor else { return };
        if open.id != done.chapter
            || Self::text_fingerprint(&open.buffer.contents()) != done.text_hash
        {
            self.toast = Some("正文已改动，模型校对结果作废，请重跑".into());
            return;
        }

        let n = done.issues.len();
        let fold = self.config.proof.fold_below;
        if let Screen::Workspace(ws) = &mut self.screen {
            // 并进本地规则那一趟的结果，按位置排好——UI 是按位置列的。
            ws.proof_issues.extend(done.issues);
            ws.proof_issues.sort_by(|a, b| {
                a.range
                    .start
                    .cmp(&b.range.start)
                    .then(a.range.end.cmp(&b.range.end))
            });
            let all = ws.proof_issues.clone();
            ws.modals.close_kind(ModalKind::Proof);
            ws.modals
                .push(Modal::Proof(Box::new(ProofPanel::new(all, fold))));
        }
        self.toast = Some(match done.warning {
            Some(w) => format!("模型校对：新增 {n} 处（{w}）"),
            None if n == 0 => "模型校对：未发现问题".into(),
            None => format!("模型校对：新增 {n} 处"),
        });
    }

    // ---- 输入框：改名（§6.1、§6.2）----

    /// 发起改名：预填原名，开输入框。
    fn start_rename(&mut self, title: impl Into<String>, intent: InputIntent) {
        self.input = Some(Input::new(rename_prompt(intent), title, intent));
    }

    /// 发起删除确认：不预填，等用户敲确认串（书名 / `y`）。
    fn start_delete_confirm(&mut self, prompt: impl Into<String>, intent: InputIntent) {
        self.input = Some(Input::new(prompt, "", intent));
    }

    /// `Alt+↑/↓`：把选中的章往上/下挪一位（§6.2 [MUST]「上下移动、跨卷移动」）。
    ///
    /// 一个动作管两件事：卷内相邻两章对调；到了卷边界就跨过去（下到下一卷的顶、
    /// 上到上一卷的底）。于是同两个键就能把一章一路挪到全书任何位置，不必另开
    /// 「移动到…」的选择框。
    fn nudge_chapter(&mut self, up: bool) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        // 只对选中的是「章」时动作。
        let Some(Row::Chapter { id: ch, .. }) = ws.tree.selected(&ws.book) else {
            return Ok(());
        };
        let book = ws.book.id;
        // 找到 ch 所在卷、卷内下标，以及相邻卷。
        let vols = &ws.book.volumes;
        let Some(vi) = vols
            .iter()
            .position(|v| v.chapters.iter().any(|c| c.id == ch))
        else {
            return Ok(());
        };
        let sibs = &vols[vi].chapters;
        let j = sibs.iter().position(|c| c.id == ch).unwrap_or(0);

        // 算出目标卷 + 排在谁之后（after=None 表示排到卷首）。
        let target: Option<(VolumeId, Option<ChapterId>)> = if up {
            if j > 0 {
                // 卷内上移：排到前一章之前 = 排在「前前一章」之后。
                let after = if j >= 2 { Some(sibs[j - 2].id) } else { None };
                Some((vols[vi].id, after))
            } else if vi > 0 {
                // 卷首再上：挪到上一卷的末尾。
                let prev = &vols[vi - 1];
                Some((prev.id, prev.chapters.last().map(|c| c.id)))
            } else {
                None // 已是全书第一章
            }
        } else if j + 1 < sibs.len() {
            // 卷内下移：排到下一章之后。
            Some((vols[vi].id, Some(sibs[j + 1].id)))
        } else if vi + 1 < vols.len() {
            // 卷尾再下：挪到下一卷的开头。
            Some((vols[vi + 1].id, None))
        } else {
            None // 已是全书最后一章
        };

        let Some((target_vol, after)) = target else {
            return Ok(());
        };
        if let Err(e) = self.store.move_chapter(book, ch, target_vol, after) {
            self.toast = Some(format!("移动失败：{e}"));
            return Ok(());
        }
        // 重载书树，并让光标跟着这一章走——好连着按 Alt+↓ 一路挪。
        if let Ok(b) = self.store.load_book(book)
            && let Screen::Workspace(ws) = &mut self.screen
        {
            ws.book = b;
            ws.tree.focus_chapter(&ws.book, ch);
        }
        Ok(())
    }

    /// 输入框吃键。返回 true 表示这一键被输入框消费了（`on_key` 据此提前返回）。
    fn input_handles_key(&mut self, code: KeyCode) -> anyhow::Result<bool> {
        let Some(input) = &mut self.input else {
            return Ok(false);
        };
        match code {
            KeyCode::Esc => {
                self.input = None;
            }
            KeyCode::Backspace => input.backspace(),
            KeyCode::Char(c) => input.input_char(c),
            KeyCode::Enter => {
                if let Some(input) = self.input.take() {
                    self.submit_input(input.value().trim(), input.intent())?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// 输入框提交：按 intent 分派——改名还是删除。
    fn submit_input(&mut self, value: &str, intent: InputIntent) -> anyhow::Result<()> {
        match intent {
            InputIntent::RenameBook(_)
            | InputIntent::RenameVolume(_)
            | InputIntent::RenameChapter(_) => self.submit_rename(value, intent),
            InputIntent::DeleteBook(_)
            | InputIntent::DeleteVolume(_)
            | InputIntent::DeleteChapter(_) => self.submit_delete(value, intent),
        }
    }

    /// 落地一次改名：调 store，刷新内存里的书树/书架。
    fn submit_rename(&mut self, name: &str, intent: InputIntent) -> anyhow::Result<()> {
        if name.is_empty() {
            self.toast = Some("名字不能为空，未改动".into());
            return Ok(());
        }
        // 书 id：改书名在书架上，用 intent 自带的；改卷/章在工作区，用当前书。
        let result = match intent {
            InputIntent::RenameBook(id) => self.store.rename_book(id, name),
            InputIntent::RenameVolume(id) => match &self.screen {
                Screen::Workspace(ws) => self.store.rename_volume(ws.book.id, id, name),
                _ => Ok(()),
            },
            InputIntent::RenameChapter(id) => match &self.screen {
                Screen::Workspace(ws) => self.store.rename_chapter(ws.book.id, id, name),
                _ => Ok(()),
            },
            _ => Ok(()),
        };
        match result {
            Ok(()) => {
                self.refresh_after_structure_change();
                self.toast = Some(format!("已重命名为「{name}」"));
            }
            Err(e) => self.toast = Some(format!("改名失败：{e}")),
        }
        Ok(())
    }

    /// 落地一次删除。确认串不符就什么都不做（§6.1 [MUST]：删书要输书名）。
    fn submit_delete(&mut self, typed: &str, intent: InputIntent) -> anyhow::Result<()> {
        let yes = typed.eq_ignore_ascii_case("y");
        let (result, gone) = match intent {
            InputIntent::DeleteBook(id) => {
                // 确认串必须**正好等于书名**，不是随便一个 y。
                let title = self.store.load_book(id).ok().map(|b| b.title);
                if title.as_deref() != Some(typed) {
                    self.toast = Some("书名不符，未删除".into());
                    return Ok(());
                }
                (self.store.delete_book(id), None)
            }
            InputIntent::DeleteVolume(id) => {
                if !yes {
                    self.toast = Some("已取消，未删除".into());
                    return Ok(());
                }
                let book = match &self.screen {
                    Screen::Workspace(ws) => ws.book.id,
                    _ => return Ok(()),
                };
                (self.store.delete_volume(book, id), None)
            }
            InputIntent::DeleteChapter(id) => {
                if !yes {
                    self.toast = Some("已取消，未删除".into());
                    return Ok(());
                }
                let book = match &self.screen {
                    Screen::Workspace(ws) => ws.book.id,
                    _ => return Ok(()),
                };
                (self.store.delete_chapter(book, id), Some(id))
            }
            _ => return Ok(()),
        };
        match result {
            Ok(()) => {
                // 删的正是打开着的那一章，就把编辑器合上——不能对着已进 trash 的文件。
                if let Some(ch) = gone
                    && let Screen::Workspace(ws) = &mut self.screen
                    && ws.editor.as_ref().is_some_and(|o| o.id == ch)
                {
                    ws.editor = None;
                    ws.focus = Focus::Tree;
                }
                self.refresh_after_structure_change();
                self.toast = Some("已删除，可在 trash 里找回".into());
            }
            Err(e) => self.toast = Some(format!("删除失败：{e}")),
        }
        Ok(())
    }

    /// 磁盘上的结构变了（改名/删除），把内存里的视图（书树或书架）同步过来。
    fn refresh_after_structure_change(&mut self) {
        match &self.screen {
            Screen::Workspace(ws) => {
                let book = ws.book.id;
                if let Ok(b) = self.store.load_book(book)
                    && let Screen::Workspace(ws) = &mut self.screen
                {
                    ws.book = b;
                }
            }
            Screen::Shelf(shelf) => {
                // 保住当前选中项，别让改完/删完光标跳走。
                let keep = shelf.selected_id();
                if let Ok(books) = self.store.list_books()
                    && let Screen::Shelf(shelf) = &mut self.screen
                {
                    shelf.reload(books, keep);
                }
            }
        }
    }

    /// 同意框上的按键。默认停在「不同意」。
    fn on_key_consent(&mut self, code: KeyCode) -> anyhow::Result<()> {
        let confirmed = match code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.modals.take_consent();
                }
                self.toast = Some("已取消，正文没有发出去".into());
                return Ok(());
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => true,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                if let Screen::Workspace(ws) = &mut self.screen
                    && let Some(Modal::Consent(c)) = ws.modals.top_mut()
                {
                    c.toggle();
                }
                return Ok(());
            }
            KeyCode::Enter => {
                let yes = matches!(self.screen_modal_consent(), Some(true));
                if !yes {
                    if let Screen::Workspace(ws) = &mut self.screen {
                        ws.modals.take_consent();
                    }
                    self.toast = Some("已取消，正文没有发出去".into());
                    return Ok(());
                }
                true
            }
            _ => return Ok(()),
        };
        if !confirmed {
            return Ok(());
        }

        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.take_consent();
        }
        // 记下来，下次不再问。写不进去也照跑这一趟——但要说一声，
        // 否则用户每次都被问一遍还不知道为什么。
        self.config.proof.llm.consented = true;
        let path = self.store.workspace().config_file();
        if let Err(e) = self.config.save(&path) {
            tracing::warn!(error = %e, "写不回 consented");
            self.toast = Some(format!("已同意，但写不回配置（{e}），下次还会再问"));
        }
        self.spawn_llm_proof()
    }

    /// 同意框当前停在哪个按钮。
    fn screen_modal_consent(&self) -> Option<bool> {
        match &self.screen {
            Screen::Workspace(ws) => match ws.modals.top() {
                Some(Modal::Consent(c)) => Some(c.is_yes()),
                _ => None,
            },
            _ => None,
        }
    }

    fn on_key_proof(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // 会改正文/写盘的动作先脱离对 ws 的借用。
        match code {
            KeyCode::Char('a') => return self.apply_proof_suggestion(),
            KeyCode::Char('I') => return self.ignore_proof_permanent(),
            _ => {}
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.proof_mut() else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::F(7) | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::Proof);
            }
            KeyCode::Down | KeyCode::Char('j') => p.move_down(),
            KeyCode::Up | KeyCode::Char('k') => p.move_up(),
            KeyCode::Char('f') => p.toggle_folded(),
            // i：本次忽略（只从列表摘掉，不落盘）。
            KeyCode::Char('i') => {
                if let Some(removed) = p.remove_current() {
                    ws.proof_issues
                        .retain(|i| !(i.range == removed.range && i.rule_id == removed.rule_id));
                    self.toast = Some("已忽略（本次）".into());
                }
            }
            // Enter：跳到问题处，关面板。
            KeyCode::Enter => {
                let target = p.current().map(|i| i.range.start);
                if let Some(pos) = target {
                    ws.modals.close_kind(ModalKind::Proof);
                    if let Some(open) = &mut ws.editor {
                        open.buffer.move_to(pos);
                        open.viewport.scroll_to_cursor(&open.buffer);
                        ws.focus = Focus::Editor;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `a`：把当前问题的首个建议应用到正文。
    fn apply_proof_suggestion(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.proof() else {
            return Ok(());
        };
        let Some(issue) = p.current() else {
            return Ok(());
        };
        let Some(suggestion) = issue.suggestions.first().cloned() else {
            self.toast = Some("这条问题没有可一键应用的建议".into());
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };

        open.buffer
            .replace_ranges(&[(issue.range.clone(), suggestion)]);
        let text = open.buffer.contents();
        open.word_count = mj_text::count::count(&text);
        open.autosave.on_edit(std::time::Instant::now());

        // 正文变了，所有 range 失效——重新校对，拿一份新的列表。
        self.reproof_current();
        self.toast = Some("已应用建议".into());
        Ok(())
    }

    /// `I`：永久忽略当前问题——算忽略键、写入 dict/ignore.json。
    fn ignore_proof_permanent(&mut self) -> anyhow::Result<()> {
        // 先取出问题与整章文本（算键要用整章文本）。
        let (issue, text) = {
            let Screen::Workspace(ws) = &self.screen else {
                return Ok(());
            };
            let (Some(p), Some(open)) = (ws.modals.proof(), &ws.editor) else {
                return Ok(());
            };
            match p.current() {
                Some(i) => (i, open.buffer.contents()),
                None => return Ok(()),
            }
        };

        let key = mj_core::proofing::ignore_key(&text, &issue);
        let path = self.store.workspace().ignore_file();
        let set = self.ignore_set();
        set.insert(key);
        if let Err(e) = set.save(&path) {
            self.toast = Some(format!("忽略已记下，但写盘失败：{e}"));
        } else {
            self.toast = Some("已永久忽略".into());
        }

        // 从当前列表与下划线集里摘掉。
        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(p) = ws.modals.proof_mut()
            && let Some(removed) = p.remove_current()
        {
            ws.proof_issues
                .retain(|i| !(i.range == removed.range && i.rule_id == removed.rule_id));
        }
        Ok(())
    }

    /// 重新校对当前章，替换面板里的问题列表（应用建议后调用）。
    fn reproof_current(&mut self) {
        let Screen::Workspace(ws) = &self.screen else {
            return;
        };
        let (Some(open), book) = (&ws.editor, ws.book.id) else {
            return;
        };
        let text = open.buffer.contents();
        let ctx = mj_core::proofing::build_context(&self.store, self.store.workspace(), book)
            .unwrap_or_default();
        let proofer =
            mj_core::proofing::Proofer::from_workspace(self.store.workspace(), &self.config);
        let path = self.store.workspace().ignore_file();
        let ignore = self
            .ignore
            .get_or_insert_with(|| mj_core::proofing::IgnoreSet::load(&path));
        let result = proofer
            .check_chapter(&text, &ctx, ignore, &mj_text::proof::CancelToken::new())
            .unwrap_or_default();
        let fold = self.config.proof.fold_below;
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.proof_issues = result.issues.clone();
            ws.modals
                .push(Modal::Proof(Box::new(ProofPanel::new(result.issues, fold))));
        }
    }

    // ---- 排版（§6.5）----

    /// F5：对当前章生成排版计划并弹出预览。
    fn open_format_preview(&mut self) -> anyhow::Result<()> {
        let opts = self.config.format.clone();
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &ws.editor else {
            self.toast = Some("没有打开的章节".into());
            return Ok(());
        };

        let text = open.buffer.contents();
        let edits = mj_text::format::plan(&text, &opts);

        if edits.is_empty() {
            // 说清楚「没有改动」，而不是弹一个空面板让用户猜。
            self.toast = Some("本章已符合排版规则，无需改动".into());
            return Ok(());
        }
        ws.modals
            .push(Modal::FormatPreview(Box::new(FormatPreview::new(
                &text, edits,
            ))));
        Ok(())
    }

    /// 排版预览的按键（§6.5：可逐条取消）。
    fn on_key_format_preview(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // Enter 要改正文，得先脱离对 ws 的借用。
        if code == KeyCode::Enter {
            return self.apply_format();
        }
        // V / B：把同一套规则施加到当前卷 / 全书（§6.5 [MUST] 范围）。
        //
        // 不做逐条预览：全书四百章的改动列表没人看得完，
        // 而 §6.5 给的保障是「进度条 + 可中断 + 每章打快照」，不是预览。
        match code {
            KeyCode::Char('V') => {
                let opts = self.config.format.clone();
                return self.confirm_batch(BatchKind::Format(opts), Scope::Volume);
            }
            KeyCode::Char('B') => {
                let opts = self.config.format.clone();
                return self.confirm_batch(BatchKind::Format(opts), Scope::Book);
            }
            _ => {}
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.format_preview_mut() else {
            return Ok(());
        };

        match code {
            KeyCode::Esc | KeyCode::F(5) | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::FormatPreview);
            }
            KeyCode::Down | KeyCode::Char('j') => p.move_down(),
            KeyCode::Up | KeyCode::Char('k') => p.move_up(),
            KeyCode::Char(' ') => p.toggle(),
            KeyCode::Char('a') => p.set_all(true),
            KeyCode::Char('n') => p.set_all(false),
            _ => {}
        }
        Ok(())
    }

    /// 应用勾选的排版改动。
    fn apply_format(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = ws.modals.take_format_preview() else {
            return Ok(());
        };

        let edits = p.selected_edits();
        if edits.is_empty() {
            self.toast = Some("没有勾选任何改动".into());
            return Ok(());
        }

        // §6.5 [MUST]：**执行前强制打快照**。
        //
        // 撤销栈只活在本次会话里——用户排完版关掉程序，就再也退不回去了。
        // 快照才是真正的后路。这条 M3 时欠着（没有快照），现在补上。
        self.take_snapshot(mj_core::history::Trigger::BeforeFormat, None);

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };

        // §6.5：一次排版 = 一个撤销组。Ctrl+Z 一次退干净。
        let n = edits.len();
        open.buffer.replace_ranges(&edits);
        open.word_count = mj_text::count::count(&open.buffer.contents());
        open.autosave.on_edit(std::time::Instant::now());
        open.viewport.scroll_to_cursor(&open.buffer);

        self.toast = Some(format!("已排版 {n} 处，Ctrl+Z 可整体撤销"));
        Ok(())
    }

    /// 统计面板的按键（§6.4）。
    fn on_key_stats(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // 导出前先把数据备好——借用检查不允许在持有 ws 的同时调 self 的方法。
        let export = matches!(code, KeyCode::Char('e'));
        if export {
            return self.export_csv();
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(stats) = ws.modals.stats_mut() else {
            return Ok(());
        };

        match code {
            KeyCode::Esc | KeyCode::F(3) | KeyCode::Char('q') => {
                ws.modals.close_kind(ModalKind::Stats);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let rows = Stats::rows(&ws.book, |_| 0).len();
                stats.scroll_down(rows, 20);
            }
            KeyCode::Up | KeyCode::Char('k') => stats.scroll_up(),
            _ => {}
        }
        Ok(())
    }

    /// 导出统计 CSV（§6.4 [MUST]）。
    fn export_csv(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &self.screen else {
            return Ok(());
        };
        let np = self.no_punct_lookup();
        let csv = stats::to_csv(&ws.book, np);

        // 落在 workspace 根目录，名字带书名与日期——用户要能一眼认出是哪份。
        let name = format!(
            "{}-字数统计-{}.csv",
            mj_core::slug::slugify(&ws.book.title),
            chrono::Local::now().format("%Y%m%d")
        );
        let path = self.store.workspace().root().join(&name);

        // 走原子写：导出中途断电不该留下半截 CSV（§0 禁令 1 的同源原则）。
        mj_core::atomic::write(&path, csv.as_bytes())?;
        self.toast = Some(format!("已导出 {name}"));
        Ok(())
    }

    /// 各章净字数的查表函数（取自索引）。
    ///
    /// front matter 只缓存了含标点的 `words`（§5.2），净字数在索引里。
    /// 索引不可用时返回 0——统计面板不该因为缓存没了就打不开。
    fn no_punct_lookup(&self) -> impl Fn(ChapterId) -> u64 + '_ {
        move |ch| {
            self.index
                .as_ref()
                .and_then(|i| i.chapter_no_punct(ch).ok().flatten())
                .unwrap_or(0)
        }
    }

    fn on_key_tree(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        // Alt+↑/↓：挪动选中的章（§6.2 [MUST]）。要 &mut self，先于下面的 ws 借用处理。
        if mods.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Up | KeyCode::Down) {
            return self.nudge_chapter(code == KeyCode::Up);
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };

        match code {
            KeyCode::Esc => {
                // 回书架前先保存——不能让未保存的字随着切屏消失（§0 禁令 1）。
                self.save_current()?;
                let books = self.store.list_books()?;
                self.screen = Screen::Shelf(Shelf::new(books));
            }
            KeyCode::Down | KeyCode::Char('j') => ws.tree.move_down(&ws.book),
            KeyCode::Up | KeyCode::Char('k') => ws.tree.move_up(),
            KeyCode::Char(' ') => ws.tree.toggle_check(&ws.book),
            // `r`：给选中的卷/章改名（§6.2 [MUST]、§7.1 树内 `r`）。
            KeyCode::Char('r') => match ws.tree.selected(&ws.book) {
                Some(Row::Volume { id, title, .. }) => {
                    self.start_rename(title, InputIntent::RenameVolume(id));
                }
                Some(Row::Chapter {
                    id, title, damaged, ..
                }) => {
                    if damaged {
                        self.toast = Some("该章元数据损坏，改名前需先人工修复".into());
                    } else {
                        self.start_rename(title, InputIntent::RenameChapter(id));
                    }
                }
                None => {}
            },
            // `d`：删选中的卷/章（软删到 trash，删前敲 y 确认）（§6.2 [MUST]）。
            KeyCode::Char('d') => match ws.tree.selected(&ws.book) {
                Some(Row::Volume {
                    id,
                    title,
                    chapter_count,
                    ..
                }) => {
                    self.start_delete_confirm(
                        format!("删除卷《{title}》（含 {chapter_count} 章）？输入 y 确认"),
                        InputIntent::DeleteVolume(id),
                    );
                }
                Some(Row::Chapter { id, title, .. }) => {
                    self.start_delete_confirm(
                        format!("删除章《{title}》？输入 y 确认"),
                        InputIntent::DeleteChapter(id),
                    );
                }
                None => {}
            },
            KeyCode::Left => ws.tree.toggle(&ws.book),
            KeyCode::Right | KeyCode::Enter => {
                match ws.tree.selected(&ws.book) {
                    Some(Row::Volume { .. }) => ws.tree.toggle(&ws.book),
                    Some(Row::Chapter { id, damaged, .. }) => {
                        if damaged {
                            // 受损章不可编辑（ADR 0004）——但要说清为什么。
                            self.toast = Some("该章元数据损坏，已拒绝打开以免覆盖正文".into());
                        } else {
                            self.open_chapter(id)?;
                            if let Screen::Workspace(ws) = &mut self.screen {
                                ws.focus = Focus::Editor;
                            }
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// @ 补全激活时的按键处理。返回 true = 本次按键已被补全吃掉，编辑器不再处理。
    ///
    /// 过滤串是 `@` 之后到光标之间那段正文（实时躺在缓冲里，不另存）。
    fn handle_completion_key(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<bool> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(false);
        };
        let (Some(comp), Some(open)) = (&mut ws.completion, &mut ws.editor) else {
            return Ok(false);
        };
        let at = comp.at();
        let cursor = open.buffer.cursor();
        // 光标退到 `@` 或更前：补全作废。
        if cursor <= at {
            ws.completion = None;
            return Ok(false);
        }
        let filter = open
            .buffer
            .contents()
            .get(at + 1..cursor)
            .unwrap_or("")
            .to_string();

        // 编辑后统一收尾。
        let finish_edit = |open: &mut OpenChapter, issues: &mut Vec<mj_text::proof::Issue>| {
            open.word_count = mj_text::count::count(&open.buffer.contents());
            open.autosave.on_edit(std::time::Instant::now());
            issues.clear();
            open.viewport.scroll_to_cursor(&open.buffer);
        };

        match code {
            KeyCode::Up => {
                comp.move_up();
                Ok(true)
            }
            KeyCode::Down => {
                comp.move_down(&filter);
                Ok(true)
            }
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(name) = comp.selected(&filter) {
                    // 用所选名字替换 `@` + 过滤串。
                    open.buffer.replace_ranges(&[(at..cursor, name.clone())]);
                    open.buffer.move_to(at + name.len());
                    finish_edit(open, &mut ws.proof_issues);
                }
                ws.completion = None;
                Ok(true)
            }
            KeyCode::Esc => {
                // 取消补全，留下字面 `@文本`。
                ws.completion = None;
                Ok(true)
            }
            KeyCode::Backspace => {
                open.buffer.delete_backward();
                let nc = open.buffer.cursor();
                if nc <= at {
                    ws.completion = None;
                } else {
                    let f = open
                        .buffer
                        .contents()
                        .get(at + 1..nc)
                        .unwrap_or("")
                        .to_string();
                    comp.clamp(&f);
                }
                finish_edit(open, &mut ws.proof_issues);
                Ok(true)
            }
            KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) && !c.is_whitespace() => {
                open.buffer.insert(&c.to_string());
                let nc = open.buffer.cursor();
                let f = open
                    .buffer
                    .contents()
                    .get(at + 1..nc)
                    .unwrap_or("")
                    .to_string();
                // 敲到没有任何候选就退出补全（用户在打的不是名字）。
                if comp.candidates(&f).is_empty() {
                    ws.completion = None;
                } else {
                    comp.clamp(&f);
                }
                finish_edit(open, &mut ws.proof_issues);
                Ok(true)
            }
            // 其余键（空格、方向左右等）：关补全，让编辑器照常处理这次按键。
            _ => {
                ws.completion = None;
                Ok(false)
            }
        }
    }

    fn on_key_editor(&mut self, code: KeyCode, _mods: KeyModifiers) -> anyhow::Result<()> {
        // @ 补全激活时先给它处理；它可能吃掉本次按键。
        if matches!(&self.screen, Screen::Workspace(ws) if ws.completion.is_some())
            && self.handle_completion_key(code, _mods)?
        {
            return Ok(());
        }

        // 敲 `@` 前把角色名备好（放在可变借用 ws 之前，避开借用冲突）。
        let at_names = if code == KeyCode::Char('@') && !_mods.contains(KeyModifiers::CONTROL) {
            match &self.screen {
                Screen::Workspace(ws) => {
                    let book = ws.book.id;
                    mj_core::proofing::build_context(&self.store, self.store.workspace(), book)
                        .map(|c| c.names)
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };

        let mut edited = false;
        match code {
            KeyCode::Esc => {
                // 有选区时 Esc 先取消选择，再按才回树——
                // 否则「取消选择」这个最常见的动作没有键可用。
                if open.buffer.selection().is_some() {
                    open.buffer.clear_selection();
                } else {
                    ws.focus = Focus::Tree;
                }
                return Ok(());
            }
            KeyCode::Char('z') if _mods.contains(KeyModifiers::CONTROL) => {
                open.buffer.undo();
                edited = true;
            }
            KeyCode::Char('y') if _mods.contains(KeyModifiers::CONTROL) => {
                open.buffer.redo();
                edited = true;
            }
            KeyCode::Char(c) => {
                // 中文输入辅助（§6.3）：「→「」、（→（）、《→《》，可关。
                if self.config.editor.auto_pair {
                    match mj_text::pair::on_input(c, open.buffer.next_char()) {
                        mj_text::pair::PairAction::InsertPair(o, cl) => {
                            open.buffer.insert_pair(o, cl)
                        }
                        // 光标右边就是要敲的右符号 → 越过去，不插重复的。
                        mj_text::pair::PairAction::Skip(_) => open.buffer.move_right(),
                        mj_text::pair::PairAction::Insert(c) => open.buffer.insert(&c.to_string()),
                    }
                } else {
                    open.buffer.insert(&c.to_string());
                }
                edited = true;
                // 敲了 `@` 且有角色可选 → 开补全（§6.7）。
                if c == '@' && !at_names.is_empty() {
                    let at = open.buffer.cursor().saturating_sub('@'.len_utf8());
                    ws.completion = Some(Completion::new(at, at_names.clone()));
                }
            }
            KeyCode::Enter => {
                open.buffer.insert("\n");
                edited = true;
            }
            KeyCode::Backspace => {
                open.buffer.delete_backward();
                edited = true;
            }
            KeyCode::Delete => {
                open.buffer.delete_forward();
                edited = true;
            }
            // Shift+方向键：延续选区（§6.4 [MUST] 选中时状态栏切为「选中 N 字」）。
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                if _mods.contains(KeyModifiers::SHIFT) =>
            {
                open.buffer.start_selection();
                match code {
                    KeyCode::Left => open.buffer.move_left(),
                    KeyCode::Right => open.buffer.move_right(),
                    KeyCode::Home => open.buffer.move_home(),
                    _ => open.buffer.move_end(),
                }
            }
            KeyCode::Left => {
                open.buffer.clear_selection();
                open.buffer.move_left();
            }
            KeyCode::Right => {
                open.buffer.clear_selection();
                open.buffer.move_right();
            }
            KeyCode::Home => {
                open.buffer.clear_selection();
                open.buffer.move_home();
            }
            KeyCode::End => {
                open.buffer.clear_selection();
                open.buffer.move_end();
            }
            KeyCode::Up => open.viewport.scroll_up(1),
            KeyCode::Down => open.viewport.scroll_down(1, &open.buffer),
            _ => {}
        }
        if edited {
            open.autosave.on_edit(std::time::Instant::now());
            // 字数只在编辑后重算，不在每帧渲染时算——后者会让 10 万字章节
            // 的每次按键都做一遍全量统计，直接吃掉 §9 的 16ms 预算。
            open.word_count = mj_text::count::count(&open.buffer.contents());
            // 正文一改，校对命中的坐标就失效——清掉下划线，别让它标在错的字上。
            ws.proof_issues.clear();
        }
        open.viewport.scroll_to_cursor(&open.buffer);
        Ok(())
    }

    fn open_chapter(&mut self, id: ChapterId) -> anyhow::Result<()> {
        // 切章前先保存当前章。
        self.save_current()?;

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let body = self.store.load_body(ws.book.id, id)?;
        let saved = body.text.to_string();
        // 磁盘上的字数——今日码字量的基线，必须在 saved 被恢复逻辑消费前取。
        let saved_words = mj_text::count::count_with_punct(&saved);
        let path = self.store.chapter_file_path(ws.book.id, id)?;

        // 崩溃恢复（§6.3）：swp 里若有比磁盘更新的内容，先救回来。
        //
        // M1 采取「自动恢复 + 明确告知」而非弹窗询问：浮层栈是 M6 的内容，
        // 而在此之前若默默丢掉 swp，就等于让崩溃前的字白写——那是 §0 禁令 1
        // 要防的事。恢复后正文进入 dirty 状态，用户可 Ctrl+Z 退回磁盘版本。
        let (text, recovered) = match mj_core::swap::detect(&path, &saved)? {
            Some(r) if r.differs() => (r.swap_body, true),
            Some(_) => {
                // 与磁盘一致 = 上次正常退出的残留，静默清理。
                let _ = mj_core::swap::remove(&path);
                (saved, false)
            }
            None => (saved, false),
        };

        let mut buffer = Buffer::new(&text, self.config.editor.undo_depth);
        if recovered {
            // 标脏：恢复出来的内容尚未落盘，状态栏要显示「未保存」。
            buffer.mark_dirty_for_recovery();
            self.toast = Some(format!(
                "已从崩溃恢复文件找回未保存的内容（{} 字），Ctrl+S 保存或 Ctrl+Z 撤销",
                mj_text::count::count_with_punct(&buffer.contents())
            ));
        }

        let word_count = mj_text::count::count(&buffer.contents());
        ws.editor = Some(OpenChapter {
            id,
            buffer,
            viewport: Viewport::new(40, 20), // 真实尺寸在渲染时校正
            autosave: AutoSave::new(
                self.config.editor.autosave_idle_ms,
                self.config.editor.autosave_words,
            ),
            path,
            word_count,
            // 以磁盘上的字数为基线：从 swp 恢复出来的增量属于「今天写的」，
            // 保存时才计入今日码字量，而不是打开章节就凭空跳一截。
            saved_words,
        });
        ws.tree.focus_chapter(&ws.book, id);
        // 换章：上一章的校对命中作废，清掉下划线；补全也作废。
        ws.proof_issues.clear();
        ws.completion = None;
        // 换了一章就是另一条历史线，自动快照的基准要跟着重置。
        self.last_snapshot = None;
        Ok(())
    }

    /// Tick 驱动的自动保存（§6.3、§7.4）。
    fn on_tick(&mut self) -> anyhow::Result<()> {
        let now = std::time::Instant::now();

        // 自动快照（§6.9）：与自动保存共用心跳。
        self.maybe_auto_snapshot();

        let action = {
            let Screen::Workspace(ws) = &mut self.screen else {
                return Ok(());
            };
            let Some(open) = &mut ws.editor else {
                return Ok(());
            };
            open.autosave
                .poll(now, open.buffer.is_dirty(), open.buffer.changed_chars())
        };

        match action {
            Action::Idle => {}
            Action::WriteSwap => {
                let Screen::Workspace(ws) = &self.screen else {
                    return Ok(());
                };
                let Some(open) = &ws.editor else {
                    return Ok(());
                };
                // swp 写失败不该打断写作——它是保险丝，不是主路径。记日志即可。
                if let Err(e) = mj_core::swap::write(&open.path, &open.buffer.contents()) {
                    tracing::warn!(error = %e, "写 swp 失败");
                }
            }
            Action::Save => {
                self.save_current()?;
                if let Screen::Workspace(ws) = &mut self.screen
                    && let Some(open) = &mut ws.editor
                {
                    open.autosave.on_saved();
                }
                self.dirty = true; // 状态栏要从「未保存」变回「已保存」
            }
        }
        Ok(())
    }

    fn save_current(&mut self) -> anyhow::Result<()> {
        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(open) = &mut ws.editor else {
            return Ok(());
        };
        if !open.buffer.is_dirty() {
            return Ok(());
        }

        let text = open.buffer.contents();
        let body = ChapterBody::new(open.id, &text);
        let book_id = ws.book.id;

        // save_body 内部会清掉 swp——正文既已落盘，swp 留着只会下次误报。
        self.store.save_body(book_id, &body)?;
        open.buffer.mark_saved();
        // 手动保存同样要重置自动保存的计时，否则它还以为欠着一次保存。
        open.autosave.on_saved();

        // 今日码字量：以本次保存的净增累加（§6.4，删改为负）。
        let wc = mj_text::count::count(&text);
        let delta = wc.with_punct as i64 - open.saved_words as i64;
        open.word_count = wc;
        open.saved_words = wc.with_punct;

        let entry = mj_core::index::ChapterEntry {
            chapter: open.id,
            book: book_id,
            volume: String::new(),
            title: String::new(),
            order: 0,
            path: open.path.clone(),
            content_hash: mj_core::index::content_hash(&text),
            words_with_punct: wc.with_punct as u64,
            words_no_punct: wc.no_punct as u64,
            han_chars: wc.han as u64,
            updated: chrono::Local::now().timestamp(),
        };

        // 索引写失败不该打断保存——正文已经安全了，索引下次重建即可。
        if let Some(idx) = &self.index {
            let day = mj_core::index::writing_day(
                chrono::Local::now(),
                self.config.general.day_starts_at,
            );
            if delta != 0
                && let Err(e) = idx.add_daily_delta(book_id, &day, delta)
            {
                tracing::warn!(error = %e, "记录今日码字量失败");
            }
            if let Err(e) = idx.upsert_chapter(&entry) {
                tracing::warn!(error = %e, "更新索引失败");
            }
        }
        self.refresh_today_words(book_id);

        // 保存后重载元数据，让树上的字数跟上。
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.book = self.store.load_book(book_id)?;
        }
        self.toast = Some("已保存".into());
        Ok(())
    }

    // ---- 渲染 ----

    #[doc(hidden)]
    pub fn render_for_test(&mut self, frame: &mut ratatui::Frame) {
        self.render(frame);
    }

    /// 供测试：直接送一个按键进真正的分发路径。
    ///
    /// 比起再加一个 `press_xxx_for_demo`，这个能测到**键位本身**——
    /// 一个只在 demo 钩子里跑过的功能，等于没验证过用户按得到它。
    /// （Ctrl+Shift+S 那次就是这么漏的，见 doc.md §7.3。）
    #[doc(hidden)]
    pub fn press_for_test(&mut self, code: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        self.on_key(code, mods)
    }

    /// 供测试：送一个鼠标事件。
    ///
    /// 命中区域是渲染时记下的，所以调用前必须先 `render_for_test` 画一帧——
    /// 真实主循环也正是这个次序（先画，用户看着画面点，事件才来）。
    #[doc(hidden)]
    pub fn mouse_for_test(
        &mut self,
        kind: MouseEventKind,
        col: u16,
        row: u16,
    ) -> anyhow::Result<()> {
        self.toast = None;
        self.on_mouse(MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// 供测试：把焦点切到目录树（`open_book` 默认打开首章、焦点落在编辑器）。
    #[doc(hidden)]
    pub fn focus_tree_for_test(&mut self) {
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.focus = Focus::Tree;
        }
    }

    /// 供测试：把树光标定到某一章（挪动测试要选中不同的章）。
    #[doc(hidden)]
    pub fn select_chapter_for_test(&mut self, ch: ChapterId) {
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.tree.focus_chapter(&ws.book, ch);
        }
    }

    /// 供测试：输入框当前文本（None = 没开输入框）。
    #[doc(hidden)]
    pub fn input_value_for_test(&self) -> Option<String> {
        self.input.as_ref().map(|i| i.value().to_string())
    }

    /// 供测试：当前书里所有卷/章的标题，按树的顺序（卷，然后它的章）。
    #[doc(hidden)]
    pub fn tree_titles_for_test(&self) -> Vec<String> {
        match &self.screen {
            Screen::Workspace(ws) => {
                let mut out = Vec::new();
                for v in &ws.book.volumes {
                    out.push(v.title.clone());
                    out.extend(v.chapters.iter().map(|c| c.title.clone()));
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// 供测试：当前侧栏宽度（§13 拖分隔条）。
    #[doc(hidden)]
    pub fn tree_width_for_test(&self) -> u16 {
        match &self.screen {
            Screen::Workspace(ws) => ws.tree_width,
            _ => 0,
        }
    }

    /// 供测试：正文光标的字节位置与视口顶行。
    #[doc(hidden)]
    pub fn editor_pos_for_test(&self) -> Option<(usize, usize)> {
        match &self.screen {
            Screen::Workspace(ws) => ws
                .editor
                .as_ref()
                .map(|o| (o.buffer.cursor(), o.viewport.top_logical())),
            _ => None,
        }
    }

    /// 供测试：跑到批量作业结束（真实主循环是分片跑的，这里一次跑完）。
    #[doc(hidden)]
    pub fn drain_batch_for_test(&mut self) -> anyhow::Result<()> {
        for _ in 0..10_000 {
            if !matches!(&self.screen, Screen::Workspace(ws) if ws.batch.is_some()) {
                return Ok(());
            }
            self.step_batch()?;
        }
        anyhow::bail!("批量作业没有收敛")
    }

    /// 供测试：当前的 toast 文本。
    #[doc(hidden)]
    pub fn toast_for_test(&self) -> Option<&str> {
        self.toast.as_deref()
    }

    #[doc(hidden)]
    pub fn confirm_open_for_test(&self) -> bool {
        matches!(&self.screen, Screen::Workspace(ws) if ws.modals.contains(ModalKind::Confirm))
    }

    /// 供测试：校对面板可见问题数（None = 面板没开）。
    #[doc(hidden)]
    pub fn proof_visible_for_test(&self) -> Option<usize> {
        match &self.screen {
            Screen::Workspace(ws) => ws.modals.proof().map(|p| p.visible_count()),
            _ => None,
        }
    }

    /// 供测试：当前编辑缓冲的正文。
    #[doc(hidden)]
    pub fn buffer_text_for_test(&self) -> Option<String> {
        match &self.screen {
            Screen::Workspace(ws) => ws.editor.as_ref().map(|o| o.buffer.contents()),
            _ => None,
        }
    }

    /// 供测试：是否已请求退出。
    #[doc(hidden)]
    pub fn should_quit_for_test(&self) -> bool {
        self.should_quit
    }

    /// 供测试：目录树是否显示。
    #[doc(hidden)]
    pub fn show_tree_for_test(&self) -> bool {
        match &self.screen {
            Screen::Workspace(ws) => ws.show_tree,
            _ => false,
        }
    }

    /// 供测试：浮层栈自底向上的种类名（§7.1）。
    #[doc(hidden)]
    pub fn modal_stack_for_test(&self) -> Vec<String> {
        match &self.screen {
            Screen::Workspace(ws) => ws
                .modals
                .kinds()
                .into_iter()
                .map(|k| format!("{k:?}"))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// 供测试：跑一条命令。没有专属键位的命令（如模型校对）只能这样触发。
    #[doc(hidden)]
    pub fn run_command_for_test(&mut self, cmd: Command) -> anyhow::Result<()> {
        self.toast = None;
        self.run_command(cmd)
    }

    /// 供测试：@ 补全是否激活。
    #[doc(hidden)]
    pub fn completion_active_for_test(&self) -> bool {
        matches!(&self.screen, Screen::Workspace(ws) if ws.completion.is_some())
    }

    /// 供测试：角色面板里筛选后的数量（None = 面板没开）。
    #[doc(hidden)]
    pub fn character_filtered_for_test(&self) -> Option<usize> {
        match &self.screen {
            Screen::Workspace(ws) => ws.modals.characters().map(|p| p.filtered_count()),
            _ => None,
        }
    }

    /// 供测试：角色面板是否在出场统计视图（None = 面板没开）。
    #[doc(hidden)]
    pub fn character_stats_open_for_test(&self) -> Option<bool> {
        match &self.screen {
            Screen::Workspace(ws) => ws.modals.characters().map(|p| p.show_stats()),
            _ => None,
        }
    }

    /// 供测试：角色面板当前选中角色的名字。
    #[doc(hidden)]
    pub fn character_current_name_for_test(&self) -> Option<String> {
        match &self.screen {
            Screen::Workspace(ws) => ws
                .modals
                .characters()
                .and_then(|p| p.current())
                .map(|c| c.name.clone()),
            _ => None,
        }
    }

    /// 供测试：把编辑焦点切到指定章（决定「当前章/当前卷」范围）。
    #[doc(hidden)]
    pub fn open_chapter_for_test(&mut self, ch: ChapterId) -> anyhow::Result<()> {
        self.open_chapter(ch)
    }

    /// 供测试：当前打开的章。
    #[doc(hidden)]
    pub fn current_chapter_for_test(&self) -> Option<ChapterId> {
        match &self.screen {
            Screen::Workspace(ws) => ws.editor.as_ref().map(|o| o.id),
            _ => None,
        }
    }

    /// 供测试：查找面板当前的作业范围。
    #[doc(hidden)]
    pub fn search_scope_for_test(&self) -> Option<Scope> {
        match &self.screen {
            Screen::Workspace(ws) => ws.modals.search().map(|p| p.scope),
            _ => None,
        }
    }

    /// 供测试：设置替换栏的内容。
    #[doc(hidden)]
    pub fn set_replace_text_for_test(&mut self, to: &str) {
        if let Screen::Workspace(ws) = &mut self.screen
            && let Some(p) = ws.modals.search_mut()
        {
            p.replace_with = to.to_string();
        }
    }

    /// 供测试：某章全部快照的正文。
    #[doc(hidden)]
    pub fn snapshot_texts_for_test(&mut self, ch: ChapterId) -> Vec<String> {
        let Screen::Workspace(ws) = &self.screen else {
            return Vec::new();
        };
        let book = ws.book.id;
        let h = self.history_of(book);
        h.list(ch)
            .iter()
            .filter_map(|s| h.read(&s.id).ok())
            .collect()
    }

    /// 供测试与截图：打开查找面板并输入查找串。
    #[doc(hidden)]
    pub fn open_search_for_demo(&mut self, replace: bool, query: &str) {
        let _ = self.open_search(replace);
        if let Screen::Workspace(ws) = &mut self.screen {
            let text = ws
                .editor
                .as_ref()
                .map(|o| o.buffer.contents())
                .unwrap_or_default();
            if let Some(p) = ws.modals.search_mut() {
                p.query = query.to_string();
                p.replace_with = "霜".to_string();
                p.refresh(&text);
            }
        }
    }

    /// 供测试：手动打快照。
    #[doc(hidden)]
    pub fn manual_snapshot_for_demo(&mut self) -> anyhow::Result<()> {
        self.manual_snapshot()
    }

    /// 供测试与截图：按 F8 打开历史面板。
    #[doc(hidden)]
    pub fn press_f8_for_demo(&mut self) -> anyhow::Result<()> {
        self.open_history()
    }

    /// 供测试与截图：在历史面板里按 Enter。
    #[doc(hidden)]
    pub fn press_enter_in_history_for_demo(&mut self) -> anyhow::Result<()> {
        self.open_diff()
    }

    /// 供测试与截图：按 F5 打开排版预览。
    #[doc(hidden)]
    pub fn press_f5_for_demo(&mut self) -> anyhow::Result<()> {
        self.open_format_preview()
    }

    /// 供测试与截图：打开统计面板。
    #[doc(hidden)]
    pub fn open_stats_for_demo(&mut self) {
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.modals.push(Modal::Stats(Box::new(Stats::new())));
        }
    }

    /// 供测试与截图：打开书架上的第一本书。
    #[doc(hidden)]
    pub fn open_first_book_for_demo(&mut self) -> anyhow::Result<()> {
        if let Screen::Shelf(s) = &self.screen
            && let Some(id) = s.selected_id()
        {
            self.open_book(id)?;
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let [body, status] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

        // 统计面板打开时占满正文区（浮层语义）。
        let stats_rows = match &self.screen {
            Screen::Workspace(ws) if ws.modals.contains(ModalKind::Stats) => {
                Some(Stats::rows(&ws.book, self.no_punct_lookup()))
            }
            _ => None,
        };

        // 配色与版面参数：与 screen 是不同字段，可与下面的可变借用并存。
        let theme = &self.theme;
        let layout = EditorLayout::from_config(&self.config, self.focus_mode);
        let keymap = &self.keymap;

        // 先铺一层主题底色——sepia 这类预设的观感八成来自它（§2.1「二级降级」）。
        // 兜底主题的 bg 是 Reset，铺上去等同不铺，对 8 色终端无害。
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.bg).fg(theme.fg)),
            area,
        );

        match &mut self.screen {
            Screen::Shelf(shelf) => render_shelf(frame, body, shelf, theme),
            Screen::Workspace(ws) => {
                // 基底：目录树 + 正文。终端光标只在没有浮层/作业时显示——
                // 否则光标会落在被盖住的正文里，看着像跑到浮层外面去了。
                let show_cursor = ws.modals.is_empty() && ws.batch.is_none();
                render_workspace(frame, body, ws, theme, show_cursor, &layout);

                if let Some(job) = &ws.batch {
                    frame.render_widget(ratatui::widgets::Clear, body);
                    render_batch(frame, body, job, theme);
                }

                // 浮层自底向上叠画：后压进来的画在上面（§7.1）。
                let book_title = ws.book.title.clone();
                for m in ws.modals.iter_mut() {
                    // 铺满型浮层先清底，免得正文从空白处透上来。
                    if m.is_fullscreen() {
                        frame.render_widget(ratatui::widgets::Clear, body);
                    }
                    match m {
                        Modal::Diff(v) => render_diff(frame, body, v, theme),
                        Modal::History(p) => render_history(frame, body, p, theme),
                        Modal::Search(p) => render_search(frame, body, p, theme),
                        Modal::FormatPreview(p) => render_format_preview(frame, body, p, theme),
                        Modal::Proof(p) => render_proof(frame, body, p, theme),
                        Modal::CharacterForm(fm) => render_character_form(frame, body, fm, theme),
                        Modal::Characters(p) => render_characters(frame, body, p, theme),
                        Modal::Confirm(c) => render_confirm(frame, body, c, theme),
                        Modal::Consent(c) => render_consent(frame, body, c, theme),
                        Modal::Palette(p) => render_palette(frame, body, p, theme),
                        Modal::Help(h) => render_help(frame, body, h, theme, keymap),
                        Modal::Settings(s) => render_settings(frame, body, s, theme),
                        Modal::Stats(st) => {
                            if let Some(rows) = &stats_rows {
                                render_stats(frame, body, st, rows, &book_title, theme);
                            }
                        }
                    }
                }
            }
        }

        // 输入框画在最上层，盖住书架/工作区都行（改书名在书架、改章名在工作区）。
        if let Some(input) = &self.input {
            render_input(frame, body, input, &self.theme);
        }

        self.render_status(frame, status);
    }

    fn render_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let theme = &self.theme;
        let mut spans: Vec<Span> = Vec::new();

        if let Some(t) = &self.toast {
            spans.push(Span::raw(format!(" {t} ")));
        } else {
            match &self.screen {
                Screen::Shelf(s) => {
                    spans.push(Span::raw(format!(" {} 本书 ", s.books().len())));
                    spans.push(Span::raw("│ Enter 打开 │ n 新建 │ q 退出 "));
                }
                Screen::Workspace(ws) if ws.modals.top_is(ModalKind::Palette) => {
                    spans.push(Span::raw(
                        " 命令面板 │ 输入筛选 │ ↑↓ 选择 │ Enter 执行 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.modals.top_is(ModalKind::Help) => {
                    spans.push(Span::raw(" 帮助 │ j/k 滚动 │ Esc 关闭 "));
                }
                Screen::Workspace(ws) if ws.modals.top_is(ModalKind::Settings) => {
                    spans.push(Span::raw(
                        " 外观 │ j/k 移动 │ ←/→ 换主题 │ y 复制配置片段 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::CharacterForm) => {
                    let editing = ws.modals.character_form().is_some_and(|f| f.is_editing());
                    if editing {
                        spans.push(Span::raw(
                            " 编辑字段 │ 输入内容 │ Enter 换行/完成 │ Esc 结束本字段 ",
                        ));
                    } else {
                        spans.push(Span::raw(
                            " 角色卡 │ Tab/j/k 换字段 │ Enter/i 编辑 │ Ctrl+S 保存 │ Esc 返回 ",
                        ));
                    }
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Characters) => {
                    let panel = ws.modals.characters();
                    if panel.is_some_and(|p| p.is_searching()) {
                        spans.push(Span::raw(" 搜索角色 │ 输入筛选 │ Enter/Esc 结束搜索 "));
                    } else if panel.is_some_and(|p| p.show_stats()) {
                        spans.push(Span::raw(" 出场统计 │ j/k 滚动 │ t/Esc 返回列表 "));
                    } else {
                        spans.push(Span::raw(
                            " 角色 │ j/k 移动 │ / 搜索 │ n 新建 │ e 编辑 │ d 删除 │ t 出场统计 │ Esc 关闭 ",
                        ));
                    }
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Proof) => {
                    spans.push(Span::raw(
                        " 校对 │ j/k 移动 │ Enter 跳转 │ a 应用建议 │ i 忽略 │ I 永久忽略 ",
                    ));
                    if let Some((n, shown)) = ws.modals.proof().and_then(|p| p.fold_hint()) {
                        let label = if shown {
                            format!("│ f 收起 {n} 条低置信 ")
                        } else {
                            format!("│ f 展开 {n} 条低置信 ")
                        };
                        spans.push(Span::styled(label, Style::default().fg(theme.dim)));
                    }
                    spans.push(Span::raw("│ Esc 关闭 "));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Confirm) => {
                    spans.push(Span::raw(" ←/→ 选择 │ Enter 确定 │ y 执行 │ Esc 取消 "));
                }
                Screen::Workspace(ws) if ws.batch.is_some() => {
                    spans.push(Span::raw(" 批量作业进行中 │ Esc 中断 "));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Diff) => {
                    // §12.4 的底栏。
                    spans.push(Span::raw(
                        " n/p 跳转改动 │ u 恢复此块 │ U 恢复整章 │ y 复制旧内容 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::History) => {
                    spans.push(Span::raw(
                        " 历史 │ Enter 看 diff │ Space 选对照条 │ P 钉住 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Search) => {
                    let wide = ws.modals.search().is_some_and(|s| s.scope.is_wide());
                    let hint = if !ws.modals.search().is_some_and(|s| s.replace_mode) {
                        " 查找 │ Tab 切到结果 │ Enter 跳转 │ Esc 关闭 "
                    } else if wide {
                        // 宽范围下勾选管不到别的章，Alt+R 也只作用于当前章——
                        // 底栏就别再提它们，免得暗示它们跟范围有关。
                        " 查找替换 │ Tab 切换栏 │ F4 范围 │ Alt+A 替换该范围全部 │ Esc 关闭 "
                    } else {
                        " 查找替换 │ Tab 切换栏 │ F4 范围 │ Space 勾选 │ Alt+R 替换本条 │ Alt+A 替换勾选 │ Esc 关闭 "
                    };
                    spans.push(Span::raw(hint));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::FormatPreview) => {
                    spans.push(Span::raw(
                        " 排版预览 │ Space 逐条取消 │ a 全选 │ n 全不选 │ Enter 应用 │ Esc 放弃 ",
                    ));
                }
                Screen::Workspace(ws) if ws.modals.contains(ModalKind::Stats) => {
                    spans.push(Span::raw(" 统计面板 │ e 导出 CSV │ ↑↓ 滚动 │ Esc 关闭 "));
                }
                Screen::Workspace(ws) => {
                    // §6.4 [MUST]：选中文本时状态栏切为「选中 N 字」。
                    // 选区是临时状态，此刻用户关心的是「我选了多少」，
                    // 而不是本章/本卷/全书那一串常驻数字。
                    if let Some(sel) = ws.editor.as_ref().and_then(|o| o.buffer.selected_text()) {
                        let n = mj_text::count::count_with_punct(&sel);
                        spans.push(Span::styled(
                            format!(" 选中 {} 字 ", format_words(n as u64)),
                            Style::default().fg(theme.accent),
                        ));
                        spans.push(Span::raw("│ Esc 取消选择 "));
                        frame.render_widget(
                            Paragraph::new(Line::from(spans)).style(Style::default().reversed()),
                            area,
                        );
                        return;
                    }

                    // §6.4 状态栏：本章 3,128 / 净 2,904 | 本卷 4.2万 | 全书 21.7万 | 今日 +1,240
                    if let Some(open) = &ws.editor {
                        let wc = open.word_count;
                        spans.push(Span::raw(format!(
                            " 本章 {} / 净 {} ",
                            format_words(wc.with_punct as u64),
                            format_words(wc.no_punct as u64)
                        )));
                        spans.push(Span::raw("│ "));
                    }

                    // 本卷：光标所在卷的字数。
                    if let Some(vol_words) = ws.current_volume_words() {
                        spans.push(Span::raw(format!("本卷 {} │ ", format_words(vol_words))));
                    }

                    let total: u64 = ws
                        .book
                        .volumes
                        .iter()
                        .flat_map(|v| &v.chapters)
                        .filter_map(|c| c.word_count)
                        .sum();
                    spans.push(Span::raw(format!("全书 {} │ ", format_words(total))));

                    // 今日码字量（§6.4）：正负都要显示——删得比写得多是常态，
                    // 显示成 0 会让人以为统计坏了。
                    let sign = if self.today_words >= 0 { "+" } else { "" };
                    spans.push(Span::styled(
                        format!("今日 {sign}{} ", self.today_words),
                        Style::default().fg(if self.today_words >= 0 {
                            theme.insert
                        } else {
                            theme.dim
                        }),
                    ));

                    if let Some(open) = &ws.editor {
                        spans.push(Span::raw("│ "));
                        if open.buffer.is_dirty() {
                            spans.push(Span::styled("●未保存", Style::default().fg(theme.warning)));
                        } else {
                            spans.push(Span::raw("已保存"));
                        }
                        spans.push(Span::raw(" "));
                    }
                    spans.push(Span::raw(
                        "│ Ctrl+S 保存 │ F5 排版 │ F8 历史 │ F9 快照 │ Esc 返回 ",
                    ));

                    // 刚做完批量作业：把撤销的入口摆在眼前（§6.6 [MUST]）。
                    // 埋在帮助里的撤销，等于没有。
                    if let Some(u) = &self.batch_undo {
                        spans.push(Span::styled(
                            format!("│ Alt+U {} ", u.describe()),
                            Style::default().fg(theme.warning),
                        ));
                    }

                    // §7.2：窄屏隐藏侧栏时要在状态栏提示。
                    if frame.area().width < NARROW_THRESHOLD {
                        spans.push(Span::styled(
                            "│ 窄屏：侧栏已隐藏",
                            Style::default().fg(theme.dim),
                        ));
                    }
                }
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().reversed()),
            area,
        );
    }
}

fn render_shelf(frame: &mut ratatui::Frame, area: Rect, shelf: &Shelf, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 墨简 · 书架 ")
        .border_style(Style::default().fg(theme.border));

    if shelf.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("书架是空的").centered(),
                Line::from(""),
                Line::from("按 n 新建一本书").centered(),
            ])
            .block(block),
            area,
        );
        return;
    }

    let mut lines = Vec::new();
    for (i, b) in shelf.books().iter().enumerate() {
        let s = Shelf::summary(b);
        let marker = if i == shelf.cursor() { "▶ " } else { "  " };
        let mut line = format!(
            "{marker}《{}》  {}  {} 卷 {} 章  {} 字",
            b.title,
            b.author,
            s.volumes,
            s.chapters,
            format_words(s.words)
        );
        if let Some(p) = s.progress {
            line.push_str(&format!("  {:.0}%", p * 100.0));
        }
        let style = if i == shelf.cursor() {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        lines.push(Line::styled(line, style));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_workspace(
    frame: &mut ratatui::Frame,
    area: Rect,
    ws: &mut Workspace,
    theme: &Theme,
    show_cursor: bool,
    layout: &EditorLayout,
) {
    // §7.2：窄屏（< 80 列）自动隐藏侧栏，只留正文。
    let narrow = area.width < NARROW_THRESHOLD;
    let show_tree = ws.show_tree && !narrow;

    let (tree_area, editor_area) = if show_tree {
        let w = ws
            .tree_width
            .clamp(TREE_MIN_WIDTH, tree_max_width(area.width));
        let [t, e] = Layout::horizontal([Constraint::Length(w), Constraint::Min(0)]).areas(area);
        (Some(t), e)
    } else {
        (None, area)
    };

    if let Some(ta) = tree_area {
        render_tree(frame, ta, ws, theme);
    }
    let body = render_editor(frame, editor_area, ws, theme, show_cursor, layout);

    // 记下这一帧的版面，下一个鼠标事件照它判命中（见 `Hit`）。
    ws.hit = Hit {
        tree: tree_area.map(|ta| (ta, tree_scroll_top(ta, ws))),
        editor: editor_area,
        body,
    };
}

/// 目录树上一帧滚到了第几行。
///
/// 与 `render_tree` 里那段是同一个算法——两处都要用，必须共用一份，
/// 否则改了渲染忘了改这里，鼠标就会点到隔壁那一章去。
fn tree_scroll_top(area: Rect, ws: &Workspace) -> usize {
    let inner_h = area.height.saturating_sub(2) as usize;
    let rows = ws.tree.rows(&ws.book).len();
    ws.tree
        .cursor()
        .saturating_sub(inner_h / 2)
        .min(rows.saturating_sub(inner_h))
}

/// 历史面板（§6.9）。
fn render_history(frame: &mut ratatui::Frame, area: Rect, p: &mut HistoryPanel, theme: &Theme) {
    let title = match p.compare_target() {
        Some(_) => " 历史 · 已选对照条，Enter 两条互比 ".to_string(),
        None => format!(" 历史 · {} 条快照 ", p.snapshots().len()),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    p.set_height(inner.height as usize);

    let lines: Vec<Line> = (p.scroll()..p.snapshots().len())
        .take(inner.height as usize)
        .map(|i| {
            let mut style = Style::default();
            if p.snapshots()[i].is_protected() {
                // 受保护的醒目一点——它们是用户特意留下的锚点。
                style = style.fg(theme.warning);
            }
            if i == p.cursor() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::styled(p.render_row(i), style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// diff 界面（§6.9、§12.4 的线框）。
fn render_diff(frame: &mut ratatui::Frame, area: Rect, v: &mut DiffView, theme: &Theme) {
    // §12.4：标题栏是「与「…」比较 ─── +312 / -87 / 3 处改动」。
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " 与「{}」比较 ─── {} ",
            v.old_title,
            v.summary_line()
        ))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    v.set_height(inner.height as usize);

    // §6.9：宽度 ≥ 100 列时左右分栏，否则 inline。
    //
    // M4 先做 inline —— 它在两种宽度下都读得懂，而分栏在窄屏上会把
    // 中文正文挤成每行三四个字。分栏留到 M6 与外观预设一起做。
    let _side_by_side = DiffView::use_side_by_side(inner.width);

    let lines: Vec<Line> = v
        .inline_lines()
        .into_iter()
        .skip(v.scroll())
        .take(inner.height as usize)
        .map(|dl| {
            let current = dl.hunk == Some(v.hunk_cursor());
            let (marker, style) = match dl.kind {
                // §6.9：增行绿底、删行红底。
                LineKind::Insert => ("+", Style::default().fg(theme.insert)),
                LineKind::Delete => ("-", Style::default().fg(theme.error)),
                LineKind::Equal => (" ", Style::default().fg(theme.dim)),
            };
            // 当前块加粗——n/p 跳过来之后要看得出跳到哪了。
            let style = if current {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };
            Line::styled(format!("{:>4} │ {marker} {}", dl.line_no, dl.text), style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// 快照的显示名，如「07-14 22:10 · 投稿版」（§12.4 的线框）。
fn snapshot_title(s: &mj_core::history::Snapshot) -> String {
    let when = s.created.format("%m-%d %H:%M");
    match &s.label {
        Some(l) => format!("{when} · {l}"),
        None => format!("{when} · {}", s.trigger.label()),
    }
}

/// 查找替换面板（§6.6）。
fn render_search(frame: &mut ratatui::Frame, area: Rect, p: &mut SearchPanel, theme: &Theme) {
    let title = if p.replace_mode {
        " 查找替换 · 当前章 "
    } else {
        " 查找 · 当前章 "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 输入栏（1~2 行） + 选项行 + 摘要行 + 结果列表
    let input_rows = if p.replace_mode { 2 } else { 1 };
    let [inputs, options, summary, results] = Layout::vertical([
        Constraint::Length(input_rows),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    // 输入栏：当前焦点用 ▸ 标出——没有光标的输入框，用户不知道在敲哪。
    let mut input_lines = vec![Line::from(field_line(
        "查找",
        &p.query,
        p.field() == search_panel::Field::Query,
        theme,
    ))];
    if p.replace_mode {
        input_lines.push(Line::from(field_line(
            "替换",
            &p.replace_with,
            p.field() == search_panel::Field::Replace,
            theme,
        )));
    }
    frame.render_widget(Paragraph::new(input_lines), inputs);

    frame.render_widget(
        Paragraph::new(p.options_line()).style(Style::default().fg(theme.dim)),
        options,
    );

    // 摘要：命中数，或非法正则的实时提示（§6.6 [MUST]）。
    let summary_style = if p.error().is_some() {
        Style::default().fg(theme.error)
    } else {
        Style::default().fg(theme.insert)
    };
    frame.render_widget(Paragraph::new(p.summary()).style(summary_style), summary);

    p.set_height(results.height as usize);

    let focused = p.field() == search_panel::Field::Results;
    let lines: Vec<Line> = p
        .hits()
        .iter()
        .enumerate()
        .skip(p.scroll())
        .take(results.height as usize)
        .map(|(i, h)| {
            let check = if p.is_checked(i) { "✓" } else { " " };
            // 命中处高亮：把上下文按 highlight 切三段。
            let before = h.context.get(..h.highlight.start).unwrap_or("");
            let hit = h.context.get(h.highlight.clone()).unwrap_or("");
            let after = h.context.get(h.highlight.end..).unwrap_or("");

            let base = if i == p.cursor() && focused {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(format!("{check} 第{}行 ", h.line), base),
                Span::styled(before.to_string(), base),
                Span::styled(
                    hit.to_string(),
                    base.fg(theme.warning).add_modifier(Modifier::BOLD),
                ),
                Span::styled(after.to_string(), base),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), results);
}

/// 一行输入框。有焦点的那行用 ▸ 与下划线标出。
fn field_line(label: &str, value: &str, focused: bool, theme: &Theme) -> Vec<Span<'static>> {
    let marker = if focused { "▸" } else { " " };
    let style = if focused {
        Style::default().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(theme.dim)
    };
    vec![
        Span::raw(format!("{marker} {label}: ")),
        Span::styled(value.to_string(), style),
        // 光标提示：终端光标被正文占着，这里用一个块字符代替。
        Span::styled(if focused { "▏" } else { "" }, style),
    ]
}

/// 排版预览（§6.5 [MUST]：显示将改动的位置与条数，可逐条取消）。
/// 校对面板（F7，§6.8）。按严重度分组，逐条列出，命中原文加下划线着色。
fn render_proof(frame: &mut ratatui::Frame, area: Rect, p: &mut ProofPanel, theme: &Theme) {
    use proof_panel::Row;

    let title = if p.is_empty() {
        " 校对 · 未发现问题 ".to_string()
    } else {
        format!(" 校对 · {} 处 ", p.visible_count())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if p.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("这一章没有发现问题。").centered(),
            ]),
            inner,
        );
        return;
    }

    p.set_height(inner.height as usize);

    // 每条问题占一行；分组表头也占一行，但不参与滚动窗口（近似：表头很少，
    // 直接连问题一起画，靠 scroll 起点对齐问题序号）。
    let rows = p.rows();
    let sev_color = |s| proof_severity_color(s, theme);

    let mut lines: Vec<Line> = Vec::new();
    for row in &rows {
        match row {
            Row::Header(sev, count) => {
                lines.push(Line::from(Span::styled(
                    format!("── {} ({count}) ──", sev.label()),
                    Style::default()
                        .fg(sev_color(*sev))
                        .add_modifier(Modifier::BOLD),
                )));
            }
            Row::Issue {
                index,
                issue,
                selected,
            } => {
                let mut base = Style::default().fg(sev_color(issue.severity));
                if *selected {
                    base = base.add_modifier(Modifier::REVERSED);
                }
                let cat = issue.category.label();
                let sug = issue
                    .suggestions
                    .first()
                    .map(|s| format!(" → {s}"))
                    .unwrap_or_default();
                // 序号仅用于让选中项在滚动窗内可辨，不显示。
                let _ = index;
                lines.push(Line::from(vec![
                    Span::styled(format!("[{cat}] "), base),
                    Span::styled(issue.message.clone(), base),
                    Span::styled(sug, Style::default().fg(theme.insert)),
                ]));
            }
        }
    }

    // 简单滚动：按选中问题所在的行，把窗口对齐（表头也算行）。
    let selected_line = rows
        .iter()
        .position(|r| matches!(r, Row::Issue { selected: true, .. }))
        .unwrap_or(0);
    let h = inner.height as usize;
    let start = selected_line.saturating_sub(h.saturating_sub(1));
    let view: Vec<Line> = lines.into_iter().skip(start).take(h).collect();
    frame.render_widget(Paragraph::new(view), inner);
}

fn render_format_preview(
    frame: &mut ratatui::Frame,
    area: Rect,
    p: &mut FormatPreview,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " 排版预览 · 共 {} 处，已选 {} 处 ",
            p.len(),
            p.included_count()
        ))
        .border_style(Style::default().fg(theme.warning));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    p.set_height(inner.height as usize);

    let lines: Vec<Line> = p
        .items()
        .iter()
        .enumerate()
        .skip(p.scroll())
        .take(inner.height as usize)
        .map(|(i, item)| {
            let text = format_preview::render_item(item);
            let mut style = if item.include {
                Style::default()
            } else {
                // 取消掉的条目压暗——一眼看出它不会被应用。
                Style::default().fg(theme.dim)
            };
            if i == p.cursor() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::styled(text, style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// 统计面板（§6.4 [MUST]：按卷/章列出双口径字数，可导出 CSV）。
/// 批量作业进行中：进度条 + 中断提示（§6.5 `[MUST]`）。
fn render_batch(frame: &mut ratatui::Frame, area: Rect, job: &BatchJob, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {}中… ", job.kind.label()))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 进度条留出 "[]" 和 " 999/999 章" 的位置。
    let bar_width = (inner.width as usize).saturating_sub(16).clamp(4, 60);
    let lines = vec![
        Line::from(format!("范围：{}", job.scope.label())),
        Line::raw(""),
        Line::from(Span::styled(
            job.progress_line(bar_width),
            Style::default().fg(theme.accent),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "Esc 中断（已完成的章保留）",
            Style::default().fg(theme.dim),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// 宽范围作业的确认框。居中浮层，盖在底下那层上。
/// 改名输入框的标题，按 intent 定。删除的提示语由 `start_delete_confirm`
/// 各自带上（要嵌名字/章数），不走这里。
fn rename_prompt(intent: InputIntent) -> &'static str {
    match intent {
        InputIntent::RenameBook(_) => "重命名书",
        InputIntent::RenameVolume(_) => "重命名卷",
        InputIntent::RenameChapter(_) => "重命名章",
        // 删除不经这个函数——真走到了给个通用词，胜过 panic。
        _ => "确认",
    }
}

/// 单行输入框（§6.1/§6.2 改名）。居中小窗，自己 Clear。
fn render_input(frame: &mut ratatui::Frame, area: Rect, input: &Input, theme: &Theme) {
    use unicode_width::UnicodeWidthStr as _;
    // 宽度按内容与提示语算，留出光标位；不小于 30 列。
    let content_w = input.value().width().max(input.title().width()) as u16 + 6;
    let w = content_w.clamp(30, area.width);
    let h = 3u16.min(area.height); // 边框 2 + 一行输入
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    frame.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} (Enter 确认 · Esc 取消) ", input.title()))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // 文本 + 一个块状光标，让人看得见在哪儿输入。
    let line = Line::from(vec![
        Span::raw(input.value().to_string()),
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
}

/// 同意框（§6.8 [MUST]）。
///
/// 这是唯一一个**截断即失效**的框：用户要据此决定把稿子发不发给第三方，
/// 看不全就等于没告知。故窄屏下折行而非截断，且高度按**折行后**的行数算——
/// 只折行不加高，内容会改从底下被切掉，一样是没看全。
fn render_consent(frame: &mut ratatui::Frame, area: Rect, c: &Consent, theme: &Theme) {
    use unicode_width::UnicodeWidthStr;
    let lines = c.lines();
    let want_w = lines
        .iter()
        .map(|l| UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(40)
        .max(40) as u16
        + 4;
    let w = want_w.min(area.width);
    // 折行后实际占几行：宽度不够时一行会摊成好几行。
    let inner_w = w.saturating_sub(2).max(1) as usize;
    let wrapped: usize = lines
        .iter()
        .map(|l| UnicodeWidthStr::width(l.as_str()).div_ceil(inner_w).max(1))
        .sum();
    let want_h = wrapped as u16 + 4;
    let h = want_h.min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    frame.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", c.title()))
        .border_style(Style::default().fg(theme.warning));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [text_area, btn_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let body: Vec<Line> = lines.into_iter().map(Line::from).collect();
    frame.render_widget(
        Paragraph::new(body).wrap(ratatui::widgets::Wrap { trim: false }),
        text_area,
    );

    let sel = Style::default()
        .fg(theme.selection_fg)
        .bg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let plain = Style::default().fg(theme.dim);
    let btns = Line::from(vec![
        Span::styled("  不同意 (Esc)  ", if c.is_yes() { plain } else { sel }),
        Span::raw("   "),
        Span::styled("  同意并发送 (y)  ", if c.is_yes() { sel } else { plain }),
    ]);
    frame.render_widget(Paragraph::new(btns).centered(), btn_area);
}

fn render_confirm(frame: &mut ratatui::Frame, area: Rect, c: &Confirm, theme: &Theme) {
    let lines = c.lines();
    // 宽度按最长一行算（CJK 占两格），高度按行数——内容多大就多大，
    // 不让「全书 200 章」这种关键数字被截掉。
    let want_w = lines
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(20)
        .max(30) as u16
        + 4;
    let want_h = lines.len() as u16 + 4; // 边框 2 + 空行 1 + 按钮行 1
    let w = want_w.min(area.width);
    let h = want_h.min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    frame.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", c.title()))
        .border_style(Style::default().fg(theme.warning));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [text_area, btn_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let body: Vec<Line> = lines.into_iter().map(Line::from).collect();
    frame.render_widget(Paragraph::new(body), text_area);

    // 选中的那个反白。默认停在「取消」。
    let sel = Style::default()
        .fg(theme.selection_fg)
        .bg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let plain = Style::default().fg(theme.dim);
    let btns = Line::from(vec![
        Span::styled("  取消 (Esc)  ", if c.is_yes() { plain } else { sel }),
        Span::raw("   "),
        Span::styled("  执行 (y)  ", if c.is_yes() { sel } else { plain }),
    ]);
    frame.render_widget(
        Paragraph::new(btns).alignment(ratatui::layout::Alignment::Center),
        btn_area,
    );
}

fn render_stats(
    frame: &mut ratatui::Frame,
    area: Rect,
    stats: &Stats,
    rows: &[stats::StatRow],
    book_title: &str,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 统计 · 《{book_title}》 "))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = Stats::render_rows(rows);
    let h = inner.height as usize;

    let lines: Vec<Line> = text
        .iter()
        .skip(stats.scroll())
        .take(h)
        .zip(rows.iter().skip(stats.scroll()))
        .map(|(t, r)| {
            let style = match r {
                stats::StatRow::Volume { .. } => Style::default().add_modifier(Modifier::BOLD),
                stats::StatRow::Total { .. } => Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                stats::StatRow::Chapter { .. } => Style::default(),
            };
            Line::styled(t.clone(), style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tree(frame: &mut ratatui::Frame, area: Rect, ws: &Workspace, theme: &Theme) {
    let focused = ws.focus == Focus::Tree;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 目录 ")
        .border_style(border_style(focused, theme));

    let rows = ws.tree.rows(&ws.book);
    let inner_h = area.height.saturating_sub(2) as usize;

    // 树也要虚拟化：四百章的书不该每帧构造四百行。
    // 滚动位置由 `tree_scroll_top` 算——鼠标命中也要用它，两处必须是同一份。
    let top = tree_scroll_top(area, ws);

    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(top).take(inner_h) {
        let selected = i == ws.tree.cursor();
        let text = match row {
            Row::Volume {
                title,
                expanded,
                chapter_count,
                ..
            } => {
                let arrow = if *expanded { "▾" } else { "▸" };
                format!("{arrow} {title} ({chapter_count})")
            }
            Row::Chapter {
                id,
                title,
                status,
                words,
                damaged,
                ..
            } => {
                let check = if ws.tree.is_checked(*id) { "✓" } else { " " };
                if *damaged {
                    format!(" {check}⚠ {title}")
                } else {
                    format!(
                        " {check}{} {title} {}",
                        status.symbol(),
                        format_words(*words)
                    )
                }
            }
        };
        let style = if selected && focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else if selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(text, style));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_editor(
    frame: &mut ratatui::Frame,
    area: Rect,
    ws: &mut Workspace,
    theme: &Theme,
    show_cursor: bool,
    layout: &EditorLayout,
) -> Rect {
    let focused = ws.focus == Focus::Editor;
    let title = match &ws.editor {
        Some(_) => format!(" 正文 · 《{}》 ", ws.book.title),
        None => " 正文 ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style(focused, theme));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 校对命中集、补全状态（不同字段，与下面对 editor 的可变借用不冲突）。
    let issues = &ws.proof_issues;
    let completion = ws.completion.as_ref();

    let Some(open) = &mut ws.editor else {
        frame.render_widget(
            Paragraph::new(vec![Line::from(""), Line::from("选一章开始写").centered()]),
            inner,
        );
        // 没开章节，正文块是空的——鼠标点这儿什么也定位不到。
        return Rect::default();
    };

    // 版面：先按留白与栏宽算出正文该占的横向范围（§6.10）。
    let (text_x, text_w) = layout.text_span(inner);
    // 行号槽：宽度按最大行号定，够 4 位就留 5 列。
    let gutter = if layout.line_number {
        let digits = open.buffer.text().len_lines().max(1).to_string().len() as u16;
        digits + 1
    } else {
        0
    };
    let body_x = text_x + gutter;
    let body_w = text_w.saturating_sub(gutter).max(1);
    // 正文**真正**落笔的那块（去掉边框、留白、行号槽）。鼠标要按它换算，
    // 拿外框算会差出边框和留白那几列几行，点谁都点不准。
    let body_rect = Rect {
        x: body_x,
        y: inner.y,
        width: body_w,
        height: inner.height,
    };

    // 视口宽度必须**等于实际绘制宽度**，否则折行的位置和画出来的对不上，
    // 行尾会溢出到留白里。
    open.viewport.resize(body_w as usize, inner.height as usize);
    open.viewport
        .set_paragraph_spacing(layout.paragraph_spacing as usize);
    open.viewport.scroll_to_cursor(&open.buffer);

    let visible = open.viewport.visible_lines(&open.buffer);
    let mut lines: Vec<Line> = Vec::new();
    let mut gutter_lines: Vec<Line> = Vec::new();
    for dl in &visible {
        // 段间距撑出来的空行：不画正文，行号槽也留空。
        if dl.is_spacer {
            lines.push(Line::raw(""));
            gutter_lines.push(Line::raw(""));
            continue;
        }
        if gutter > 0 {
            // 只在段首标行号——续行标号会让人以为那是新的一段。
            let label = if dl.is_paragraph_start {
                format!(
                    "{:>width$} ",
                    dl.logical_line + 1,
                    width = (gutter - 1) as usize
                )
            } else {
                " ".repeat(gutter as usize)
            };
            gutter_lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(theme.dim),
            )));
        }

        let text = crate::editor::view::line_slice(&open.buffer, dl.range.clone());
        // 续行缩进，与段首正文左边缘对齐——否则折行看起来像新段落。
        let mut spans: Vec<Span> = Vec::new();
        if dl.indent > 0 {
            spans.push(Span::raw(" ".repeat(dl.indent)));
        }
        // 按校对命中把本行切段，命中处加下划线着色（§6.8 [MUST]）。
        for seg in proof_panel::line_segments(&text, dl.range.start, issues) {
            match seg.hit {
                None => spans.push(Span::raw(seg.text)),
                Some(sev) => spans.push(Span::styled(
                    seg.text,
                    Style::default()
                        .fg(proof_severity_color(sev, theme))
                        .add_modifier(Modifier::UNDERLINED),
                )),
            }
        }
        lines.push(Line::from(spans));
    }

    if gutter > 0 {
        frame.render_widget(
            Paragraph::new(gutter_lines),
            Rect {
                x: text_x,
                y: inner.y,
                width: gutter,
                height: inner.height,
            },
        );
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.fg)),
        Rect {
            x: body_x,
            y: inner.y,
            width: body_w,
            height: inner.height,
        },
    );

    let cursor_pos = open.viewport.cursor_screen_pos(&open.buffer);

    // 光标只在编辑器有焦点时显示。位置要跟着正文一起右移（留白 + 行号槽）。
    if focused
        && show_cursor
        && let Some((col, row)) = cursor_pos
    {
        frame.set_cursor_position((body_x + col, inner.y + row));
    }

    // @ 补全弹框（§6.7）：贴着光标下方列候选。
    if let (Some(comp), Some((col, row))) = (completion, cursor_pos) {
        let cursor = open.buffer.cursor();
        let at = comp.at();
        let filter = if cursor > at {
            open.buffer
                .contents()
                .get(at + 1..cursor)
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };
        let cands = comp.candidates(&filter);
        if !cands.is_empty() {
            let shown = cands.len().min(6);
            let popup_w = cands
                .iter()
                .take(shown)
                .map(|s| unicode_width::UnicodeWidthStr::width(*s))
                .max()
                .unwrap_or(4)
                .clamp(4, 24) as u16
                + 2;
            let popup_h = shown as u16 + 2;
            // 光标下方；贴着底或右边时上移/左移，别出界。
            let x = (inner.x + col).min(inner.x + inner.width.saturating_sub(popup_w));
            let below = inner.y + row + 1;
            let y = if below + popup_h <= inner.y + inner.height {
                below
            } else {
                (inner.y + row).saturating_sub(popup_h)
            };
            let pw = popup_w.min(area.width);
            let ph = popup_h.min(area.height);
            let popup = Rect {
                x,
                y,
                width: pw,
                height: ph,
            };
            frame.render_widget(ratatui::widgets::Clear, popup);
            let pblock = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent));
            let pinner = pblock.inner(popup);
            frame.render_widget(pblock, popup);
            let items: Vec<Line> = cands
                .iter()
                .take(shown)
                .enumerate()
                .map(|(i, name)| {
                    let style = if i == comp.cursor() {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    Line::styled((*name).to_string(), style)
                })
                .collect();
            frame.render_widget(Paragraph::new(items), pinner);
        }
    }
    body_rect
}

/// 角色卡表单编辑（§6.7）。字段逐行，聚焦项高亮，编辑态末尾加光标符。
fn render_character_form(
    frame: &mut ratatui::Frame,
    area: Rect,
    form: &mut CharacterForm,
    theme: &Theme,
) {
    let dirty = if form.is_dirty() { " ●未保存" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 编辑角色{dirty} "))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in form.fields().iter().enumerate() {
        let focused = i == form.focus();
        let editing = focused && form.is_editing();
        let label_style = if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim)
        };
        let marker = if focused { "▸ " } else { "  " };
        // 多行值可能含换行；首行跟在标签后，其余行缩进对齐。
        let mut value_lines = field.value.split('\n');
        let first = value_lines.next().unwrap_or("");
        let cursor = if editing { "▏" } else { "" };
        let val_style = if editing {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{}：", field.key.label()), label_style),
            Span::styled(
                format!(
                    "{first}{}",
                    if value_lines.clone().next().is_none() {
                        cursor
                    } else {
                        ""
                    }
                ),
                val_style,
            ),
        ]));
        for (n, extra) in value_lines.clone().enumerate() {
            let is_last = value_lines.clone().count() == n + 1;
            lines.push(Line::from(Span::styled(
                format!(
                    "      {extra}{}",
                    if editing && is_last { cursor } else { "" }
                ),
                val_style,
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// 命令面板（Ctrl+P，§7.3）。居中小窗：查询行 + 候选列表。
fn render_palette(frame: &mut ratatui::Frame, area: Rect, p: &mut CommandPalette, theme: &Theme) {
    // 居中，宽度取 area 的六成（下限 40 上限 70），高度按候选数。
    let w = (area.width * 6 / 10)
        .clamp(40.min(area.width), 70)
        .min(area.width);
    let rows = p.match_count().clamp(1, 12) as u16;
    let h = (rows + 4).min(area.height); // 边框 2 + 查询行 1 + 分隔 1
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 3; // 略偏上，符合观感
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 命令 · {} 条 ", p.match_count()))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [query_area, list_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", Style::default().fg(theme.accent)),
            Span::styled(p.query().to_string(), Style::default().fg(theme.fg)),
            Span::styled("▏", Style::default().fg(theme.accent)),
        ])),
        query_area,
    );

    p.set_height(list_area.height as usize);
    let matches = p.matches();
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "没有匹配的命令",
                Style::default().fg(theme.dim),
            )),
            list_area,
        );
        return;
    }

    let key_w = 10usize;
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(p.scroll())
        .take(list_area.height as usize)
        .map(|(i, c)| {
            let selected = i == p.cursor();
            let base = if selected {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            let keys = format!("{:>key_w$}", c.keys);
            Line::from(vec![
                Span::styled(format!("{} ", c.name), base),
                Span::styled(
                    c.desc.to_string(),
                    if selected {
                        base
                    } else {
                        Style::default().fg(theme.dim)
                    },
                ),
                Span::styled(keys, Style::default().fg(theme.accent)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);
}

/// 外观设置（§6.10）。字体不可用的那两栏置灰并给出原因，末尾附配置片段。
fn render_settings(frame: &mut ratatui::Frame, area: Rect, s: &mut Settings, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " 外观 · 终端：{}（←/→ 换主题，y 复制片段，Esc 关闭） ",
            s.terminal().label()
        ))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in s.rows().iter().enumerate() {
        let selected = i == s.cursor();
        let disabled = s.is_disabled(*row);
        // 灰态：不可用的项压暗，用户一眼看出「这里点了也没用」（§6.10 [MUST]）。
        let value_style = if disabled {
            Style::default().fg(theme.dim)
        } else if selected {
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
        } else {
            Style::default().fg(theme.fg)
        };
        let label_style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim)
        };
        let marker = if selected { "▸ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{:<10}", row.label()), label_style),
            Span::styled(s.value_of(*row), value_style),
        ]));
        if let Some(note) = s.note_of(*row) {
            lines.push(Line::from(Span::styled(
                format!("            {note}"),
                Style::default().fg(theme.dim),
            )));
        }
    }

    // 配置片段正文——把「做不到」变成「帮你做到」的那一段（§2.1 三级降级）。
    if let Some(snip) = s.snippet() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "── 配置片段（y 复制）──",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for l in snip.lines() {
            lines.push(Line::from(Span::styled(
                format!("    {l}"),
                Style::default().fg(theme.warning),
            )));
        }
    }

    let view: Vec<Line> = lines.into_iter().take(inner.height as usize).collect();
    frame.render_widget(Paragraph::new(view), inner);
}

/// 帮助页（F1，§7.3）：键位总表，内容由命令表生成。
fn render_help(
    frame: &mut ratatui::Frame,
    area: Rect,
    h: &mut Help,
    theme: &Theme,
    keymap: &crate::keymap::Keymap,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 帮助 · 键位总表（j/k 滚动，Esc 关闭） ")
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    h.set_height(inner.height as usize);
    let rows = Help::rows_with(keymap);
    let lines: Vec<Line> = rows
        .iter()
        .skip(h.scroll())
        .take(inner.height as usize)
        .map(|r| match r {
            help::HelpRow::Section(s) => Line::from(Span::styled(
                format!("── {s} ──"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            help::HelpRow::Blank => Line::raw(""),
            help::HelpRow::Entry { keys, what } => Line::from(vec![
                Span::styled(format!("  {keys:<24}"), Style::default().fg(theme.warning)),
                Span::styled(what.clone(), Style::default().fg(theme.fg)),
            ]),
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// 角色速查侧栏 / 列表页（Alt+C，§6.7）。宽屏左列表右详情，窄屏只列表。
fn render_characters(
    frame: &mut ratatui::Frame,
    area: Rect,
    p: &mut CharacterPanel,
    theme: &Theme,
) {
    // 出场统计视图（t 打开，§6.7 [SHOULD]）。
    if p.show_stats() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 角色出场统计 · 消失最久在前 ")
            .border_style(Style::default().fg(theme.accent));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        p.set_height(inner.height as usize);
        let (scroll, cursor) = (p.scroll(), p.cursor());
        let stats = p.stats().unwrap_or(&[]);
        let lines: Vec<Line> = stats
            .iter()
            .enumerate()
            .skip(scroll)
            .take(inner.height as usize)
            .map(|(i, a)| {
                // 长期未出现的标黄，未出场的压暗。
                let mut style = if a.total == 0 {
                    Style::default().fg(theme.dim)
                } else if a.chapters_since_last().is_some_and(|n| n >= 3) {
                    Style::default().fg(theme.warning)
                } else {
                    Style::default()
                };
                if i == cursor {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                Line::styled(CharacterPanel::stat_line(a), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let title = if p.is_searching() {
        format!(" 角色 · 搜索：{}▏", p.query())
    } else {
        format!(" 角色 · {} 位 ", p.total())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if p.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("还没有角色。").centered(),
                Line::from("按 n 新建。").centered(),
            ]),
            inner,
        );
        return;
    }

    // 宽度够就分栏：左列表、右详情。
    let (list_area, detail_area) = if inner.width >= 60 {
        let [l, r] = Layout::horizontal([Constraint::Length(28), Constraint::Min(0)]).areas(inner);
        (l, Some(r))
    } else {
        (inner, None)
    };

    p.set_height(list_area.height as usize);
    let filtered = p.filtered();
    let list: Vec<Line> = filtered
        .iter()
        .enumerate()
        .skip(p.scroll())
        .take(list_area.height as usize)
        .map(|(i, c)| {
            let mut style = Style::default();
            if i == p.cursor() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::styled(CharacterPanel::summary_line(c), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(list), list_area);

    if let Some(da) = detail_area {
        // 竖分隔线用左边框近似。
        let dblock = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.border));
        let dinner = dblock.inner(da);
        frame.render_widget(dblock, da);
        if let Some(c) = p.current() {
            let lines: Vec<Line> = p
                .detail_lines(c)
                .into_iter()
                .take(dinner.height as usize)
                .map(Line::from)
                .collect();
            frame.render_widget(Paragraph::new(lines), dinner);
        }
    }
}

/// 正文版面参数（§6.10 `[appearance]` + §7.3 专注模式）。
///
/// 把它们从 config 摘出来单独传，是因为专注模式要**临时覆盖**栏宽：
/// 直接读 config 就得在渲染里到处判断「现在是不是专注模式」。
#[derive(Debug, Clone, Copy)]
struct EditorLayout {
    /// 正文栏宽，单位「全角字」；0 = 撑满。
    column_width: u16,
    /// 左右留白列数。
    margin: u16,
    /// 段间额外空行。
    paragraph_spacing: u16,
    line_number: bool,
}

impl EditorLayout {
    fn from_config(config: &Config, focus_mode: bool) -> Self {
        let a = &config.appearance;
        Self {
            // 专注模式用 editor.focus_column_width 覆盖常规栏宽（§8）。
            column_width: if focus_mode {
                config.editor.focus_column_width
            } else {
                a.column_width
            },
            margin: a.margin,
            paragraph_spacing: a.paragraph_spacing,
            // 专注模式下不显示行号：那是「专注」的反面。
            line_number: a.line_number && !focus_mode,
        }
    }

    /// 在给定可用区里算出正文该占的横向范围。
    ///
    /// 顺序：先扣左右留白，再按栏宽收窄并**居中**——居中是长文本可读性的关键，
    /// 满屏宽的中文正文一行三四十字，眼睛来回扫得很累。
    /// 全程 saturating：§7.2 `[MUST]` 窄至 60 列不崩，别让减法翻负。
    fn text_span(&self, area: Rect) -> (u16, u16) {
        let margin = self.margin.min(area.width / 4);
        let avail = area.width.saturating_sub(margin * 2).max(1);
        // 栏宽单位是全角字，一个全角字占两列。
        let want = if self.column_width == 0 {
            avail
        } else {
            (self.column_width.saturating_mul(2)).min(avail)
        };
        let x = area.x + margin + (avail.saturating_sub(want)) / 2;
        (x, want.max(1))
    }
}

/// 校对严重度配色（§6.8：Error 红 / Warning 黄 / Hint 暗）。正文下划线与面板共用。
fn proof_severity_color(sev: mj_text::proof::Severity, theme: &Theme) -> Color {
    use mj_text::proof::Severity;
    match sev {
        Severity::Error => theme.error,
        Severity::Warning => theme.warning,
        Severity::Hint => theme.hint,
    }
}

fn border_style(focused: bool, theme: &Theme) -> Style {
    if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.border)
    }
}

/// 起窗 → 跑循环 → 恢复终端。
///
/// 恢复不依赖循环正常返回：`run_loop` 出错时也要先恢复再传播错误，
/// 否则用户会拿到一个卡在 alternate screen 里的终端（doc.md §6.10）。
pub fn run(store: Store, config: Config) -> anyhow::Result<()> {
    let mut app = App::new(store, config)?;
    let mut term = ratatui::try_init()?;

    // kitty 键盘协议：探测到就开，让 Ctrl+Shift+S / Ctrl+Tab 到得了程序（§2.3、§7.3）。
    // 不支持时静默降级——缺的只是两个键位，不该拦着人写字。
    // 必须在起窗之后开：它是写给终端的转义序列，得在 alternate screen 里发。
    if crate::keyboard::enable() {
        app.note_keyboard_protocol();
    }

    // 鼠标捕获（§13）。默认关——开了终端自己的拖选复制就没了，见 config 的注释。
    //
    // 关掉它是**必须做到**的事，不是收尾的客气：留着捕获退出去，用户的终端
    // 就在这个目录下再也划不动选区了，而他多半想不到是刚才那个程序干的。
    // 故下面 disable 不受 result 影响，无论 run_loop 怎么结束都要跑。
    let mouse = app.mouse_enabled();
    if mouse {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::EnableMouseCapture
        );
    }

    let events = EventLoop::spawn();
    let result = app.run_loop(&mut term, &events);

    // 顺序要紧：先把我们改过的终端状态收回来，再交还给 ratatui 复原。
    if mouse {
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::event::DisableMouseCapture
        );
    }
    crate::keyboard::disable();
    crate::font::emit_reset_sequence();
    ratatui::try_restore()?;
    result
}
