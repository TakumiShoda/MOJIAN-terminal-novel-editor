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
use crate::screens::shelf::{Shelf, format_words};
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
}

struct OpenChapter {
    id: ChapterId,
    buffer: Buffer,
    viewport: Viewport,
    autosave: AutoSave,
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
    screen: Screen,
    should_quit: bool,
    /// 仅在状态变化时重绘（§7.4：不要固定 60fps 空转）。
    dirty: bool,
    /// 底部提示语，操作后给一句反馈。
    toast: Option<String>,
}

impl App {
    pub fn new(store: Store, config: Config) -> anyhow::Result<Self> {
        let books = store.list_books()?;
        Ok(Self {
            store,
            config,
            screen: Screen::Shelf(Shelf::new(books)),
            should_quit: false,
            dirty: true,
            toast: None,
        })
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
        let mut ws = Workspace {
            book,
            tree: Tree::new(),
            editor: None,
            focus: Focus::Tree,
            show_tree: true,
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
        // 先处理全局键。
        match code {
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
                ws.focus = Focus::Tree;
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
            KeyCode::Left => open.buffer.move_left(),
            KeyCode::Right => open.buffer.move_right(),
            KeyCode::Home => open.buffer.move_home(),
            KeyCode::End => open.buffer.move_end(),
            KeyCode::Up => open.viewport.scroll_up(1),
            KeyCode::Down => open.viewport.scroll_down(1, &open.buffer),
            _ => {}
        }
        if edited {
            open.autosave.on_edit(std::time::Instant::now());
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

        ws.editor = Some(OpenChapter {
            id,
            buffer,
            viewport: Viewport::new(40, 20), // 真实尺寸在渲染时校正
            autosave: AutoSave::new(
                self.config.editor.autosave_idle_ms,
                self.config.editor.autosave_words,
            ),
            path,
        });
        ws.tree.focus_chapter(&ws.book, id);
        Ok(())
    }

    /// Tick 驱动的自动保存（§6.3、§7.4）。
    fn on_tick(&mut self) -> anyhow::Result<()> {
        let now = std::time::Instant::now();

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

        let body = ChapterBody::new(open.id, open.buffer.contents());
        // save_body 内部会清掉 swp——正文既已落盘，swp 留着只会下次误报。
        self.store.save_body(ws.book.id, &body)?;
        open.buffer.mark_saved();
        // 手动保存同样要重置自动保存的计时，否则它还以为欠着一次保存。
        open.autosave.on_saved();

        // 保存后重载元数据，让树上的字数跟上。
        ws.book = self.store.load_book(ws.book.id)?;
        self.toast = Some("已保存".into());
        Ok(())
    }

    // ---- 渲染 ----

    #[doc(hidden)]
    pub fn render_for_test(&mut self, frame: &mut ratatui::Frame) {
        self.render(frame);
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

        match &mut self.screen {
            Screen::Shelf(shelf) => render_shelf(frame, body, shelf),
            Screen::Workspace(ws) => render_workspace(frame, body, ws),
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
                Screen::Workspace(ws) => {
                    if let Some(open) = &ws.editor {
                        let words = mj_text::count::count_with_punct(&open.buffer.contents());
                        spans.push(Span::raw(format!(" 本章 {} ", format_words(words as u64))));
                        spans.push(Span::raw("│ "));
                        if open.buffer.is_dirty() {
                            spans.push(Span::styled("●未保存", Style::default().fg(Color::Yellow)));
                        } else {
                            spans.push(Span::raw("已保存"));
                        }
                        spans.push(Span::raw(" │ "));
                    }
                    let total: u64 = ws
                        .book
                        .volumes
                        .iter()
                        .flat_map(|v| &v.chapters)
                        .filter_map(|c| c.word_count)
                        .sum();
                    spans.push(Span::raw(format!("全书 {} ", format_words(total))));
                    spans.push(Span::raw("│ Ctrl+S 保存 │ Tab 切焦点 │ Esc 返回 "));

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
