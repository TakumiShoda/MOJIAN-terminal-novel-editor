//! 命令表。见 doc.md §7.3。
//!
//! `Ctrl+P` 命令面板那条要求是「所有功能都必须能从这里触达 —— 这是最重要的一条」。
//! 要做到这点，前提是有一张**唯一的**命令表：面板从它取候选，帮助页（F1）也从它
//! 生成键位总表。两处共用一张表，键位说明与实际能做的事就不可能对不上——
//! 分两处手写迟早会分叉，而分叉的帮助页比没有帮助页更坑人。
//!
//! 加功能的规矩：先往这张表里加一条，再去实现 `App::run_command` 的分支。
//! 表里有而跑不动的命令，比不在表里更糟。

/// 一条可执行的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    // 文件
    Save,
    Snapshot,
    NewChapter,
    BackToShelf,
    Export,
    NextChapter,
    PrevChapter,
    Quit,
    // 编辑
    Undo,
    Redo,
    Find,
    Replace,
    Format,
    UndoBatch,
    // 工具
    Proof,
    ProofLlm,
    History,
    Characters,
    Stats,
    // 视图
    ToggleTree,
    Appearance,
    FocusMode,
    // 帮助
    Help,
}

impl Command {
    /// 稳定标识，用作 `[keymap]` 里的配置键。
    ///
    /// **不要改这些字符串**——改一个，用户配置里对应的重绑定就悄悄失效了。
    pub fn id(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Snapshot => "snapshot",
            Self::NewChapter => "new_chapter",
            Self::BackToShelf => "back_to_shelf",
            Self::Export => "export",
            Self::NextChapter => "next_chapter",
            Self::PrevChapter => "prev_chapter",
            Self::Quit => "quit",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Find => "find",
            Self::Replace => "replace",
            Self::Format => "format",
            Self::UndoBatch => "undo_batch",
            Self::Proof => "proof",
            Self::ProofLlm => "proof_llm",
            Self::History => "history",
            Self::Characters => "characters",
            Self::Stats => "stats",
            Self::ToggleTree => "toggle_tree",
            Self::Appearance => "appearance",
            Self::FocusMode => "focus_mode",
            Self::Help => "help",
        }
    }

    /// 是否占用一个**全局**键位（可被 `[keymap]` 重绑定）。
    ///
    /// `Esc` 那种上下文相关的键不算：它在浮层里是「关掉这层」、在正文里是
    /// 「取消选区」、在树里才是「回书架」。登记成全局键会把前两者吃掉。
    pub fn has_global_key(self) -> bool {
        !matches!(self, Self::BackToShelf | Self::Export)
    }

    /// 按 id 找命令。
    pub fn from_id(id: &str) -> Option<Self> {
        COMMANDS.iter().map(|c| c.cmd).find(|c| c.id() == id)
    }
}

/// 命令分类，帮助页按此分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    File,
    Edit,
    Tools,
    View,
    Help,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "文件",
            Self::Edit => "编辑",
            Self::Tools => "工具",
            Self::View => "视图",
            Self::Help => "帮助",
        }
    }

    /// 帮助页里的分组顺序。
    pub fn all() -> &'static [Category] {
        &[
            Category::File,
            Category::Edit,
            Category::Tools,
            Category::View,
            Category::Help,
        ]
    }
}

/// 一条命令的说明。
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub cmd: Command,
    pub name: &'static str,
    /// 一句话说明，在面板里跟在名字后面。
    pub desc: &'static str,
    /// 键位，照 §7.3 的表。没有专属键位的写空串。
    pub keys: &'static str,
    pub category: Category,
}

impl CommandSpec {
    /// 供搜索匹配的文本：名字 + 说明 + 键位。
    ///
    /// 键位也参与匹配，这样敲 `f7` 能直接找到「校对当前章」——
    /// 记得住键的人和记不住键的人都能用同一个入口。
    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_ascii_lowercase();
        self.name.to_ascii_lowercase().contains(&q)
            || self.desc.to_ascii_lowercase().contains(&q)
            || self.keys.to_ascii_lowercase().contains(&q)
    }
}

