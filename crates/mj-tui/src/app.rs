//! 应用状态机与运行入口。见 doc.md §7。
//!
//! M0 只做到「能开能关，崩溃不留残端」：起窗、事件循环、退出恢复。
//! 书架/目录树/编辑器是 M1。

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::event::{AppEvent, EventLoop};

pub struct App {
    should_quit: bool,
    /// 仅在状态变化时重绘（doc.md §7.4：不要固定 60fps 空转）。
    dirty: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            dirty: true,
        }
    }

    /// 主循环。返回即表示已请求退出；终端恢复由调用方负责（见 `run`）。
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
                    self.on_key(k.code, k.modifiers);
                }
                AppEvent::Term(Event::Resize(_, _)) => self.dirty = true,
                AppEvent::Term(_) => {}
                AppEvent::Tick => {}
            }
        }
        Ok(())
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        match code {
            // M0 临时退出键。M1 起 Esc 归「弹出浮层 / 回书架」，退出走命令面板。
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.should_quit = true,
            _ => {}
        }
        self.dirty = true;
    }

    /// 供渲染快照测试调用（doc.md §10）。
    ///
    /// 单开一个入口而非把 `render` 提为 pub：渲染是内部细节，
    /// 不应成为 crate 的公开契约。
    #[doc(hidden)]
    pub fn render_for_test(&self, frame: &mut ratatui::Frame) {
        self.render(frame);
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        let [body, status] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

        let placeholder = Text::from(vec![
            Line::from(""),
            Line::from("墨简 · mojian").bold().centered(),
            Line::from(""),
            Line::from("M0 骨架：能开能关，崩溃不留残端。").centered(),
            Line::from("书架与编辑器见 M1。").centered(),
        ]);

        frame.render_widget(
            Paragraph::new(placeholder)
                .block(Block::default().borders(Borders::ALL).title(" 墨简 "))
                .alignment(Alignment::Center),
            body,
        );

        frame.render_widget(
            Paragraph::new(" q / Esc 退出 ").style(Style::default().reversed()),
            status,
        );
    }
}

/// 起窗 → 跑循环 → 恢复终端。
///
/// 恢复不依赖循环正常返回：`run_loop` 出错时也要先恢复再传播错误，
/// 否则用户会拿到一个卡在 alternate screen 里的终端（doc.md §6.10）。
pub fn run() -> anyhow::Result<()> {
    let mut term = ratatui::try_init()?;
    let events = EventLoop::spawn();

    let result = App::new().run_loop(&mut term, &events);

    crate::font::emit_reset_sequence();
    ratatui::try_restore()?;
    result
}
