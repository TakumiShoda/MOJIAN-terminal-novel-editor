//! 排版预览面板。见 doc.md §6.5 核心约束 2：
//! `[MUST]` 执行前弹出 diff 预览面板，显示将改动的位置与条数，**可逐条取消**。
//!
//! 为什么这条是验收项：排版会动用户的正文。不给看就动手，等于让用户
//! 拿自己的稿子赌我们的规则没写错——他赌不起，于是就再也不按 F5 了。
//!
//! 状态与渲染分离：这里只管「有哪些改动、勾了哪些」，绘制在 app.rs。

use mj_text::format::Edit;

/// 预览里的一条改动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewItem {
    pub edit: Edit,
    /// 该改动在正文中的行号（1 起），用于告诉用户「改在哪」。
    pub line: usize,
    /// 原文（被替换掉的部分）。
    pub old: String,
    /// 是否应用。默认全选——用户按 F5 的本意就是「都排」。
    pub include: bool,
}

/// 排版预览面板的状态。
#[derive(Debug)]
pub struct FormatPreview {
    items: Vec<PreviewItem>,
    cursor: usize,
    scroll: usize,
    /// 可见行数。渲染时由真实尺寸校正。
    height: usize,
}

impl FormatPreview {
    /// 由编辑计划与原文构造。
    pub fn new(text: &str, edits: Vec<Edit>) -> Self {
        // 预先算好每个字节偏移对应的行号：逐条去数会退化成平方级，
        // 而一章可能有上千处改动。
        let items = edits
            .into_iter()
            .map(|e| {
                let line = text
                    .get(..e.range.start)
                    .map(|head| head.matches('\n').count() + 1)
                    .unwrap_or(1);
                let old = text.get(e.range.clone()).unwrap_or_default().to_string();
                PreviewItem {
                    edit: e,
                    line,
                    old,
                    include: true,
                }
            })
            .collect();

        Self {
            items,
            cursor: 0,
            scroll: 0,
            height: 10,
        }
    }