/// 全部命令。**加功能就往这里加一条**（§7.3：所有功能都要能从命令面板触达）。
pub const COMMANDS: &[CommandSpec] = &[
    // ---- 文件 ----
    CommandSpec {
        cmd: Command::Save,
        name: "保存",
        desc: "把当前章写回磁盘",
        keys: "Ctrl+S",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::Snapshot,
        name: "打快照",
        desc: "给当前章存一个可回溯的版本",
        keys: "F9",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::NewChapter,
        name: "新建章",
        desc: "在当前卷末尾新建一章",
        keys: "Ctrl+N",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::BackToShelf,
        name: "回书架",
        desc: "关掉当前书，回到书架",
        keys: "Esc",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::Export,
        name: "导出全书",
        desc: "把整本书导出成 Markdown，存到工作区根目录",
        keys: "",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::NextChapter,
        name: "下一章",
        desc: "跳到下一章（需终端支持 kitty 键盘协议，否则用命令面板）",
        keys: "Ctrl+Tab",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::PrevChapter,
        name: "上一章",
        desc: "跳到上一章（需终端支持 kitty 键盘协议，否则用命令面板）",
        keys: "Ctrl+Shift+Tab",
        category: Category::File,
    },
    CommandSpec {
        cmd: Command::Quit,
        name: "退出",
        desc: "保存并退出墨简",
        keys: "Ctrl+Q",
        category: Category::File,
    },
    // ---- 编辑 ----
    CommandSpec {
        cmd: Command::Undo,
        name: "撤销",
        desc: "退回上一步编辑",
        keys: "Ctrl+Z",
        category: Category::Edit,
    },
    CommandSpec {
        cmd: Command::Redo,
        name: "重做",
        desc: "取消上一次撤销",
        keys: "Ctrl+Y",
        category: Category::Edit,
    },
    CommandSpec {
        cmd: Command::Find,
        name: "查找",
        desc: "在正文里找字",
        keys: "Ctrl+F",
        category: Category::Edit,
    },
    CommandSpec {
        cmd: Command::Replace,
        name: "查找替换",
        desc: "找字并替换，可选当前章/当前卷/全书",
        keys: "Ctrl+H",
        category: Category::Edit,
    },
    CommandSpec {
        cmd: Command::Format,
        name: "一键排版",
        desc: "规范段首缩进、标点、空行，先弹预览",
        keys: "F5",
        category: Category::Edit,
    },
    CommandSpec {
        cmd: Command::UndoBatch,
        name: "撤销批量作业",
        desc: "一次性回滚刚才那次全卷/全书排版或替换",
        keys: "Alt+U",
        category: Category::Edit,
    },
    // ---- 工具 ----
    CommandSpec {
        cmd: Command::Proof,
        name: "校对当前章",
        desc: "查错别字、标点、文风、专名一致性",
        keys: "F7",
        category: Category::Tools,
    },
    CommandSpec {
        cmd: Command::ProofLlm,
        name: "模型校对当前章",
        desc: "把当前章发给大模型查病句（默认关，需先在配置里开启并同意）",
        keys: "",
        category: Category::Tools,
    },
    CommandSpec {
        cmd: Command::History,
        name: "版本历史",
        desc: "翻看快照、比对差异、恢复旧版本",
        keys: "F8",
        category: Category::Tools,
    },
    CommandSpec {
        cmd: Command::Characters,
        name: "角色",
        desc: "角色卡速查、增删改、出场统计",
        keys: "Alt+C",
        category: Category::Tools,
    },
    CommandSpec {
        cmd: Command::Stats,
        name: "统计",
        desc: "按卷/章列字数，可导出 CSV",
        keys: "F3",
        category: Category::Tools,
    },
    // ---- 视图 ----
    CommandSpec {
        cmd: Command::ToggleTree,
        name: "切换目录树",
        desc: "显示/隐藏左侧目录",
        keys: "Ctrl+B",
        category: Category::View,
    },
    CommandSpec {
        cmd: Command::Appearance,
        name: "外观",
        desc: "换主题、看版面设置；字体不可用时给终端配置片段",
        keys: "F2",
        category: Category::View,
    },
    CommandSpec {
        cmd: Command::FocusMode,
        name: "专注模式",
        desc: "收起目录树，正文收窄居中，只剩字",
        keys: "F11",
        category: Category::View,
    },
    // ---- 帮助 ----
    CommandSpec {
        cmd: Command::Help,
        name: "帮助",
        desc: "键位总表",
        keys: "F1",
        category: Category::Help,
    },
];

/// 按分类取命令，供帮助页分组。
pub fn by_category(cat: Category) -> impl Iterator<Item = &'static CommandSpec> {
    COMMANDS.iter().filter(move |c| c.category == cat)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn every_command_has_name_and_desc() {
        for c in COMMANDS {
            assert!(!c.name.is_empty(), "{:?} 缺名字", c.cmd);
            assert!(!c.desc.is_empty(), "{:?} 缺说明", c.cmd);
        }
    }

    /// 命令表里不得有重复的 Command——重复意味着面板里出现两条一样的项。
    #[test]
    fn no_duplicate_commands() {
        let mut seen: Vec<Command> = Vec::new();
        for c in COMMANDS {
            assert!(!seen.contains(&c.cmd), "{:?} 重复登记", c.cmd);
            seen.push(c.cmd);
        }
    }

    /// 每条命令都要落在某个分类里，否则帮助页会漏掉它。
    #[test]
    fn every_command_appears_in_some_category() {
        let grouped: usize = Category::all()
            .iter()
            .map(|c| by_category(*c).count())
            .sum();
        assert_eq!(grouped, COMMANDS.len(), "有命令不属于任何已知分类");
    }

    #[test]
    fn empty_query_matches_everything() {
        for c in COMMANDS {
            assert!(c.matches(""));
        }
    }

    #[test]
    fn matches_by_name() {
        let proof = COMMANDS.iter().find(|c| c.cmd == Command::Proof).unwrap();
        assert!(proof.matches("校对"));
        assert!(!proof.matches("查无此命令"));
    }

    /// 敲键位也要能找到——记得住键的人不必再想名字叫什么。
    #[test]
    fn matches_by_keybinding() {
        let proof = COMMANDS.iter().find(|c| c.cmd == Command::Proof).unwrap();
        assert!(proof.matches("F7"));
        assert!(proof.matches("f7"), "大小写不敏感");
    }

    #[test]
    fn matches_by_description() {
        let fmt = COMMANDS.iter().find(|c| c.cmd == Command::Format).unwrap();
        assert!(fmt.matches("缩进"), "说明里的词也该能搜到");
    }
}
