//! 角色速查侧栏 / 列表页（Alt+C，§6.7）。
//!
//! 一个面板兼两用：编辑器旁的速查侧栏，和角色列表页——都是「列表 + 搜索 + 详情」。
//! `[MUST]` 不离开正文就能翻角色卡、能在侧栏内搜索。
//!
//! 状态与渲染分离：这里只管「有哪些角色、搜什么、选中谁」，绘制在 app.rs。
//! 增删改经 Store 落盘后，由 app 重新载入本面板。

use mj_core::model::Character;

pub struct CharacterPanel {
    all: Vec<Character>,
    /// 搜索串（对名字 + 别名做包含匹配）。
    query: String,
    /// 搜索框是否有焦点（`/` 进入，Esc/Enter 退出）。
    searching: bool,
    cursor: usize,
    scroll: usize,
    height: usize,
}

impl CharacterPanel {
    pub fn new(all: Vec<Character>) -> Self {
        Self {
            all,
            query: String::new(),
            searching: false,
            cursor: 0,
            scroll: 0,
            height: 10,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// 命中搜索的角色（按名字已在载入时排好序）。
    pub fn filtered(&self) -> Vec<&Character> {
        if self.query.is_empty() {
            return self.all.iter().collect();
        }
        let q = self.query.as_str();
        self.all
            .iter()
            .filter(|c| c.name.contains(q) || c.aliases.iter().any(|a| a.contains(q)))
            .collect()
    }

    pub fn filtered_count(&self) -> usize {
        self.filtered().len()
    }

    /// 当前选中的角色。
    pub fn current(&self) -> Option<&Character> {
        self.filtered().into_iter().nth(self.cursor)
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
        self.follow_cursor();
    }

    pub fn move_down(&mut self) {
        let n = self.filtered_count();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
        self.follow_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.follow_cursor();
    }

    /// `/` 进入搜索。
    pub fn start_search(&mut self) {
        self.searching = true;
    }

    /// 结束搜索输入（列表焦点回到结果）。
    pub fn end_search(&mut self) {
        self.searching = false;
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

    fn follow_cursor(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
    }

    /// 一行摘要，供列表显示：`沈砚（沈公子/小砚）· 主角`。
    pub fn summary_line(c: &Character) -> String {
        let mut s = c.name.clone();
        if !c.aliases.is_empty() {
            s.push_str(&format!("（{}）", c.aliases.join("/")));
        }
        if !c.role.is_empty() {
            s.push_str(&format!(" · {}", c.role));
        }
        s
    }

    /// 选中角色的详情行（右侧/下方展示）。空字段跳过，别撑出一堆空标签。
    pub fn detail_lines(c: &Character) -> Vec<String> {
        let mut v = vec![c.name.clone()];
        if !c.aliases.is_empty() {
            v.push(format!("别名：{}", c.aliases.join("、")));
        }
        let mut meta = Vec::new();
        for (label, val) in [("身份", &c.role), ("性别", &c.gender), ("年龄", &c.age)] {
            if !val.is_empty() {
                meta.push(format!("{label}：{val}"));
            }
        }
        if !meta.is_empty() {
            v.push(meta.join("  "));
        }
        for (label, val) in [
            ("背景", &c.background),
            ("性格", &c.personality),
            ("外貌", &c.appearance),
            ("习惯", &c.habits),
            ("语言", &c.speech),
            ("备注", &c.notes),
        ] {
            if !val.is_empty() {
                v.push(String::new());
                v.push(format!("【{label}】"));
                v.extend(val.lines().map(|l| l.to_string()));
            }
        }
        if !c.relations.is_empty() {
            v.push(String::new());
            v.push("【关系】".into());
            for r in &c.relations {
                v.push(format!("· {}", r.label));
            }
        }
        for (k, val) in &c.custom {
            if let Some(s) = val.as_str() {
                v.push(format!("{k}：{s}"));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_core::id::CharacterId;

    fn ch(name: &str, aliases: &[&str], role: &str) -> Character {
        let mut c = Character::new(CharacterId::generate(), name);
        c.aliases = aliases.iter().map(|s| s.to_string()).collect();
        c.role = role.into();
        c
    }

    fn panel() -> CharacterPanel {
        CharacterPanel::new(vec![
            ch("沈砚", &["沈公子", "小砚"], "主角"),
            ch("苏妲己", &[], "反派"),
            ch("周暮", &["老周"], "配角"),
        ])
    }

    #[test]
    fn search_matches_name_and_alias() {
        let mut p = panel();
        p.input_char('小');
        p.input_char('砚');
        assert_eq!(p.filtered_count(), 1, "别名「小砚」应命中");
        assert_eq!(p.current().unwrap().name, "沈砚");
    }

    #[test]
    fn search_by_name_substring() {
        let mut p = panel();
        p.input_char('周');
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.current().unwrap().name, "周暮");
    }

    #[test]
    fn empty_query_shows_all() {
        let p = panel();
        assert_eq!(p.filtered_count(), 3);
    }

    #[test]
    fn cursor_resets_on_new_query() {
        let mut p = panel();
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), 2);
        p.input_char('沈');
        assert_eq!(p.cursor(), 0, "改搜索串后光标回顶");
    }

    #[test]
    fn navigation_clamps_to_filtered() {
        let mut p = panel();
        p.input_char('沈'); // 只剩 1 个
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), 0, "只有 1 条，光标不越界");
    }

    #[test]
    fn summary_shows_aliases_and_role() {
        let c = ch("沈砚", &["沈公子", "小砚"], "主角");
        let s = CharacterPanel::summary_line(&c);
        assert!(
            s.contains("沈砚") && s.contains("小砚") && s.contains("主角"),
            "{s}"
        );
    }

    #[test]
    fn detail_skips_empty_fields() {
        let mut c = ch("沈砚", &[], "");
        c.background = "书香门第".into();
        let lines = CharacterPanel::detail_lines(&c);
        let text = lines.join("\n");
        assert!(text.contains("背景"), "有背景应显示");
        assert!(!text.contains("性别"), "空字段不该出现标签：{text}");
    }

    #[test]
    fn empty_panel_is_safe() {
        let mut p = CharacterPanel::new(vec![]);
        assert!(p.is_empty());
        assert!(p.current().is_none());
        p.move_down();
        assert_eq!(p.cursor(), 0);
    }
}
