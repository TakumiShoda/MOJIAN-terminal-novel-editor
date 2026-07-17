//! 应用状态机与运行入口。见 doc.md §7。
//!
//! 屏幕状态机（§7.1）：
//! ```text
//! Shelf(书架) ──open──> Workspace(工作区) ──Esc──> Shelf
//! Workspace = Tree | Editor 双焦点 + 底部状态栏
//! ```

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::model::{Book, ChapterBody};
use mj_core::store::Store;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::editor::{Action, AutoSave, Buffer, Viewport};
use crate::event::{AppEvent, EventLoop};
use crate::screens::format_preview::{self, FormatPreview};
use crate::screens::history_panel::{DiffView, HistoryPanel, LineKind};
use crate::screens::search_panel::{self, SearchPanel};
use crate::screens::shelf::{Shelf, format_words};
use crate::screens::stats::{self, Stats};
use crate::screens::tree::{Row, Tree};

/// §7.2：目录树宽度默认 24 列。
const TREE_WIDTH: u16 = 24;
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
    /// 统计面板（F3）。Some = 正在显示。
    ///
    /// M1/M2 尚无浮层栈（§7.1 的 `Vec<Modal>` 属 M6），故先用一个
    /// Option 表示。M6 接入浮层栈时这里会并进去——但那不该拖着
    /// §6.4 的 [MUST] 一起等。
    stats: Option<Stats>,
    /// 排版预览面板（F5，§6.5 [MUST]）。同上，M6 并入浮层栈。
    format_preview: Option<FormatPreview>,
    /// 查找替换面板（Ctrl+F / Ctrl+H，§6.6）。同上。
    search: Option<SearchPanel>,
    /// 历史面板（F8，§6.9）。同上。
    history: Option<HistoryPanel>,
    /// diff 视图（历史面板里 Enter 打开，§6.9）。同上。
    diff: Option<DiffView>,
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
}

