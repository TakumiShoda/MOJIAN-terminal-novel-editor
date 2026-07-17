//! 校对面板（F7）。见 doc.md §6.8。
//!
//! 状态与渲染分离：这里只管「有哪些问题、光标在哪、折叠没折叠」，绘制在 app.rs。
//! 校对本身（跑规则、算忽略键、落盘）在 mj-core；面板只拿到一份 `Issue` 列表。
//!
//! 按严重度分组（Error/Warning/Hint）。低置信问题（confidence < fold_below，
//! 主要是的地得）默认**折叠**，`f` 展开——§12.3 [MUST]：这类提示宁保守勿激进，
//! 别一上来就糊用户一脸。

use mj_text::proof::{Issue, Severity};

pub struct ProofPanel {
    /// 全部问题，已按位置排序。
    issues: Vec<Issue>,
    /// 低于此置信度默认折叠。
    fold_below: f32,
    /// 是否显示被折叠的低置信问题。
    show_folded: bool,
    /// 光标落在「可见问题」列表里的第几个（不含分组表头）。
    cursor: usize,
    scroll: usize,
    height: usize,
}

/// 面板里的一行：分组表头或一条问题。
pub enum Row<'a> {
    Header(Severity, usize),
    Issue {
        /// 在可见问题列表里的序号（供光标比对）。
        index: usize,
        issue: &'a Issue,
        selected: bool,
    },
}

