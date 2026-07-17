//! 批量作业的确认框。见 doc.md §6.5、§6.6。
//!
//! §0 的话是「破坏性操作必须可撤销或可预览」。全书替换两样都有了（快照 +
//! Alt+U），照理不必再拦一道。但「可撤销」是**事后**的补救，前提是用户
//! 意识到自己按错了——一次跨 200 章的替换，改坏的地方大多不在眼前，
//! 等发现时人早就走远了。所以宽范围仍然要当面报一遍数：**改哪些章、
//! 把什么换成什么**。
//!
//! 默认落在「取消」上：手滑连按两下回车不该把整本书改掉。

use crate::batch::{BatchKind, Scope};

pub struct Confirm {
    pub kind: BatchKind,
    pub scope: Scope,
    /// 范围内的章节数。
    pub chapters: usize,
    /// 默认 false = 停在「取消」。
    yes: bool,
}

impl Confirm {
    pub fn new(kind: BatchKind, scope: Scope, chapters: usize) -> Self {
        Self {
            kind,
            scope,
            chapters,
            yes: false,
        }
    }

    pub fn is_yes(&self) -> bool {
        self.yes
    }

    /// ←/→ 或 Tab 切换。
    pub fn toggle(&mut self) {
        self.yes = !self.yes;
    }

    pub fn title(&self) -> String {
        format!("确认{}", self.kind.label())
    }

    /// 正文：把要做的事一条条摆出来。
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "范围：{}（{} 章）",
            self.scope.label(),
            self.chapters
        )];
        match &self.kind {
            BatchKind::Format(_) => {
                v.push("对范围内每一章执行排版规范化。".into());
            }
            BatchKind::Replace { query, to } => {
                v.push(format!("查找：{}", show(&query.pattern)));
                v.push(format!("替换为：{}", show(to)));
                // 空替换 = 删除。这事值得单说一句，别让人以为没写完。
                if to.is_empty() {
                    v.push("（替换内容为空 = 删除所有命中）".into());
                }
                // 结果列表只有当前章的命中，勾选管不到别的章。说清楚。
                v.push("范围内所有命中都会被替换，不受结果列表勾选影响。".into());
            }
        }
        v.push(String::new());
        v.push("每章执行前都会各打一条快照，事后 Alt+U 可整体撤销。".into());
        v
    }
}

/// 把空串和首尾空白显示成看得见的样子——
/// 「替换为： 」和「替换为：」在屏幕上长得一样，可意思差着一个空格。
fn show(s: &str) -> String {
    if s.is_empty() {
        return "（空）".into();
    }
    if s != s.trim() {
        return format!("「{s}」（注意首尾空白）");
    }
    format!("「{s}」")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_text::search::Query;

    fn replace(pattern: &str, to: &str) -> BatchKind {
        BatchKind::Replace {
            query: Query {
                pattern: pattern.into(),
                ..Default::default()
            },
            to: to.into(),
        }
    }

    /// 手滑连按两下回车不该把整本书改掉。
    #[test]
    fn defaults_to_cancel() {
        let c = Confirm::new(replace("甲", "乙"), Scope::Book, 200);
        assert!(!c.is_yes(), "默认必须停在「取消」");
    }

    #[test]
    fn toggle_flips() {
        let mut c = Confirm::new(replace("甲", "乙"), Scope::Book, 1);
        c.toggle();
        assert!(c.is_yes());
        c.toggle();
        assert!(!c.is_yes());
    }

    #[test]
    fn shows_scope_and_chapter_count() {
        let c = Confirm::new(replace("甲", "乙"), Scope::Volume, 12);
        let text = c.lines().join("\n");
        assert!(text.contains("当前卷"), "{text}");
        assert!(text.contains("12 章"), "{text}");
    }

    /// 空替换 = 删除。用户得知道自己按下去会发生什么。
    #[test]
    fn empty_replacement_is_called_out_as_deletion() {
        let c = Confirm::new(replace("甲", ""), Scope::Book, 3);
        let text = c.lines().join("\n");
        assert!(text.contains("删除"), "空替换要说明是删除：{text}");
    }

    /// 「乙 」和「乙」在屏幕上看不出差别，但结果差一个空格。
    #[test]
    fn trailing_space_is_visible() {
        let c = Confirm::new(replace("甲", "乙 "), Scope::Book, 3);
        let text = c.lines().join("\n");
        assert!(text.contains("首尾空白"), "{text}");
    }

    /// 勾选只对当前章有效——宽范围替换必须讲明这点，否则用户会以为
    /// 自己取消勾选的那些命中不会被改。
    #[test]
    fn wide_replace_states_that_checkmarks_dont_apply() {
        let c = Confirm::new(replace("甲", "乙"), Scope::Book, 3);
        let text = c.lines().join("\n");
        assert!(text.contains("勾选"), "{text}");
    }

    #[test]
    fn mentions_undo_path() {
        let c = Confirm::new(replace("甲", "乙"), Scope::Book, 3);
        let text = c.lines().join("\n");
        assert!(text.contains("快照"), "{text}");
        assert!(text.contains("Alt+U"), "{text}");
    }

    #[test]
    fn format_job_has_no_replace_fields() {
        let c = Confirm::new(BatchKind::Format(Default::default()), Scope::Book, 5);
        let text = c.lines().join("\n");
        assert!(c.title().contains("排版"), "{}", c.title());
        assert!(
            !text.contains("查找："),
            "排版作业不该出现查找/替换：{text}"
        );
    }
}