impl App {
    pub fn new(store: Store, config: Config) -> anyhow::Result<Self> {
        let books = store.list_books()?;

        Ok(Self {
            store,
            config,
            // 索引按书打开（§5.1：books/<id>/.index.sqlite），故此处为空。
            index: None,
            screen: Screen::Shelf(Shelf::new(books)),
            should_quit: false,
            dirty: true,
            toast: None,
            today_words: 0,
            last_snapshot: None,
        })
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
        while !self.should_quit {
            if self.dirty {
                term.draw(|f| self.render(f))?;
                self.dirty = false;
            }

            match events.next()? {
                AppEvent::Term(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    self.on_key(k.code, k.modifiers)?;
                    self.dirty = true;
                }
                AppEvent::Term(Event::Resize(_, _)) => self.dirty = true,
                AppEvent::Term(_) => {}
                // 自动保存的心跳（§7.4：Tick 驱动自动保存计时）。
                AppEvent::Tick => self.on_tick()?,
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
            stats: None,
            format_preview: None,
            search: None,
            history: None,
            diff: None,
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
        // 浮层打开时它吃掉所有按键（浮层语义，§7.1）。
        if matches!(&self.screen, Screen::Workspace(ws) if ws.diff.is_some()) {
            return self.on_key_diff(code);
        }
        if matches!(&self.screen, Screen::Workspace(ws) if ws.history.is_some()) {
            return self.on_key_history(code);
        }
        if matches!(&self.screen, Screen::Workspace(ws) if ws.search.is_some()) {
            return self.on_key_search(code, mods);
        }
        if matches!(&self.screen, Screen::Workspace(ws) if ws.format_preview.is_some()) {
            return self.on_key_format_preview(code);
        }
        if matches!(&self.screen, Screen::Workspace(ws) if ws.stats.is_some()) {
            return self.on_key_stats(code);
        }

        // 先处理全局键。
        match code {
            // F5 一键排版（当前章，弹预览）——§7.3 键位表。
            KeyCode::F(5) => return self.open_format_preview(),
            // F8 历史面板（§7.3）。
            KeyCode::F(8) => return self.open_history(),

            // 手动打快照。
            //
            // §7.3 指定的是 Ctrl+Shift+S，但**在传统键盘模式下它根本到不了**：
            // 终端对 Ctrl+S 与 Ctrl+Shift+S 发的是同一个字节（0x13），Shift 压根没编码进去。
            // 要区分得开 kitty 键盘协议（§2.3：可选，需运行时探测），那是 M6 的活。
            //
            // 一个永远不触发的键位比没有更糟——用户会以为功能坏了。
            // 故 F9 作为当下真正可用的入口；Ctrl+Shift+S 的分支留着，
            // M6 开了协议它自然就活了。
            KeyCode::F(9) => return self.manual_snapshot(),
            KeyCode::Char('S') if mods.contains(KeyModifiers::CONTROL) => {
                return self.manual_snapshot();
            }
            // Ctrl+F 查找 / Ctrl+H 查找替换（§7.3）。
            KeyCode::Char('f') if mods.contains(KeyModifiers::CONTROL) => {
                return self.open_search(false);
            }
            KeyCode::Char('h') if mods.contains(KeyModifiers::CONTROL) => {
                return self.open_search(true);
            }
            // F3 打开统计面板（§6.4 [MUST]）。
            // §7.3 的键位表没给它分配键——F5/F7/F8 已被排版/校对/历史占用，
            // F3 空着且相邻，故取之。M6 做命令面板时会有正式入口。
            KeyCode::F(3) => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.stats = Some(Stats::new());
                }
                return Ok(());
            }
            KeyCode::Char('b') if mods.contains(KeyModifiers::CONTROL) => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.show_tree = !ws.show_tree;
                    if !ws.show_tree {
                        ws.focus = Focus::Editor;
                    }
                }
                return Ok(());
            }
            KeyCode::Char('s') if mods.contains(KeyModifiers::CONTROL) => {
                return self.save_current();
            }
            KeyCode::Tab => {
                if let Screen::Workspace(ws) = &mut self.screen {
                    ws.focus = match ws.focus {
                        Focus::Tree => Focus::Editor,
                        Focus::Editor => Focus::Tree,
                    };
                }
                return Ok(());
            }
            _ => {}
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
            self.toast = Some("本章还没有快照（Ctrl+S 保存或 Ctrl+Shift+S 手动打一条）".into());
            return Ok(());
        }
        if let Screen::Workspace(ws) = &mut self.screen {
            ws.history = Some(HistoryPanel::new(snaps));
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
        let Some(p) = &mut ws.history else {
            return Ok(());
        };
        match code {
            KeyCode::Esc | KeyCode::F(8) | KeyCode::Char('q') => ws.history = None,
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
        let (Some(p), Some(open), book) = (&ws.history, &ws.editor, ws.book.id) else {
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
            ws.history = Some(HistoryPanel::new(snaps));
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
        let (Some(p), Some(open), book) = (&ws.history, &ws.editor, ws.book.id) else {
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
            ws.history = None;
            ws.diff = Some(view);
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
        let Some(v) = &mut ws.diff else { return Ok(()) };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => ws.diff = None,
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
        let Some(v) = &ws.diff else { return Ok(()) };
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
        ws.diff = None;

        self.toast = Some("已恢复此块，Ctrl+Z 可撤销".into());
        Ok(())
    }

    /// `U`：整章恢复（§6.9 恢复粒度 1）。
    fn restore_whole_chapter(&mut self) -> anyhow::Result<()> {
        let old = match &self.screen {
            Screen::Workspace(ws) => ws.diff.as_ref().map(|v| v.old_text().to_string()),
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
        ws.diff = None;

        self.toast = Some("已恢复整章。恢复前的版本已存为快照，可再退回去".into());
        Ok(())
    }

    /// `y`：复制旧内容到剪贴板，不改当前版本（§6.9 恢复粒度 3）。
    fn copy_old_content(&mut self) -> anyhow::Result<()> {
        let text = match &self.screen {
            Screen::Workspace(ws) => ws.diff.as_ref().map(|v| v.copy_text()),
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
        ws.search = Some(SearchPanel::new(replace_mode));
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

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = &mut ws.search else {
            return Ok(());
        };
        let text = ws.editor.as_ref().map(|o| o.buffer.contents());

        let mut need_refresh = false;
        match code {
            KeyCode::Esc => {
                ws.search = None;
                return Ok(());
            }
            KeyCode::Tab => p.next_field(),
            // Enter：跳到当前命中处（§6.6）。
            KeyCode::Enter => {
                let target = p.current_hit().map(|h| h.range.start);
                if let Some(pos) = target {
                    ws.search = None;
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
        let Some(p) = &mut ws.search else {
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
        let Some(p) = &mut ws.search else {
            return Ok(());
        };
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
        let Some(p) = &mut ws.search else {
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
        ws.format_preview = Some(FormatPreview::new(&text, edits));
        Ok(())
    }

    /// 排版预览的按键（§6.5：可逐条取消）。
    fn on_key_format_preview(&mut self, code: KeyCode) -> anyhow::Result<()> {
        // Enter 要改正文，得先脱离对 ws 的借用。
        if code == KeyCode::Enter {
            return self.apply_format();
        }

        let Screen::Workspace(ws) = &mut self.screen else {
            return Ok(());
        };
        let Some(p) = &mut ws.format_preview else {
            return Ok(());
        };

        match code {
            KeyCode::Esc | KeyCode::F(5) | KeyCode::Char('q') => ws.format_preview = None,
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
        let Some(p) = ws.format_preview.take() else {
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
        let Some(stats) = &mut ws.stats else {
            return Ok(());
        };

        match code {
            KeyCode::Esc | KeyCode::F(3) | KeyCode::Char('q') => ws.stats = None,
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

    fn on_key_tree(&mut self, code: KeyCode, _mods: KeyModifiers) -> anyhow::Result<()> {
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

    fn on_key_editor(&mut self, code: KeyCode, _mods: KeyModifiers) -> anyhow::Result<()> {
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
            if let Some(p) = &mut ws.search {
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
            ws.stats = Some(Stats::new());
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
            Screen::Workspace(ws) if ws.stats.is_some() => {
                Some(Stats::rows(&ws.book, self.no_punct_lookup()))
            }
            _ => None,
        };

        match &mut self.screen {
            Screen::Shelf(shelf) => render_shelf(frame, body, shelf),
            Screen::Workspace(ws) => {
                if let Some(v) = &mut ws.diff {
                    render_diff(frame, body, v);
                } else if let Some(p) = &mut ws.history {
                    render_history(frame, body, p);
                } else if let Some(p) = &mut ws.search {
                    render_search(frame, body, p);
                } else if let Some(p) = &mut ws.format_preview {
                    render_format_preview(frame, body, p);
                } else if let (Some(st), Some(rows)) = (&ws.stats, stats_rows) {
                    render_stats(frame, body, st, &rows, &ws.book.title);
                } else {
                    render_workspace(frame, body, ws);
                }
            }
        }
        self.render_status(frame, status);
    }

    fn render_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut spans: Vec<Span> = Vec::new();

        if let Some(t) = &self.toast {
            spans.push(Span::raw(format!(" {t} ")));
        } else {
            match &self.screen {
                Screen::Shelf(s) => {
                    spans.push(Span::raw(format!(" {} 本书 ", s.books().len())));
                    spans.push(Span::raw("│ Enter 打开 │ n 新建 │ q 退出 "));
                }
                Screen::Workspace(ws) if ws.diff.is_some() => {
                    // §12.4 的底栏。
                    spans.push(Span::raw(
                        " n/p 跳转改动 │ u 恢复此块 │ U 恢复整章 │ y 复制旧内容 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.history.is_some() => {
                    spans.push(Span::raw(
                        " 历史 │ Enter 看 diff │ Space 选对照条 │ P 钉住 │ Esc 关闭 ",
                    ));
                }
                Screen::Workspace(ws) if ws.search.is_some() => {
                    let hint = if ws.search.as_ref().is_some_and(|s| s.replace_mode) {
                        " 查找替换 │ Tab 切换栏 │ Space 勾选 │ Alt+R 替换本条 │ Alt+A 替换勾选 │ Enter 跳转 │ Esc 关闭 "
                    } else {
                        " 查找 │ Tab 切到结果 │ Enter 跳转 │ Esc 关闭 "
                    };
                    spans.push(Span::raw(hint));
                }
                Screen::Workspace(ws) if ws.format_preview.is_some() => {
                    spans.push(Span::raw(
                        " 排版预览 │ Space 逐条取消 │ a 全选 │ n 全不选 │ Enter 应用 │ Esc 放弃 ",
                    ));
                }
                Screen::Workspace(ws) if ws.stats.is_some() => {
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
                            Style::default().fg(Color::Cyan),
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
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ));

                    if let Some(open) = &ws.editor {
                        spans.push(Span::raw("│ "));
                        if open.buffer.is_dirty() {
                            spans.push(Span::styled("●未保存", Style::default().fg(Color::Yellow)));
                        } else {
                            spans.push(Span::raw("已保存"));
                        }
                        spans.push(Span::raw(" "));
                    }
                    spans.push(Span::raw(
                        "│ Ctrl+S 保存 │ F5 排版 │ F8 历史 │ F9 快照 │ Esc 返回 ",
                    ));

                    // §7.2：窄屏隐藏侧栏时要在状态栏提示。
                    if frame.area().width < NARROW_THRESHOLD {
                        spans.push(Span::styled(
                            "│ 窄屏：侧栏已隐藏",
                            Style::default().fg(Color::DarkGray),
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

fn render_shelf(frame: &mut ratatui::Frame, area: Rect, shelf: &Shelf) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 墨简 · 书架 ");

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
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(line, style));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_workspace(frame: &mut ratatui::Frame, area: Rect, ws: &mut Workspace) {
    // §7.2：窄屏（< 80 列）自动隐藏侧栏，只留正文。
    let narrow = area.width < NARROW_THRESHOLD;
    let show_tree = ws.show_tree && !narrow;

    let (tree_area, editor_area) = if show_tree {
        let [t, e] =
            Layout::horizontal([Constraint::Length(TREE_WIDTH), Constraint::Min(0)]).areas(area);
        (Some(t), e)
    } else {
        (None, area)
    };

    if let Some(ta) = tree_area {
        render_tree(frame, ta, ws);
    }
    render_editor(frame, editor_area, ws);
}

/// 历史面板（§6.9）。
fn render_history(frame: &mut ratatui::Frame, area: Rect, p: &mut HistoryPanel) {
    let title = match p.compare_target() {
        Some(_) => " 历史 · 已选对照条，Enter 两条互比 ".to_string(),
        None => format!(" 历史 · {} 条快照 ", p.snapshots().len()),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Magenta));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    p.set_height(inner.height as usize);

    let lines: Vec<Line> = (p.scroll()..p.snapshots().len())
        .take(inner.height as usize)
        .map(|i| {
            let mut style = Style::default();
            if p.snapshots()[i].is_protected() {
                // 受保护的醒目一点——它们是用户特意留下的锚点。
                style = style.fg(Color::Yellow);
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
fn render_diff(frame: &mut ratatui::Frame, area: Rect, v: &mut DiffView) {
    // §12.4：标题栏是「与「…」比较 ─── +312 / -87 / 3 处改动」。
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " 与「{}」比较 ─── {} ",
            v.old_title,
            v.summary_line()
        ))
        .border_style(Style::default().fg(Color::Magenta));
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
                LineKind::Insert => ("+", Style::default().fg(Color::Green)),
                LineKind::Delete => ("-", Style::default().fg(Color::Red)),
                LineKind::Equal => (" ", Style::default().fg(Color::DarkGray)),
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
fn render_search(frame: &mut ratatui::Frame, area: Rect, p: &mut SearchPanel) {
    let title = if p.replace_mode {
        " 查找替换 · 当前章 "
    } else {
        " 查找 · 当前章 "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));
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
    ))];
    if p.replace_mode {
        input_lines.push(Line::from(field_line(
            "替换",
            &p.replace_with,
            p.field() == search_panel::Field::Replace,
        )));
    }
    frame.render_widget(Paragraph::new(input_lines), inputs);

    frame.render_widget(
        Paragraph::new(p.options_line()).style(Style::default().fg(Color::DarkGray)),
        options,
    );

    // 摘要：命中数，或非法正则的实时提示（§6.6 [MUST]）。
    let summary_style = if p.error().is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
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
                    base.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(after.to_string(), base),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), results);
}

/// 一行输入框。有焦点的那行用 ▸ 与下划线标出。
fn field_line(label: &str, value: &str, focused: bool) -> Vec<Span<'static>> {
    let marker = if focused { "▸" } else { " " };
    let style = if focused {
        Style::default().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    vec![
        Span::raw(format!("{marker} {label}: ")),
        Span::styled(value.to_string(), style),
        // 光标提示：终端光标被正文占着，这里用一个块字符代替。
        Span::styled(if focused { "▏" } else { "" }, style),
    ]
}

/// 排版预览（§6.5 [MUST]：显示将改动的位置与条数，可逐条取消）。
fn render_format_preview(frame: &mut ratatui::Frame, area: Rect, p: &mut FormatPreview) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " 排版预览 · 共 {} 处，已选 {} 处 ",
            p.len(),
            p.included_count()
        ))
        .border_style(Style::default().fg(Color::Yellow));

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
                Style::default().fg(Color::DarkGray)
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
fn render_stats(
    frame: &mut ratatui::Frame,
    area: Rect,
    stats: &Stats,
    rows: &[stats::StatRow],
    book_title: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 统计 · 《{book_title}》 "))
        .border_style(Style::default().fg(Color::Cyan));
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
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                stats::StatRow::Chapter { .. } => Style::default(),
            };
            Line::styled(t.clone(), style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tree(frame: &mut ratatui::Frame, area: Rect, ws: &Workspace) {
    let focused = ws.focus == Focus::Tree;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 目录 ")
        .border_style(border_style(focused));

    let rows = ws.tree.rows(&ws.book);
    let inner_h = area.height.saturating_sub(2) as usize;

    // 树也要虚拟化：四百章的书不该每帧构造四百行。
    let top = ws
        .tree
        .cursor()
        .saturating_sub(inner_h / 2)
        .min(rows.len().saturating_sub(inner_h));

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

fn render_editor(frame: &mut ratatui::Frame, area: Rect, ws: &mut Workspace) {
    let focused = ws.focus == Focus::Editor;
    let title = match &ws.editor {
        Some(_) => format!(" 正文 · 《{}》 ", ws.book.title),
        None => " 正文 ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style(focused));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(open) = &mut ws.editor else {
        frame.render_widget(
            Paragraph::new(vec![Line::from(""), Line::from("选一章开始写").centered()]),
            inner,
        );
        return;
    };

    // 用真实尺寸校正视口——渲染前不知道终端多大。
    open.viewport
        .resize(inner.width as usize, inner.height as usize);
    open.viewport.scroll_to_cursor(&open.buffer);

    let lines: Vec<Line> = open
        .viewport
        .visible_lines(&open.buffer)
        .iter()
        .map(|dl| {
            let text = crate::editor::view::line_slice(&open.buffer, dl.range.clone());
            // 续行缩进，与段首正文左边缘对齐——否则折行看起来像新段落。
            Line::raw(format!("{}{}", " ".repeat(dl.indent), text))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);

    // 光标只在编辑器有焦点时显示。
    if focused && let Some((col, row)) = open.viewport.cursor_screen_pos(&open.buffer) {
        frame.set_cursor_position((inner.x + col, inner.y + row));
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// 起窗 → 跑循环 → 恢复终端。
///
/// 恢复不依赖循环正常返回：`run_loop` 出错时也要先恢复再传播错误，
/// 否则用户会拿到一个卡在 alternate screen 里的终端（doc.md §6.10）。
pub fn run(store: Store, config: Config) -> anyhow::Result<()> {
    let mut app = App::new(store, config)?;
    let mut term = ratatui::try_init()?;
    let events = EventLoop::spawn();

    let result = app.run_loop(&mut term, &events);

    crate::font::emit_reset_sequence();
    ratatui::try_restore()?;
    result
}
