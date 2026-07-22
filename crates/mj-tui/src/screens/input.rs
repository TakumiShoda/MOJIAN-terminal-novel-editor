//! 单行文本输入浮层（§7.1 的 `Input`）。见 doc.md §6.1、§6.2。
//!
//! 一切要「敲字进去」的操作都靠它：给章/卷/书改名。浮层只管收字，
//! **要拿这些字干什么由 app 决定**——`intent` 把目标 id 一并带着，
//! app 在提交时照它调对应的 store 操作。
//!
//! 输入沿用命令面板那套 char 级 push/pop：中文由终端输入法处理，程序收到的是
//! 上屏后的最终字符（§2.3），不必自己管候选框。

use mj_core::id::{BookId, ChapterId, VolumeId};

/// 提交后要干的事。带上目标 id——浮层不认得 store，动作归 app。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputIntent {
    RenameBook(BookId),
    RenameVolume(VolumeId),
    RenameChapter(ChapterId),
}

pub struct Input {
    /// 浮层标题，如「重命名章」。
    title: String,
    /// 当前文本。改名时预填原名，好让用户在原名上改而非从头敲。
    value: String,
    intent: InputIntent,
}

impl Input {
    pub fn new(title: impl Into<String>, initial: impl Into<String>, intent: InputIntent) -> Self {
        Self {
            title: title.into(),
            value: initial.into(),
            intent,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn intent(&self) -> InputIntent {
        self.intent
    }

    pub fn input_char(&mut self, c: char) {
        // 控制字符（除已被上层截走的回车/退格外）不进正文——防止把制表、
        // 换页之类塞进名字里，那会让文件名与显示都出怪。
        if !c.is_control() {
            self.value.push(c);
        }
    }

    pub fn backspace(&mut self) {
        self.value.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mj_core::id::ChapterId;

    fn intent() -> InputIntent {
        InputIntent::RenameChapter(ChapterId::generate())
    }

    #[test]
    fn prefills_and_edits() {
        let mut i = Input::new("重命名章", "第一章", intent());
        assert_eq!(i.value(), "第一章", "改名要预填原名");
        i.backspace();
        assert_eq!(i.value(), "第一", "退格删一个字");
        i.input_char('回');
        assert_eq!(i.value(), "第一回");
    }

    #[test]
    fn rejects_control_chars() {
        let mut i = Input::new("t", "", intent());
        i.input_char('\t');
        i.input_char('\n');
        i.input_char('好');
        assert_eq!(i.value(), "好", "控制字符不该进名字");
    }
}