    pub fn items(&self) -> &[PreviewItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// 勾选的条数。
    pub fn included_count(&self) -> usize {
        self.items.iter().filter(|i| i.include).count()
    }

    pub fn move_down(&mut self) {
        if !self.items.is_empty() {
            self.cursor = (self.cursor + 1).min(self.items.len() - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    /// 视口跟着高亮走。
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

    /// 逐条取消：勾/取消当前条（§6.5 [MUST]）。
    pub fn toggle(&mut self) {
        if let Some(i) = self.items.get_mut(self.cursor) {
            i.include = !i.include;
        }
    }

    /// 全选 / 全不选。
    pub fn set_all(&mut self, include: bool) {
        for i in &mut self.items {
            i.include = include;
        }
    }

    /// 勾选的改动，转成 `Buffer::replace_ranges` 要的形式。
    pub fn selected_edits(&self) -> Vec<(std::ops::Range<usize>, String)> {
        self.items
            .iter()
            .filter(|i| i.include)
            .map(|i| (i.edit.range.clone(), i.edit.new.clone()))
            .collect()
    }
}

/// 把一条改动渲染成一行文本。
///
/// 空白不可见，得显形——否则「删除行尾空白」那条会显示成 `"" → ""`，
/// 用户完全看不出改了什么。
pub fn render_item(item: &PreviewItem) -> String {
    let mark = if item.include { "✓" } else { " " };
    format!(
        "{mark} 第{}行 [{}] {} → {}",
        item.line,
        item.edit.rule,
        visible(&item.old),
        visible(&item.edit.new)
    )
}

/// 让空白与换行在预览里看得见。
pub fn visible(s: &str) -> String {
    if s.is_empty() {
        return "（空）".to_string();
    }
    let shown: String = s
        .chars()
        .map(|c| match c {
            '\n' => '⏎',
            '\t' => '⇥',
            ' ' => '·',
            '\u{3000}' => '□', // 全角空格
            other => other,
        })
        .collect();
    format!("「{shown}」")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_text::format::{FormatOptions, plan};

    fn preview(text: &str) -> FormatPreview {
        FormatPreview::new(text, plan(text, &FormatOptions::default()))
    }

    #[test]
    fn builds_items_from_plan() {
        let p = preview("雪落了一夜...");
        assert!(!p.is_empty());
        assert!(p.items().iter().all(|i| i.include), "默认应全选");
        assert_eq!(p.included_count(), p.len());
    }

    #[test]
    fn already_formatted_text_has_no_items() {
        let p = preview("　　雪落了一夜。\n");
        assert!(p.is_empty(), "已排好的文本不该有改动");
    }

    /// 行号要对——用户靠它找到改动的位置。
    #[test]
    fn reports_line_numbers() {
        let text = "第一行\n第二行...\n";
        let p = preview(text);
        let ellipsis = p
            .items()
            .iter()
            .find(|i| i.edit.new.contains('…'))
            .expect("应有省略号改动");
        assert_eq!(ellipsis.line, 2, "省略号在第二行");
    }

    #[test]
    fn toggle_excludes_single_item() {
        let mut p = preview("雪落了一夜...");
        let n = p.len();
        p.toggle();
        assert_eq!(p.included_count(), n - 1, "应少一条");
        p.toggle();
        assert_eq!(p.included_count(), n, "再按应恢复");
    }

    #[test]
    fn set_all_selects_and_clears() {
        let mut p = preview("雪落了一夜...");
        p.set_all(false);
        assert_eq!(p.included_count(), 0);
        assert!(p.selected_edits().is_empty(), "全不选时不该有改动");
        p.set_all(true);
        assert_eq!(p.included_count(), p.len());
    }

    /// 取消掉的条目不该出现在最终改动里——这是「逐条取消」的全部意义。
    #[test]
    fn selected_edits_respects_exclusions() {
        let mut p = preview("雪落了一夜...");
        assert!(p.len() >= 2, "这段文本应有多处改动");
        p.toggle(); // 取消第一条
        let edits = p.selected_edits();
        assert_eq!(edits.len(), p.len() - 1);
        assert!(
            !edits.iter().any(|(r, _)| *r == p.items()[0].edit.range),
            "被取消的那条不该出现"
        );
    }

    #[test]
    fn cursor_stops_at_ends() {
        let mut p = preview("雪落了一夜...");
        p.move_up();
        assert_eq!(p.cursor(), 0, "顶部再上移仍是 0");
        for _ in 0..50 {
            p.move_down();
        }
        assert_eq!(p.cursor(), p.len() - 1, "底部再下移仍是末条");
    }

    #[test]
    fn empty_preview_navigation_is_safe() {
        let mut p = preview("　　雪落了一夜。\n");
        p.move_down();
        p.move_up();
        p.toggle();
        assert!(p.selected_edits().is_empty());
    }

    /// 视口跟着高亮走，否则长列表里选中项会跑到屏幕外。
    #[test]
    fn scroll_follows_cursor() {
        let mut p = preview("雪...落...了...一...夜...他...推...门...风...雪...");
        p.set_height(3);
        for _ in 0..8 {
            p.move_down();
        }
        assert!(
            p.cursor() >= p.scroll() && p.cursor() < p.scroll() + 3,
            "高亮 {} 跑出了视口 {}..{}",
            p.cursor(),
            p.scroll(),
            p.scroll() + 3
        );
    }

    // ---- 显形 ----

    #[test]
    fn whitespace_is_made_visible() {
        assert_eq!(visible(" "), "「·」");
        assert_eq!(visible("\n"), "「⏎」");
        assert_eq!(visible("\t"), "「⇥」");
        assert_eq!(visible("\u{3000}"), "「□」");
    }

    /// 删除类改动的新文本是空的——必须说「空」，不能显示成一片虚无。
    #[test]
    fn empty_string_is_labeled() {
        assert_eq!(visible(""), "（空）");
    }

    #[test]
    fn render_item_shows_rule_and_change() {
        let p = preview("雪落了一夜...");
        let line = render_item(&p.items()[0]);
        assert!(line.starts_with('✓'), "默认勾选: {line}");
        assert!(line.contains("第"), "应有行号: {line}");
        assert!(line.contains('→'), "应有前后对照: {line}");
    }

    #[test]
    fn render_item_shows_unchecked_state() {
        let mut p = preview("雪落了一夜...");
        p.toggle();
        assert!(
            !render_item(&p.items()[0]).starts_with('✓'),
            "取消后不该带勾"
        );
    }
}
