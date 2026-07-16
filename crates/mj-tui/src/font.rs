//! FontController 与能力探测。见 doc.md §2.1、§6.10。
//!
//! M0 只实现 panic/退出路径所需的「重置字体」；多后端探测与切换是 M6。
//! `[VERIFY]` 各终端的实际能力必须在真机上验证，不得照抄文档表格。

use std::io::Write as _;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FontCap: u8 {
        const SET_FAMILY = 0b001;
        const SET_SIZE   = 0b010;
        const RESET      = 0b100;
    }
}

pub trait FontController: Send {
    fn id(&self) -> &'static str;
    fn caps(&self) -> FontCap;
    fn set_family(&mut self, family: &str) -> anyhow::Result<()>;
    fn set_size(&mut self, pt: f32) -> anyhow::Result<()>;
    fn reset(&mut self) -> anyhow::Result<()>;
}

/// 什么都做不到的后端——doc.md §2.1 的「三级」：Alacritty / WT / VS Code 内置终端等。
///
/// 不是失败，是诚实：UI 应据 `caps()` 显示灰态与原因，而不是假装能改。
#[derive(Debug, Default)]
pub struct NoopFont;

impl FontController for NoopFont {
    fn id(&self) -> &'static str {
        "noop"
    }

    fn caps(&self) -> FontCap {
        FontCap::empty()
    }

    fn set_family(&mut self, _family: &str) -> anyhow::Result<()> {
        anyhow::bail!("当前终端不支持运行时更改字体")
    }

    fn set_size(&mut self, _pt: f32) -> anyhow::Result<()> {
        anyhow::bail!("当前终端不支持运行时更改字号")
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// 探测可用后端。M0 一律返回 `NoopFont`；真实探测见 M6（doc.md §6.10）。
pub fn detect() -> Box<dyn FontController> {
    Box::new(NoopFont)
}

/// 直接向 stdout 发送「恢复默认字体」的 OSC 50 序列。
///
/// 专供 panic hook 与退出路径：这两处不能依赖 `FontController`（可能正持锁，
/// 或本身就是 panic 的来源）。对不支持 OSC 50 的终端无副作用——
/// 它们会忽略未知的 OSC 序列。
///
/// `[VERIFY]` 空参数的 OSC 50 在各终端下是否确为「恢复默认」，需真机验证（M6）。
pub fn emit_reset_sequence() {
    // 仅当我们确实改过字体时才有必要发送。M0 从不改字体，故此处直接返回，
    // 留出调用点以便 M6 接上真实逻辑而不必再动 panic hook。
    if !font_was_changed() {
        return;
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b]50;\x07");
    let _ = out.flush();
}

/// 本进程是否改过终端字体。M6 接入 FontController 后由其置位。
fn font_was_changed() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_reports_no_caps() {
        let f = NoopFont;
        assert!(f.caps().is_empty());
        assert_eq!(f.id(), "noop");
    }

    /// 不支持时必须报错而非假装成功——UI 要据此显示灰态与原因（doc.md §6.10）。
    #[test]
    fn noop_refuses_changes_but_reset_is_ok() {
        let mut f = NoopFont;
        assert!(f.set_family("Source Han Serif").is_err());
        assert!(f.set_size(14.0).is_err());
        assert!(f.reset().is_ok(), "reset 应总是安全的");
    }

    #[test]
    fn emit_reset_is_safe_to_call() {
        emit_reset_sequence(); // 不得 panic
    }
}
