//! 角色速查侧栏 / 列表页（Alt+C，§6.7）。
//!
//! 一个面板兼两用：编辑器旁的速查侧栏，和角色列表页——都是「列表 + 搜索 + 详情」。
//! `[MUST]` 不离开正文就能翻角色卡、能在侧栏内搜索。
//!
//! 状态与渲染分离：这里只管「有哪些角色、搜什么、选中谁」，绘制在 app.rs。
//! 增删改经 Store 落盘后，由 app 重新载入本面板。

use mj_core::appearance::Appearance;
use mj_core::model::Character;

pub struct CharacterPanel {
    all: Vec<Character>,
    /// 搜索串（对名字 + 别名做包含匹配）。
    query: String,
    /// 搜索框是否有焦点（`/` 进入，Esc/Enter 退出）。
    searching: bool,
    /// 出场统计视图（`t` 打开，§6.7 [SHOULD]）。Some = 正在看统计。
    stats: Option<Vec<Appearance>>,
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
            stats: None,
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

    // ---- 出场统计（§6.7 [SHOULD]）----

    /// 是否正在看出场统计。
    pub fn show_stats(&self) -> bool {
        self.stats.is_some()
    }

    /// 载入统计并切到统计视图。已按「消失最久」在前排好。
    pub fn set_stats(&mut self, mut stats: Vec<Appearance>) {
        // 长期未出现的浮到前面：chapters_since_last 降序；从未出场的（None）垫底，
        // 它们是「还没登场」而非「消失」，另算。
        stats.sort_by(|a, b| {
            let ka = a.chapters_since_last();
            let kb = b.chapters_since_last();
            match (ka, kb) {
                (Some(x), Some(y)) => y.cmp(&x).then(a.name.cmp(&b.name)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.cmp(&b.name),
            }
        });
        self.stats = Some(stats);
        self.cursor = 0;
        self.scroll = 0;
    }

    pub fn clear_stats(&mut self) {
        self.stats = None;
        self.cursor = 0;
        self.scroll = 0;
    }

    pub fn stats(&self) -> Option<&[Appearance]> {
        self.stats.as_deref()
    }

    /// 当前视图里可导航的条目数（统计视图或卡片列表）。
    fn active_len(&self) -> usize {
        match &self.stats {
            Some(s) => s.len(),
            None => self.filtered_count(),
        }
    }

    pub fn move_down(&mut self) {
        let n = self.active_len();
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

    /// 出场统计的一行：`沈砚  128次  最近第12章 · 近5章未出现`。
    pub fn stat_line(a: &Appearance) -> String {
        if a.total == 0 {
            return format!("{}  —  尚未出场", a.name);
        }
        let last = a
            .last
            .as_ref()
            .map(|(i, _)| format!("最近第{}章", i + 1))
            .unwrap_or_default();
        match a.chapters_since_last() {
            Some(0) | None => format!("{}  {}次  {last}", a.name, a.total),
            Some(n) => format!("{}  {}次  {last} · 近{n}章未出现", a.name, a.total),
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

    /// id → 名字，供关系目标解析。
    fn name_of(&self, id: mj_core::id::CharacterId) -> Option<&str> {
        self.all
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.as_str())
    }

    /// 选中角色的详情行（右侧/下方展示）。空字段跳过，别撑出一堆空标签。
    ///
    /// 关系目标（`CharacterId`）解析成名字显示；目标已删除则退回显示 id 尾段。
    pub fn detail_lines(&self, c: &Character) -> Vec<String> {
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
                let target = self
                    .name_of(r.target)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "（已删除）".to_string());
                v.push(format!("· {}：{target}", r.label));
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
        let p = CharacterPanel::new(vec![c.clone()]);
        let lines = p.detail_lines(&c);
        let text = lines.join("\n");
        assert!(text.contains("背景"), "有背景应显示");
        assert!(!text.contains("性别"), "空字段不该出现标签：{text}");
    }

    #[test]
    fn detail_resolves_relation_target_names() {
        use mj_core::model::Relation;
        let master = ch("师父", &[], "配角");
        let mut disciple = ch("沈砚", &[], "主角");
        disciple.relations = vec![Relation {
            target: master.id,
            label: "师父".into(),
        }];
        let p = CharacterPanel::new(vec![master.clone(), disciple.clone()]);
        let text = p.detail_lines(&disciple).join("\n");
        assert!(text.contains("师父：师父"), "关系应解析出目标名：{text}");
    }

    #[test]
    fn detail_relation_to_deleted_target_is_graceful() {
        use mj_core::model::Relation;
        let mut c = ch("沈砚", &[], "");
        c.relations = vec![Relation {
            target: CharacterId::generate(), // 不在列表里 = 已删除
            label: "旧友".into(),
        }];
        let p = CharacterPanel::new(vec![c.clone()]);
        let text = p.detail_lines(&c).join("\n");
        assert!(text.contains("旧友：（已删除）"), "{text}");
    }

    #[test]
    fn stats_sort_absent_first() {
        use mj_core::appearance::Appearance;
        use mj_core::id::CharacterId;
        let a = |name: &str, last: Option<usize>, total: usize| Appearance {
            id: CharacterId::generate(),
            name: name.into(),
            total,
            last: last.map(|i| (i, format!("第{}章", i + 1))),
            total_chapters: 10,
        };
        let mut p = CharacterPanel::new(vec![]);
        // 甲最近第9章(近0章)、乙最近第2章(近7章)、丙从未出场。
        p.set_stats(vec![
            a("甲", Some(9), 5),
            a("乙", Some(2), 3),
            a("丙", None, 0),
        ]);
        let order: Vec<&str> = p.stats().unwrap().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(order, vec!["乙", "甲", "丙"], "消失最久在前，未出场垫底");
        assert!(p.show_stats());
        p.clear_stats();
        assert!(!p.show_stats());
    }

    #[test]
    fn stat_line_flags_absence() {
        use mj_core::appearance::Appearance;
        use mj_core::id::CharacterId;
        let absent = Appearance {
            id: CharacterId::generate(),
            name: "乙".into(),
            total: 3,
            last: Some((2, "第3章".into())),
            total_chapters: 10,
        };
        let line = CharacterPanel::stat_line(&absent);
        assert!(line.contains("近7章未出现"), "{line}");
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
