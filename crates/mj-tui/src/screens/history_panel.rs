//! 历史面板（F8）与 diff 视图。见 doc.md §6.9、§12.4。
//!
//! 状态与渲染分离：这里管「有哪些快照、选了哪两条、光标在哪个改动块」，
//! 绘制在 app.rs。

use mj_core::diff::{DiffHunk, DiffSummary, diff, summarize};
use mj_core::history::Snapshot;

/// 历史面板。
#[derive(Debug)]
pub struct HistoryPanel {
    snapshots: Vec<Snapshot>,
    cursor: usize,
    scroll: usize,
    height: usize,
    /// `Space` 选中的第二条，用于两条快照互比（§6.9）。
    compare_with: Option<usize>,
}

impl HistoryPanel {
    /// `snapshots` 按时间升序（History::list 的约定）。这里倒过来显示：
    /// 最近的在最上面——用户找的九成是刚才那版。
    pub fn new(mut snapshots: Vec<Snapshot>) -> Self {
        snapshots.reverse();
        Self {
            snapshots,
            cursor: 0,
            scroll: 0,
            height: 10,
            compare_with: None,
        }
    }

    pub fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn compare_with(&self) -> Option<usize> {
        self.compare_with
    }

    pub fn selected(&self) -> Option<&Snapshot> {
        self.snapshots.get(self.cursor)
    }

