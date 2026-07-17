//! 查找替换面板。见 doc.md §6.6、§7.3（Ctrl+F / Ctrl+H）。
//!
//! 范围（F4）：当前章 / 当前卷 / 全书。
//!
//! 全书替换一度不敢给——§6.6 的 `[MUST]` 是「执行前强制打快照」+「撤销本次
//! 批量替换」，而快照直到 M4 才有。没有快照就让人替换整本书，等于拿全书赌一次
//! 正则没写错。M4 之后两条都兑现了，这才放开。
//!
//! 状态与渲染分离：这里只管「查什么、找到了什么、勾了哪些」，绘制在 app.rs。

use std::collections::HashSet;

use mj_text::search::{HitContext, MatchFlags, MatchMode, Query, hit_context, search_text};

/// 当前输入焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Query,
    Replace,
    /// 结果列表。
    Results,
}

pub struct SearchPanel {
    pub query: String,
    pub replace_with: String,
    pub mode: MatchMode,
    pub flags: MatchFlags,
    /// Ctrl+H 进来的（带替换栏）；Ctrl+F 只查找。
    pub replace_mode: bool,
    /// 作业范围（§6.6）。F4 切换。
    ///
    /// 结果列表只显示**当前章**的命中——跨章的结果树（§6.6 的「书→卷→章分组」）
    /// 要先把全书读进内存，那是 M6 索引搜索的活。这里 scope 只影响**替换**
    /// 落到哪些章：范围本身是诚实的，界面上写明「替换 N 章」。
    pub scope: crate::batch::Scope,
    field: Field,
    hits: Vec<HitContext>,
    checked: HashSet<usize>,
    cursor: usize,
    scroll: usize,
    height: usize,
    /// 非法正则的实时提示（§6.6 [MUST]：不得 panic）。
    error: Option<String>,
}

impl SearchPanel {
    pub fn new(replace_mode: bool) -> Self {
        Self {
            query: String::new(),
            replace_with: String::new(),
            mode: MatchMode::default(),
            flags: MatchFlags::default(),
            replace_mode,
            scope: crate::batch::Scope::Chapter,
            field: Field::Query,
            hits: Vec::new(),
            checked: HashSet::new(),
            cursor: 0,
            scroll: 0,
            height: 10,
            error: None,
        }
    }

    pub fn field(&self) -> Field {
        self.field
    }