impl ProofPanel {
    pub fn new(mut issues: Vec<Issue>, fold_below: f32) -> Self {
        issues.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then(a.range.start.cmp(&b.range.start))
        });
        Self {
            issues,
            fold_below,
            show_folded: false,
            cursor: 0,
            scroll: 0,
            height: 10,
        }
    }

    /// 一条问题是否因低置信被折叠。
    fn is_folded(&self, issue: &Issue) -> bool {
        !self.show_folded && issue.confidence < self.fold_below
    }

    fn folded_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.confidence < self.fold_below)
            .count()
    }

    /// 当前可见的问题（按当前折叠状态过滤），保持排序。
    pub fn visible(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| !self.is_folded(i)).collect()
    }

    pub fn total(&self) -> usize {
        self.issues.len()
    }

    pub fn visible_count(&self) -> usize {
        self.visible().len()
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn show_folded(&self) -> bool {
        self.show_folded
    }

    /// 当前高亮的问题。
    pub fn current(&self) -> Option<Issue> {
        self.visible().get(self.cursor).map(|i| (*i).clone())
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
        self.follow_cursor();
    }

    /// `f` 切换是否显示折叠的低置信项。有折叠项才有意义。
    pub fn toggle_folded(&mut self) {
        if self.folded_count() == 0 {
            return;
        }
        self.show_folded = !self.show_folded;
        self.cursor = self.cursor.min(self.visible_count().saturating_sub(1));
        self.follow_cursor();
    }

    /// 折叠了多少条、当前是否展开——供状态栏提示。
    pub fn fold_hint(&self) -> Option<(usize, bool)> {
        let n = self.folded_count();
        (n > 0).then_some((n, self.show_folded))
    }

    pub fn move_down(&mut self) {
        let n = self.visible_count();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    /// 从列表里摘掉当前项（忽略本次/永久忽略后调用）。返回被摘掉的那条。
    ///
    /// 摘的是**排序后**列表里的对应项——按可见序号定位，再从底层 issues 删掉。
    pub fn remove_current(&mut self) -> Option<Issue> {
        let target = self.current()?;
        // 底层可能有多条 range 相同但内容不同的问题；按 range+rule_id 精确定位。
        if let Some(pos) = self
            .issues
            .iter()
            .position(|i| i.range == target.range && i.rule_id == target.rule_id)
        {
            let removed = self.issues.remove(pos);
            let n = self.visible_count();
            if n == 0 {
                self.cursor = 0;
            } else {
                self.cursor = self.cursor.min(n - 1);
            }
            self.follow_cursor();
            return Some(removed);
        }
        None
    }

    fn follow_cursor(&mut self) {
        // 渲染时每个可见问题占一行、外加分组表头。为滚动简单起见，
        // 用「问题序号」的近似：光标问题前的表头数最多 = 严重度种类数(3)。
        // 直接以可见问题序号驱动滚动，表头行由渲染时插入、不影响选择逻辑。
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
    }

    /// 供渲染：产出「表头 + 问题」的行序列（已按当前折叠状态过滤、按严重度分组）。
    pub fn rows(&self) -> Vec<Row<'_>> {
        let visible = self.visible();
        let mut rows = Vec::new();
        let mut last: Option<Severity> = None;
        for (index, issue) in visible.iter().enumerate() {
            if last != Some(issue.severity) {
                let count = visible
                    .iter()
                    .filter(|i| i.severity == issue.severity)
                    .count();
                rows.push(Row::Header(issue.severity, count));
                last = Some(issue.severity);
            }
            rows.push(Row::Issue {
                index,
                issue,
                selected: index == self.cursor,
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_text::proof::{Category, Source};
    use std::ops::Range;

    fn issue(rule: &str, sev: Severity, conf: f32, range: Range<usize>) -> Issue {
        Issue {
            range,
            severity: sev,
            category: Category::Typo,
            rule_id: rule.into(),
            message: format!("问题 {rule}"),
            suggestions: vec!["建议".into()],
            source: Source::Rule,
            confidence: conf,
        }
    }

    fn sample() -> Vec<Issue> {
        vec![
            issue("typo.confusion", Severity::Warning, 0.9, 10..14),
            issue("punct.unpaired", Severity::Warning, 0.85, 2..3),
            issue("typo.de_di_de", Severity::Hint, 0.5, 20..21), // 折叠
            issue("style.long_sentence", Severity::Hint, 0.5, 0..30), // 折叠
        ]
    }

    #[test]
    fn low_confidence_folded_by_default() {
        let p = ProofPanel::new(sample(), 0.6);
        assert_eq!(p.total(), 4);
        assert_eq!(p.visible_count(), 2, "两条 Hint(<0.6) 默认折叠");
        assert_eq!(p.fold_hint(), Some((2, false)));
    }

    #[test]
    fn toggle_reveals_folded() {
        let mut p = ProofPanel::new(sample(), 0.6);
        p.toggle_folded();
        assert_eq!(p.visible_count(), 4);
        assert_eq!(p.fold_hint(), Some((2, true)));
    }

    #[test]
    fn sorted_by_severity_then_position() {
        let p = ProofPanel::new(sample(), 0.6);
        let v = p.visible();
        // 两条都是 Warning，按位置：2..3 在 10..14 前。
        assert_eq!(v[0].range.start, 2);
        assert_eq!(v[1].range.start, 10);
    }

    #[test]
    fn rows_have_group_headers() {
        let mut p = ProofPanel::new(sample(), 0.6);
        p.toggle_folded();
        let rows = p.rows();
        let headers: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Header(s, n) => Some((*s, *n)),
                _ => None,
            })
            .collect();
        assert_eq!(headers, vec![(Severity::Warning, 2), (Severity::Hint, 2)]);
    }

    #[test]
    fn navigation_clamps() {
        let mut p = ProofPanel::new(sample(), 0.6);
        p.move_up();
        assert_eq!(p.cursor(), 0, "顶部不越界");
        p.move_down();
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), 1, "只有 2 条可见，光标停在末条");
    }

    #[test]
    fn remove_current_drops_the_right_issue() {
        let mut p = ProofPanel::new(sample(), 0.6);
        let removed = p.remove_current().unwrap();
        assert_eq!(removed.range.start, 2, "摘掉当前高亮（首条 Warning）");
        assert_eq!(p.visible_count(), 1);
        assert_eq!(p.total(), 3);
    }

    #[test]
    fn empty_panel_is_safe() {
        let mut p = ProofPanel::new(vec![], 0.6);
        assert!(p.is_empty());
        assert!(p.current().is_none());
        assert!(p.remove_current().is_none());
        p.move_down();
        assert_eq!(p.cursor(), 0);
    }
}
