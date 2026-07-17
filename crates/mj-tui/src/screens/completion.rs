//! `@` 触发的角色名补全（§6.7 [SHOULD]）。
//!
//! 在正文里敲 `@` 弹出角色名列表，接着敲的字作过滤，Enter/Tab 上屏所选名字
//! （替换掉 `@` 与已敲的过滤串）。Esc 取消，留下字面文本。
//!
//! 状态只记「`@` 在哪、候选有哪些、选中第几个」；过滤串由 app 从缓冲里现取
//! （`@` 之后到光标之间那段），因为那段文字本就实时躺在缓冲里，不必存两份。

pub struct Completion {
    /// `@` 在缓冲里的字节偏移。
    at: usize,
    /// 全部候选（角色名 + 别名），已去重。
    names: Vec<String>,
    cursor: usize,
}

impl Completion {
    pub fn new(at: usize, mut names: Vec<String>) -> Self {
        names.retain(|n| !n.trim().is_empty());
        names.dedup();
        Self {
            at,
            names,
            cursor: 0,
        }
    }

    pub fn at(&self) -> usize {
        self.at
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 按过滤串筛候选：包含即命中；空串给全部。
    pub fn candidates(&self, filter: &str) -> Vec<&str> {
        if filter.is_empty() {
            return self.names.iter().map(|s| s.as_str()).collect();
        }
        self.names
            .iter()
            .filter(|n| n.contains(filter))
            .map(|s| s.as_str())
            .collect()
    }

    /// 当前选中的候选（依当前过滤串）。
    pub fn selected(&self, filter: &str) -> Option<String> {
        self.candidates(filter)
            .get(self.cursor)
            .map(|s| s.to_string())
    }

    pub fn move_down(&mut self, filter: &str) {
        let n = self.candidates(filter).len();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// 过滤串变化后把光标夹回有效范围（候选变少时）。
    pub fn clamp(&mut self, filter: &str) {
        let n = self.candidates(filter).len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    /// 没有候选就不必显示补全框。
    pub fn is_useful(&self) -> bool {
        !self.names.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp() -> Completion {
        Completion::new(
            5,
            vec![
                "沈砚".into(),
                "沈公子".into(),
                "苏妲己".into(),
                "周暮".into(),
            ],
        )
    }

    #[test]
    fn empty_filter_lists_all() {
        let c = comp();
        assert_eq!(c.candidates("").len(), 4);
    }

    #[test]
    fn filter_narrows_candidates() {
        let c = comp();
        let cands = c.candidates("沈");
        assert_eq!(cands, vec!["沈砚", "沈公子"]);
    }

    #[test]
    fn selected_follows_cursor() {
        let mut c = comp();
        assert_eq!(c.selected("沈").as_deref(), Some("沈砚"));
        c.move_down("沈");
        assert_eq!(c.selected("沈").as_deref(), Some("沈公子"));
    }

    #[test]
    fn cursor_clamps_when_filter_shrinks_list() {
        let mut c = comp();
        c.move_down(""); // cursor=1
        c.move_down(""); // cursor=2
        c.move_down(""); // cursor=3
        assert_eq!(c.cursor(), 3);
        // 过滤到只剩 2 个，光标要夹回。
        c.clamp("沈");
        assert_eq!(c.cursor(), 1);
        assert_eq!(c.selected("沈").as_deref(), Some("沈公子"));
    }

    #[test]
    fn dedup_and_blank_removed() {
        let c = Completion::new(0, vec!["沈砚".into(), "沈砚".into(), "  ".into()]);
        assert_eq!(c.candidates("").len(), 1);
    }

    #[test]
    fn move_down_clamps_on_empty_candidates() {
        let mut c = comp();
        c.move_down("查无此人"); // 无候选，不应 panic
        assert_eq!(c.cursor(), 0);
        assert!(c.selected("查无此人").is_none());
    }
}
