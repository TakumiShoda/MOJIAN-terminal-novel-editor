//! 帮助页（`F1`）：键位总表。见 doc.md §7.3。
//!
//! 内容**从命令表生成**，不另抄一份。手写的帮助页迟早与实现分叉，
//! 而分叉的帮助页比没有帮助页更坑人——用户按着它给的键，发现没反应。
//!
//! 表里没有专属键位的命令也列出来（键位栏留空），并注明可从命令面板触达。
//! 另有一小节列「不属于任何命令」的通用键（Esc、Tab、方向键这类）。

use crate::commands::{Category, by_category};

/// 帮助页里的一行。
#[derive(Debug, Clone, PartialEq)]
pub enum HelpRow {
    /// 分组标题。
    Section(String),
    /// 一条键位：（键, 说明）。
    Entry {
        keys: String,
        what: String,
    },
    Blank,
}

/// 与命令无关的通用键——它们不是「功能」，是界面的基本操作，
/// 故不进命令表，但帮助页必须写明。
const GENERAL: &[(&str, &str)] = &[
    ("Ctrl+P", "命令面板（所有功能都能从这里找到）"),
    ("Esc", "关掉当前浮层；没有浮层时回书架"),
    ("Tab", "在目录树与正文之间切换焦点"),
    ("↑ ↓ / j k", "在列表里上下移动"),
    ("Enter", "打开 / 确认"),
    ("Ctrl+C", "立即退出"),
];

/// 各面板内部的专属键——同样不是独立命令，但不写用户找不到。
const IN_PANEL: &[(&str, &str)] = &[
    ("查找替换内 F4", "切换范围：当前章 / 当前卷 / 全书"),
    ("查找替换内 Alt+R / Alt+A", "替换当前一处 / 替换全部"),
    ("排版预览内 空格", "勾选或取消某条改动"),
    ("排版预览内 V / B", "把排版施加到当前卷 / 全书"),
    ("校对面板内 a / i / I", "应用建议 / 忽略本次 / 永久忽略"),
    ("校对面板内 f", "展开或收起低置信提示"),
    ("历史面板内 Enter", "与选中的快照比对"),
    ("diff 内 n / p", "跳到下一处 / 上一处改动"),
    ("diff 内 u / U / y", "恢复此块 / 恢复整章 / 复制旧内容"),
    ("角色面板内 n / e / d", "新建 / 编辑 / 删除角色"),
    ("角色面板内 / 与 t", "搜索角色 / 出场统计"),
    ("正文内 @", "补全角色名"),
];

pub struct Help {
    scroll: usize,
    height: usize,
}

impl Default for Help {
    fn default() -> Self {
        Self::new()
    }
}

impl Help {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            height: 10,
        }
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h.max(1);
        self.clamp();
    }

    pub fn scroll_down(&mut self) {
        let max = Self::rows().len().saturating_sub(self.height);
        self.scroll = (self.scroll + 1).min(max);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn clamp(&mut self) {
        let max = Self::rows().len().saturating_sub(self.height);
        self.scroll = self.scroll.min(max);
    }

    /// 整页内容，键位取自**当前生效的键位表**。
    ///
    /// 不能直接印命令表里的默认键：用户在 `[keymap]` 重绑过之后，
    /// 帮助页若还印默认值，就成了一张骗人的表。
    pub fn rows_with(keymap: &crate::keymap::Keymap) -> Vec<HelpRow> {
        Self::build(Some(keymap))
    }

    /// 整页内容（用命令表里的默认键位）。
    pub fn rows() -> Vec<HelpRow> {
        Self::build(None)
    }

    fn build(keymap: Option<&crate::keymap::Keymap>) -> Vec<HelpRow> {
        let mut out = Vec::new();

        out.push(HelpRow::Section("通用".into()));
        for (k, w) in GENERAL {
            out.push(HelpRow::Entry {
                keys: (*k).to_string(),
                what: (*w).to_string(),
            });
        }

        for cat in Category::all() {
            let items: Vec<_> = by_category(*cat).collect();
            if items.is_empty() {
                continue;
            }
            out.push(HelpRow::Blank);
            out.push(HelpRow::Section(cat.label().to_string()));
            for c in items {
                // 当前实际绑定优先；没有键位的明说走命令面板，别留空让人猜。
                let keys = match keymap.and_then(|k| k.binding_of(c.cmd)) {
                    Some(b) => b.display(),
                    None if keymap.is_some() => "（Ctrl+P）".to_string(),
                    None if c.keys.is_empty() => "（Ctrl+P）".to_string(),
                    None => c.keys.to_string(),
                };
                out.push(HelpRow::Entry {
                    keys,
                    what: format!("{} —— {}", c.name, c.desc),
                });
            }
        }

        out.push(HelpRow::Blank);
        out.push(HelpRow::Section("面板内".into()));
        for (k, w) in IN_PANEL {
            out.push(HelpRow::Entry {
                keys: (*k).to_string(),
                what: (*w).to_string(),
            });
        }

        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::commands::COMMANDS;

    #[test]
    fn lists_every_command() {
        let rows = Help::rows();
        let text: String = rows
            .iter()
            .map(|r| match r {
                HelpRow::Entry { keys, what } => format!("{keys} {what}\n"),
                HelpRow::Section(s) => format!("[{s}]\n"),
                HelpRow::Blank => "\n".into(),
            })
            .collect();
        // 帮助页从命令表生成，故每条命令都必然在页面上。
        for c in COMMANDS {
            assert!(text.contains(c.name), "帮助页漏了命令「{}」", c.name);
        }
    }

    #[test]
    fn shows_the_real_keybindings() {
        let rows = Help::rows();
        let has = |k: &str| {
            rows.iter().any(|r| match r {
                HelpRow::Entry { keys, .. } => keys.contains(k),
                _ => false,
            })
        };
        assert!(has("F7"), "校对的键位该出现在帮助页");
        assert!(has("Ctrl+P"), "命令面板本身也要写明");
        assert!(has("F9"), "打快照用的是 F9（不是够不到的 Ctrl+Shift+S）");
    }

    #[test]
    fn has_sections() {
        let rows = Help::rows();
        let sections: Vec<&String> = rows
            .iter()
            .filter_map(|r| match r {
                HelpRow::Section(s) => Some(s),
                _ => None,
            })
            .collect();
        assert!(sections.iter().any(|s| *s == "通用"));
        assert!(sections.iter().any(|s| *s == "工具"));
        assert!(sections.iter().any(|s| *s == "面板内"));
    }

    #[test]
    fn scrolling_clamps() {
        let mut h = Help::new();
        h.set_height(5);
        h.scroll_up();
        assert_eq!(h.scroll(), 0, "顶部不越界");
        for _ in 0..1000 {
            h.scroll_down();
        }
        let max = Help::rows().len().saturating_sub(5);
        assert_eq!(h.scroll(), max, "底部不越界");
    }

    /// 页面很高时不该还能滚。
    #[test]
    fn no_scroll_when_everything_fits() {
        let mut h = Help::new();
        h.set_height(1000);
        h.scroll_down();
        assert_eq!(h.scroll(), 0);
    }
}