    pub fn hits(&self) -> &[HitContext] {
        &self.hits
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn is_checked(&self, i: usize) -> bool {
        self.checked.contains(&i)
    }

    pub fn checked_count(&self) -> usize {
        self.checked.len()
    }

    /// 重新查找。每次改动输入或选项都要调——§6.6 要求非法正则**实时**提示。
    pub fn refresh(&mut self, text: &str) {
        self.hits.clear();
        self.checked.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.error = None;

        if self.query.is_empty() {
            return;
        }
        let q = Query {
            pattern: self.query.clone(),
            mode: self.mode,
            flags: self.flags,
        };
        match search_text(text, &q) {
            Ok(ranges) => {
                self.hits = ranges.into_iter().map(|r| hit_context(text, r)).collect();
                // 默认全选：用户按 Ctrl+H 的本意通常是「全换掉」。
                self.checked = (0..self.hits.len()).collect();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
        self.follow_cursor();
    }

    fn follow_cursor(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
    }

    /// Tab 切换输入焦点。
    pub fn next_field(&mut self) {
        self.field = match (self.field, self.replace_mode) {
            (Field::Query, true) => Field::Replace,
            (Field::Query, false) => Field::Results,
            (Field::Replace, _) => Field::Results,
            (Field::Results, _) => Field::Query,
        };
    }

    /// 往当前输入框敲一个字符。返回 true 表示需要重新查找。
    pub fn input_char(&mut self, c: char) -> bool {
        match self.field {
            Field::Query => {
                self.query.push(c);
                true
            }
            Field::Replace => {
                self.replace_with.push(c);
                false // 改替换文本不影响命中
            }
            Field::Results => false,
        }
    }

    /// 退格。返回 true 表示需要重新查找。
    pub fn backspace(&mut self) -> bool {
        match self.field {
            Field::Query => {
                self.query.pop();
                true
            }
            Field::Replace => {
                self.replace_with.pop();
                false
            }
            Field::Results => false,
        }
    }

    pub fn move_down(&mut self) {
        if !self.hits.is_empty() {
            self.cursor = (self.cursor + 1).min(self.hits.len() - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    /// Space 勾选当前条（§6.6）。
    pub fn toggle_check(&mut self) {
        if self.cursor < self.hits.len() && !self.checked.remove(&self.cursor) {
            self.checked.insert(self.cursor);
        }
    }

    /// 当前高亮命中的位置，供 Enter 跳转。
    pub fn current_hit(&self) -> Option<&HitContext> {
        self.hits.get(self.cursor)
    }

    /// `r`：只替换当前这一条。
    pub fn current_edit(&self) -> Option<(std::ops::Range<usize>, String)> {
        self.current_hit()
            .map(|h| (h.range.clone(), self.replace_with.clone()))
    }

    /// `A`：替换全部勾选（§6.6）。
    ///
    /// 结果按起点升序——`Buffer::replace_ranges` 要靠它把偏移校正对。
    pub fn checked_edits(&self) -> Vec<(std::ops::Range<usize>, String)> {
        let mut out: Vec<_> = self
            .hits
            .iter()
            .enumerate()
            .filter(|(i, _)| self.checked.contains(i))
            .map(|(_, h)| (h.range.clone(), self.replace_with.clone()))
            .collect();
        out.sort_by_key(|(r, _)| r.start);
        out
    }

    /// 面板标题栏的摘要。
    pub fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return format!("⚠ {e}");
        }
        if self.query.is_empty() {
            return "输入要查找的内容".to_string();
        }
        if self.hits.is_empty() {
            return "没有找到".to_string();
        }
        format!("{} 处命中，已选 {} 处", self.hits.len(), self.checked.len())
    }

    /// 选项的一行摘要，让用户知道当前开了什么。
    pub fn options_line(&self) -> String {
        let mode = match self.mode {
            MatchMode::Literal => "普通",
            MatchMode::WholeWord => "全词",
            MatchMode::Regex => {
                if self.flags.extended {
                    "正则+扩展"
                } else {
                    "正则"
                }
            }
        };
        let on = |b: bool| if b { "✓" } else { " " };
        format!(
            "范围:{}(F4) 模式:{mode}(F2) [{}]大小写(F6) [{}]全半角(F7) [{}]中文标点(F8)",
            self.scope.label(),
            on(self.flags.ignore_case),
            on(self.flags.fold_width),
            on(self.flags.fold_cjk_punct),
        )
    }

    /// F2 循环切换模式。
    pub fn cycle_mode(&mut self) {
        self.mode = match self.mode {
            MatchMode::Literal => MatchMode::WholeWord,
            MatchMode::WholeWord => MatchMode::Regex,
            MatchMode::Regex if !self.flags.extended => {
                self.flags.extended = true;
                MatchMode::Regex
            }
            MatchMode::Regex => {
                self.flags.extended = false;
                MatchMode::Literal
            }
        };
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn panel(query: &str, text: &str) -> SearchPanel {
        let mut p = SearchPanel::new(true);
        p.query = query.into();
        p.refresh(text);
        p
    }

    #[test]
    fn finds_and_selects_all_by_default() {
        let p = panel("雪", "雪落了。雪停了。");
        assert_eq!(p.hits().len(), 2);
        assert_eq!(p.checked_count(), 2, "默认全选");
        assert!(p.error().is_none());
    }

    #[test]
    fn empty_query_finds_nothing() {
        let p = panel("", "雪落了");
        assert!(p.hits().is_empty());
        assert!(p.error().is_none(), "空查找不算错误");
        assert_eq!(p.summary(), "输入要查找的内容");
    }

    #[test]
    fn no_match_says_so() {
        let p = panel("风", "雪落了");
        assert!(p.hits().is_empty());
        assert_eq!(p.summary(), "没有找到");
    }

    /// §6.6 [MUST]：非法正则实时提示，不得 panic。
    #[test]
    fn invalid_regex_shows_error_not_panic() {
        let mut p = SearchPanel::new(false);
        p.query = "[unclosed".into();
        p.mode = MatchMode::Regex;
        p.refresh("雪落了");

        assert!(p.error().is_some(), "应有错误提示");
        assert!(p.hits().is_empty());
        assert!(
            p.summary().starts_with('⚠'),
            "摘要应显示警告: {}",
            p.summary()
        );
    }

    /// 改正之后错误要消失——「实时」的意思。
    #[test]
    fn fixing_regex_clears_the_error() {
        let mut p = SearchPanel::new(false);
        p.query = "[unclosed".into();
        p.mode = MatchMode::Regex;
        p.refresh("雪落了");
        assert!(p.error().is_some());

        p.query = "雪".into();
        p.refresh("雪落了");
        assert!(p.error().is_none(), "改对之后错误应消失");
        assert_eq!(p.hits().len(), 1);
    }

    #[test]
    fn toggle_check_excludes_a_hit() {
        let mut p = panel("雪", "雪落。雪停。");
        p.toggle_check();
        assert_eq!(p.checked_count(), 1);
        assert_eq!(p.checked_edits().len(), 1, "取消的那条不该出现");
    }

    #[test]
    fn checked_edits_are_sorted_by_start() {
        let p = panel("雪", "雪落。雪停。雪化。");
        let edits = p.checked_edits();
        for w in edits.windows(2) {
            assert!(w[0].0.start < w[1].0.start, "必须按起点升序");
        }
    }

    #[test]
    fn current_edit_targets_the_highlighted_hit() {
        let mut p = panel("雪", "雪落。雪停。");
        p.replace_with = "风".into();
        p.move_down();
        let (range, to) = p.current_edit().unwrap();
        assert_eq!(range, p.hits()[1].range, "应是第二条");
        assert_eq!(to, "风");
    }

    #[test]
    fn cursor_stops_at_ends() {
        let mut p = panel("雪", "雪落。雪停。");
        p.move_up();
        assert_eq!(p.cursor(), 0);
        for _ in 0..10 {
            p.move_down();
        }
        assert_eq!(p.cursor(), 1);
    }

    #[test]
    fn navigation_on_empty_results_is_safe() {
        let mut p = panel("风", "雪落了");
        p.move_down();
        p.move_up();
        p.toggle_check();
        assert!(p.current_edit().is_none());
        assert!(p.checked_edits().is_empty());
    }

    // ---- 输入 ----

    #[test]
    fn typing_in_query_triggers_refresh() {
        let mut p = SearchPanel::new(true);
        assert!(p.input_char('雪'), "改查找串应触发重查");
        assert_eq!(p.query, "雪");
    }

    #[test]
    fn typing_in_replace_does_not_trigger_refresh() {
        let mut p = SearchPanel::new(true);
        p.next_field();
        assert_eq!(p.field(), Field::Replace);
        assert!(!p.input_char('风'), "改替换文本不影响命中");
        assert_eq!(p.replace_with, "风");
    }

    #[test]
    fn backspace_edits_the_focused_field() {
        let mut p = SearchPanel::new(true);
        p.query = "雪落".into();
        assert!(p.backspace());
        assert_eq!(p.query, "雪", "应按字符退格，不是字节");
    }

    /// 退格必须按字符——中文一字 3 字节，按字节退会留下半个字。
    #[test]
    fn backspace_removes_whole_cjk_char() {
        let mut p = SearchPanel::new(false);
        p.query = "雪".into();
        p.backspace();
        assert_eq!(p.query, "");
    }

    #[test]
    fn tab_cycles_fields_in_replace_mode() {
        let mut p = SearchPanel::new(true);
        assert_eq!(p.field(), Field::Query);
        p.next_field();
        assert_eq!(p.field(), Field::Replace);
        p.next_field();
        assert_eq!(p.field(), Field::Results);
        p.next_field();
        assert_eq!(p.field(), Field::Query);
    }

    /// 只查找模式没有替换栏，Tab 应跳过它。
    #[test]
    fn tab_skips_replace_field_in_find_mode() {
        let mut p = SearchPanel::new(false);
        p.next_field();
        assert_eq!(p.field(), Field::Results, "查找模式不该停在替换栏");
    }

    // ---- 选项 ----

    #[test]
    fn cycle_mode_goes_through_all_modes() {
        let mut p = SearchPanel::new(false);
        assert_eq!(p.mode, MatchMode::Literal);
        p.cycle_mode();
        assert_eq!(p.mode, MatchMode::WholeWord);
        p.cycle_mode();
        assert_eq!(p.mode, MatchMode::Regex);
        assert!(!p.flags.extended);
        p.cycle_mode();
        assert_eq!(p.mode, MatchMode::Regex);
        assert!(p.flags.extended, "再切一次开扩展语法");
        p.cycle_mode();
        assert_eq!(p.mode, MatchMode::Literal, "转回普通");
        assert!(!p.flags.extended);
    }

    #[test]
    fn options_line_reflects_flags() {
        let mut p = SearchPanel::new(false);
        assert!(p.options_line().contains("普通"));
        p.flags.fold_width = true;
        assert!(
            p.options_line().contains("[✓]全半角"),
            "{}",
            p.options_line()
        );
    }

    /// 折叠选项要真的生效。
    #[test]
    fn folding_affects_hits() {
        let mut p = SearchPanel::new(false);
        p.query = "A".into();
        p.refresh("Ａ");
        assert!(p.hits().is_empty(), "不折叠时全角 A 不该命中");

        p.flags.fold_width = true;
        p.refresh("Ａ");
        assert_eq!(p.hits().len(), 1, "开折叠后应命中");
    }

    #[test]
    fn hits_carry_context_and_line() {
        let p = panel("风", "第一行\n第二行有风。");
        assert_eq!(p.hits().len(), 1);
        assert_eq!(p.hits()[0].line, 2);
        assert!(p.hits()[0].context.contains('风'));
    }
}