    /// 被 `Space` 选为对照的那条。
    pub fn compare_target(&self) -> Option<&Snapshot> {
        self.compare_with.and_then(|i| self.snapshots.get(i))
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

    pub fn move_down(&mut self) {
        if !self.snapshots.is_empty() {
            self.cursor = (self.cursor + 1).min(self.snapshots.len() - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    /// `Space`：把当前条选为对照。再按一次取消。
    pub fn toggle_compare(&mut self) {
        if self.compare_with == Some(self.cursor) {
            self.compare_with = None;
        } else {
            self.compare_with = Some(self.cursor);
        }
    }

    /// 渲染成一行。
    pub fn render_row(&self, i: usize) -> String {
        let Some(s) = self.snapshots.get(i) else {
            return String::new();
        };
        let mark = if self.compare_with == Some(i) {
            "◆"
        } else {
            " "
        };
        let pin = if s.pinned { "📌" } else { "  " };
        let label = s
            .label
            .as_deref()
            .map(|l| format!(" · {l}"))
            .unwrap_or_default();
        format!(
            "{mark}{pin} {} [{}]{label}  {} 字",
            s.created.format("%m-%d %H:%M"),
            s.trigger.label(),
            s.words
        )
    }
}

/// diff 视图（§6.9、§12.4）。
#[derive(Debug)]
pub struct DiffView {
    /// 左侧（旧）的标题，如「2026-07-14 22:10 · 投稿版」。
    pub old_title: String,
    /// 右侧（新）的标题。默认是「当前版本」。
    pub new_title: String,
    old_text: String,
    new_text: String,
    hunks: Vec<DiffHunk>,
    summary: DiffSummary,
    /// 当前高亮的改动块（`n`/`p` 跳转）。
    hunk_cursor: usize,
    scroll: usize,
    height: usize,
}

impl DiffView {
    pub fn new(old_title: String, old_text: String, new_title: String, new_text: String) -> Self {
        let hunks = diff(&old_text, &new_text);
        let summary = summarize(&hunks, &old_text, &new_text);
        Self {
            old_title,
            new_title,
            old_text,
            new_text,
            hunks,
            summary,
            hunk_cursor: 0,
            scroll: 0,
            height: 20,
        }
    }

    pub fn hunks(&self) -> &[DiffHunk] {
        &self.hunks
    }

    pub fn summary(&self) -> DiffSummary {
        self.summary
    }

    pub fn hunk_cursor(&self) -> usize {
        self.hunk_cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn old_text(&self) -> &str {
        &self.old_text
    }

    pub fn is_identical(&self) -> bool {
        self.hunks.is_empty()
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
    }

    /// 顶部摘要（§6.9：`+312 字 / -87 字 / 3 处改动`）。
    pub fn summary_line(&self) -> String {
        if self.hunks.is_empty() {
            return "两版内容相同".to_string();
        }
        format!(
            "+{} 字 / -{} 字 / {} 处改动",
            self.summary.added, self.summary.removed, self.summary.hunks
        )
    }

    /// `n`：跳到下一个改动块。
    pub fn next_hunk(&mut self) {
        if !self.hunks.is_empty() {
            // 到底了就回到第一个——改动块通常不多，回绕比卡住更顺手。
            self.hunk_cursor = (self.hunk_cursor + 1) % self.hunks.len();
            self.scroll_to_hunk();
        }
    }

    /// `p`：跳到上一个改动块。
    pub fn prev_hunk(&mut self) {
        if !self.hunks.is_empty() {
            self.hunk_cursor = if self.hunk_cursor == 0 {
                self.hunks.len() - 1
            } else {
                self.hunk_cursor - 1
            };
            self.scroll_to_hunk();
        }
    }

    fn scroll_to_hunk(&mut self) {
        let Some(h) = self.hunks.get(self.hunk_cursor) else {
            return;
        };
        // 让改动块出现在视口靠上的位置，留出后文的上下文。
        self.scroll = h.new_lines.start.saturating_sub(2);
    }

    pub fn scroll_down(&mut self) {
        let max = self.new_text.lines().count().saturating_sub(1);
        self.scroll = (self.scroll + 1).min(max);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// 当前改动块。
    pub fn current_hunk(&self) -> Option<&DiffHunk> {
        self.hunks.get(self.hunk_cursor)
    }

    /// `u`：单块恢复——把当前块的旧内容贴回当前版本（§6.9 恢复粒度 2）。
    ///
    /// 返回 (新版中要被替换的字节区间, 旧内容)。
    pub fn restore_hunk_edit(&self) -> Option<(std::ops::Range<usize>, String)> {
        let h = self.current_hunk()?;
        let old = self.old_text.get(h.old_range.clone())?.to_string();
        Some((h.new_range.clone(), old))
    }

    /// `y`：复制旧内容（§6.9 恢复粒度 3）。
    ///
    /// 复制的是**当前块**的旧内容；没有改动块时复制整份旧版。
    pub fn copy_text(&self) -> String {
        match self.current_hunk() {
            Some(h) => self
                .old_text
                .get(h.old_range.clone())
                .unwrap_or_default()
                .to_string(),
            None => self.old_text.clone(),
        }
    }

    /// 是否用左右分栏（§6.9：宽度 ≥ 100 列时分栏，否则 inline）。
    pub fn use_side_by_side(width: u16) -> bool {
        width >= 100
    }

    /// inline 视图的行。
    pub fn inline_lines(&self) -> Vec<DiffLine> {
        let old_lines: Vec<&str> = self.old_text.lines().collect();
        let new_lines: Vec<&str> = self.new_text.lines().collect();
        let mut out = Vec::new();

        // 只需跟着**新版**的行走：没变的行按新版显示，改动块处插入删/增两组。
        // 旧版的行号从 hunk 里直接取，不必另设游标。
        let mut new_i = 0usize;

        for (hi, h) in self.hunks.iter().enumerate() {
            // 改动块之前没变的行。
            while new_i < h.new_lines.start {
                out.push(DiffLine {
                    kind: LineKind::Equal,
                    line_no: new_i + 1,
                    text: new_lines.get(new_i).copied().unwrap_or("").to_string(),
                    hunk: None,
                });
                new_i += 1;
            }
            // 删除的行。
            for i in h.old_lines.clone() {
                out.push(DiffLine {
                    kind: LineKind::Delete,
                    line_no: i + 1,
                    text: old_lines.get(i).copied().unwrap_or("").to_string(),
                    hunk: Some(hi),
                });
            }
            // 新增的行。
            for i in h.new_lines.clone() {
                out.push(DiffLine {
                    kind: LineKind::Insert,
                    line_no: i + 1,
                    text: new_lines.get(i).copied().unwrap_or("").to_string(),
                    hunk: Some(hi),
                });
            }
            new_i = h.new_lines.end;
        }
        // 末尾没变的行。
        while new_i < new_lines.len() {
            out.push(DiffLine {
                kind: LineKind::Equal,
                line_no: new_i + 1,
                text: new_lines[new_i].to_string(),
                hunk: None,
            });
            new_i += 1;
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub line_no: usize,
    pub text: String,
    /// 属于第几个改动块（供高亮当前块）。
    pub hunk: Option<usize>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::{Duration, Local};
    use mj_core::history::{SnapshotId, Trigger};
    use mj_core::id::ChapterId;

    fn snap(label: Option<&str>, ago_min: i64, words: u64) -> Snapshot {
        Snapshot {
            id: SnapshotId::of(&format!("{label:?}{ago_min}")),
            chapter: ChapterId::generate(),
            created: Local::now() - Duration::minutes(ago_min),
            trigger: Trigger::Auto,
            pinned: label.is_some(),
            label: label.map(String::from),
            words,
            parent: None,
            extra: serde_json::Map::new(),
        }
    }

    // ---- 历史面板 ----

    /// 最近的排最上面——用户找的九成是刚才那版。
    #[test]
    fn newest_snapshot_is_listed_first() {
        // 传入按时间升序（History::list 的约定）。
        let p = HistoryPanel::new(vec![snap(None, 60, 100), snap(None, 1, 300)]);
        assert_eq!(p.snapshots()[0].words, 300, "最近的该在最上面");
    }

    #[test]
    fn empty_history_is_safe() {
        let mut p = HistoryPanel::new(Vec::new());
        assert!(p.is_empty());
        p.move_down();
        p.move_up();
        p.toggle_compare();
        assert!(p.selected().is_none());
    }

    #[test]
    fn cursor_stops_at_ends() {
        let mut p = HistoryPanel::new(vec![snap(None, 2, 1), snap(None, 1, 2)]);
        p.move_up();
        assert_eq!(p.cursor(), 0);
        for _ in 0..5 {
            p.move_down();
        }
        assert_eq!(p.cursor(), 1);
    }

    #[test]
    fn space_picks_and_unpicks_compare_target() {
        let mut p = HistoryPanel::new(vec![snap(None, 2, 1), snap(None, 1, 2)]);
        p.toggle_compare();
        assert_eq!(p.compare_with(), Some(0));
        assert!(p.compare_target().is_some());
        p.toggle_compare();
        assert_eq!(p.compare_with(), None, "再按应取消");
    }

    #[test]
    fn row_shows_label_and_pin() {
        let p = HistoryPanel::new(vec![snap(Some("投稿版"), 1, 3128)]);
        let row = p.render_row(0);
        assert!(row.contains("投稿版"), "{row}");
        assert!(row.contains("📌"), "钉住的应有标记: {row}");
        assert!(row.contains("3128"), "{row}");
        assert!(row.contains("自动"), "应显示触发来源: {row}");
    }

    #[test]
    fn compare_target_is_marked() {
        let mut p = HistoryPanel::new(vec![snap(None, 1, 1)]);
        p.toggle_compare();
        assert!(p.render_row(0).starts_with('◆'), "{}", p.render_row(0));
    }

    // ---- diff 视图 ----

    fn view(old: &str, new: &str) -> DiffView {
        DiffView::new("旧版".into(), old.into(), "当前版本".into(), new.into())
    }

    #[test]
    fn identical_texts_report_no_changes() {
        let v = view("雪落了", "雪落了");
        assert!(v.is_identical());
        assert_eq!(v.summary_line(), "两版内容相同");
    }

    /// §6.9 顶部摘要：`+312 字 / -87 字 / 3 处改动`。
    #[test]
    fn summary_line_matches_spec_format() {
        let v = view("雪落了。", "雪落了一夜。");
        let s = v.summary_line();
        assert!(s.contains("+6 字"), "{s}");
        assert!(s.contains("-4 字"), "{s}");
        assert!(s.contains("1 处改动"), "{s}");
    }

    #[test]
    fn n_and_p_cycle_through_hunks() {
        let v = &mut view("A\n同\nB\n同\nC", "X\n同\nY\n同\nZ");
        assert_eq!(v.hunks().len(), 3);
        assert_eq!(v.hunk_cursor(), 0);
        v.next_hunk();
        assert_eq!(v.hunk_cursor(), 1);
        v.next_hunk();
        v.next_hunk();
        assert_eq!(v.hunk_cursor(), 0, "到底应回绕");
        v.prev_hunk();
        assert_eq!(v.hunk_cursor(), 2, "往前也回绕");
    }

    #[test]
    fn hunk_navigation_on_identical_text_is_safe() {
        let v = &mut view("雪", "雪");
        v.next_hunk();
        v.prev_hunk();
        assert!(v.current_hunk().is_none());
    }

    /// §6.9 恢复粒度 2：单块恢复——把该块的旧内容贴回当前版本。
    #[test]
    fn restore_hunk_returns_old_content_for_the_block() {
        let old = "第一行\n旧的第二行\n第三行";
        let new = "第一行\n新的第二行\n第三行";
        let v = view(old, new);

        let (range, content) = v.restore_hunk_edit().unwrap();
        assert!(content.contains("旧的第二行"), "应是旧内容: {content:?}");
        // 把它贴回 new，应还原成 old。
        let mut result = new.to_string();
        result.replace_range(range, &content);
        assert_eq!(result, old, "单块恢复应精确还原");
    }

    /// 多个改动块时，单块恢复只该动当前那一块。
    #[test]
    fn restore_hunk_only_affects_the_current_block() {
        let old = "旧A\n同\n旧B";
        let new = "新A\n同\n新B";
        let v = view(old, new);
        assert_eq!(v.hunks().len(), 2);

        let (range, content) = v.restore_hunk_edit().unwrap();
        let mut result = new.to_string();
        result.replace_range(range, &content);
        assert_eq!(result, "旧A\n同\n新B", "只该恢复第一块");
    }

    /// §6.9 恢复粒度 3：复制旧内容。
    #[test]
    fn copy_text_returns_the_current_blocks_old_content() {
        let v = view("第一行\n旧的\n第三行", "第一行\n新的\n第三行");
        assert!(v.copy_text().contains("旧的"));
    }

    #[test]
    fn copy_text_falls_back_to_whole_old_version() {
        let v = view("整份旧版", "整份旧版");
        assert_eq!(v.copy_text(), "整份旧版", "无改动块时复制整份");
    }

    /// §6.9 布局：宽度 ≥ 100 列时左右分栏，否则 inline。
    #[test]
    fn layout_switches_at_100_columns() {
        assert!(DiffView::use_side_by_side(100));
        assert!(DiffView::use_side_by_side(120));
        assert!(!DiffView::use_side_by_side(99));
        assert!(!DiffView::use_side_by_side(80));
    }

    // ---- inline 视图 ----

    #[test]
    fn inline_lines_show_delete_then_insert() {
        let v = view("第一行\n旧的\n第三行", "第一行\n新的\n第三行");
        let lines = v.inline_lines();

        let kinds: Vec<LineKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Equal,
                LineKind::Delete,
                LineKind::Insert,
                LineKind::Equal
            ],
            "应是 等/删/增/等"
        );
        assert_eq!(lines[1].text, "旧的");
        assert_eq!(lines[2].text, "新的");
    }

    #[test]
    fn inline_lines_cover_all_new_lines() {
        let v = view("A\nB\nC", "A\nX\nC\nD");
        let lines = v.inline_lines();
        // 新版的每一行都该出现（作为 Equal 或 Insert）。
        for want in ["A", "X", "C", "D"] {
            assert!(
                lines
                    .iter()
                    .any(|l| l.text == want && l.kind != LineKind::Delete),
                "新版的 {want:?} 没出现在 inline 视图里"
            );
        }
    }

    #[test]
    fn inline_lines_of_identical_text_are_all_equal() {
        let v = view("A\nB", "A\nB");
        assert!(v.inline_lines().iter().all(|l| l.kind == LineKind::Equal));
    }

    #[test]
    fn inline_lines_tag_their_hunk() {
        let v = view("旧A\n同\n旧B", "新A\n同\n新B");
        let lines = v.inline_lines();
        let changed: Vec<Option<usize>> = lines
            .iter()
            .filter(|l| l.kind != LineKind::Equal)
            .map(|l| l.hunk)
            .collect();
        assert_eq!(changed, [Some(0), Some(0), Some(1), Some(1)]);
    }

    #[test]
    fn inline_handles_pure_insert() {
        let v = view("A", "A\nB");
        let lines = v.inline_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.kind == LineKind::Insert && l.text == "B")
        );
    }

    #[test]
    fn inline_handles_pure_delete() {
        let v = view("A\nB", "A");
        let lines = v.inline_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.kind == LineKind::Delete && l.text == "B")
        );
    }

    /// 跳到改动块时视口要跟过去，否则 n/p 按了没反应。
    #[test]
    fn jumping_to_a_hunk_scrolls_to_it() {
        let mut old = String::new();
        let mut new = String::new();
        for i in 0..50 {
            old.push_str(&format!("第{i}行\n"));
            new.push_str(&format!("第{i}行\n"));
        }
        old.push_str("旧结尾");
        new.push_str("新结尾");

        let mut v = view(&old, &new);
        v.next_hunk();
        assert!(
            v.scroll() > 40,
            "视口应跟到文末的改动块，实得 {}",
            v.scroll()
        );
    }
}
