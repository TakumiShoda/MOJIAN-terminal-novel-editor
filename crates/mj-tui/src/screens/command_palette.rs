//! 命令面板（`Ctrl+P`）。见 doc.md §7.3。
//!
//! §7.3 把这条标成「最重要的一条」：所有功能都必须能从这里触达。候选来自
//! `commands::COMMANDS` 那张唯一的命令表——面板不自己维护一份列表，
//! 否则迟早与实际能做的事分叉。
//!
//! 状态与渲染分离：这里只管「敲了什么、筛出哪些、选中第几个」，绘制在 app.rs。

use crate::commands::{COMMANDS, Command, CommandSpec};

pub struct CommandPalette {
    query: String,
    cursor: usize,
    scroll: usize,
    height: usize,
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            scroll: 0,
            height: 10,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// 当前筛出的候选。
    pub fn matches(&self) -> Vec<&'static CommandSpec> {
        COMMANDS.iter().filter(|c| c.matches(&self.query)).collect()
    }

    pub fn match_count(&self) -> usize {
        self.matches().len()
    }

    /// 选中的命令。没有候选时为 None。
    pub fn selected(&self) -> Option<Command> {
        self.matches().get(self.cursor).map(|c| c.cmd)
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
        self.follow_cursor();
    }

    pub fn input_char(&mut self, c: char) {
        self.query.push(c);
        self.reset_cursor();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.reset_cursor();
    }

    fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
    }

    pub fn move_down(&mut self) {
        let n = self.match_count();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    fn follow_cursor(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
    }

    /// 一行候选：`校对当前章   查错别字、标点…        F7`。
    pub fn row(c: &CommandSpec) -> (String, String, String) {
        (c.name.to_string(), c.desc.to_string(), c.keys.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn opens_listing_everything() {
        let p = CommandPalette::new();
        assert_eq!(p.match_count(), COMMANDS.len(), "空查询列出全部命令");
        assert!(p.selected().is_some());
    }

    #[test]
    fn filters_by_name() {
        let mut p = CommandPalette::new();
        for c in "校对".chars() {
            p.input_char(c);
        }
        // 不写死条数：往命令表里加一条带「校对」的命令就会把它撞坏，
        // 而这个用例要验的是「筛选起作用」，不是「校对恰好只有一条」。
        assert!(p.match_count() >= 1);
        assert!(p.match_count() < COMMANDS.len(), "该筛掉一部分");
        assert_eq!(p.selected(), Some(Command::Proof), "表里靠前的先出");
    }

    /// 敲键位也能找到命令。
    #[test]
    fn filters_by_keybinding() {
        let mut p = CommandPalette::new();
        for c in "F8".chars() {
            p.input_char(c);
        }
        assert_eq!(p.selected(), Some(Command::History));
    }

    #[test]
    fn backspace_widens_the_result() {
        let mut p = CommandPalette::new();
        // 「查找替换」只命中一条；退回「查找」则同时命中「查找」与「查找替换」。
        for c in "查找替换".chars() {
            p.input_char(c);
        }
        assert_eq!(p.match_count(), 1);
        p.backspace();
        p.backspace();
        assert_eq!(p.query(), "查找");
        assert!(p.match_count() > 1, "退格应放宽筛选");
    }

    #[test]
    fn cursor_resets_when_query_changes() {
        let mut p = CommandPalette::new();
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), 2);
        p.input_char('查');
        assert_eq!(p.cursor(), 0, "改查询后光标回顶");
    }

    #[test]
    fn cursor_clamps_to_matches() {
        let mut p = CommandPalette::new();
        for c in "校对".chars() {
            p.input_char(c);
        }
        let n = p.match_count();
        assert!(n > 0);
        for _ in 0..20 {
            p.move_down();
        }
        assert_eq!(p.cursor(), n - 1, "光标该停在最后一条，不越界");
    }

    #[test]
    fn no_match_selects_nothing() {
        let mut p = CommandPalette::new();
        for c in "查无此命令".chars() {
            p.input_char(c);
        }
        assert_eq!(p.match_count(), 0);
        assert!(p.selected().is_none(), "没有候选时不该选中任何东西");
        p.move_down(); // 不得 panic
    }

    /// 每条命令都能被它自己的名字搜到——否则命令面板就到不了它。
    #[test]
    fn every_command_is_reachable_by_its_own_name() {
        for spec in COMMANDS {
            let mut p = CommandPalette::new();
            for c in spec.name.chars() {
                p.input_char(c);
            }
            let hits = p.matches();
            assert!(
                hits.iter().any(|h| h.cmd == spec.cmd),
                "命令「{}」搜不到自己",
                spec.name
            );
        }
    }
}
