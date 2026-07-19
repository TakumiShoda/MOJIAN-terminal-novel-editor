//! kitty 键盘协议。见 doc.md §2.3、§7.3 的注。
//!
//! # 它解决什么
//!
//! 传统键盘模式下，终端对 `Ctrl+S` 与 `Ctrl+Shift+S` 发的是**同一个字节**（0x13）——
//! Shift 压根没编码进去。`Ctrl+Tab` 同理，与 `Tab` 无从区分。于是 §7.3 键位表里
//! 这两个键在实现时只能搁置（打快照改用 F9）。
//!
//! kitty 键盘协议（CSI u）把修饰键完整编码进转义序列，这两个键就能到程序了。
//!
//! # 三条得当心的事
//!
//! 1. **必须探测再开**（§2.3：不支持时静默降级）。不支持的终端收到 `CSI > u`
//!    多半会忽略，但也可能把它当普通输入吐到屏幕上。
//! 2. **退出时必须弹栈**，且 panic 路径也要弹——否则用户的 shell 会留在增强模式，
//!    之后按键行为全不对。这与字体恢复是同一类事故，故照 `font::emit_reset_sequence`
//!    的做法留一个不依赖任何状态的直发函数。
//! 3. **开了协议会多收到按键释放事件**。本程序在事件循环里只认 `KeyEventKind::Press`
//!    （app.rs 两处都判了），故不会一次按键算两下。改那两处之前先想想这里。

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

/// 本进程是否真的开启过增强模式。退出与 panic 路径据此决定要不要弹栈。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 我们要的能力：只要「把修饰键编码进来」这一条。
///
/// 不要 `REPORT_EVENT_TYPES`（会额外送释放/重复事件）、也不要
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES`（会把普通字符也变成转义序列，
/// 中文输入法上屏的文本会遭殃）。只拿必需的那一位，副作用最小。
fn flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
}

/// 终端是否支持（§2.3：需运行时探测，不支持时静默降级）。
pub fn probe() -> bool {
    ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
}

/// 探测并开启。返回是否真的开了。
///
/// 探测失败/不支持一律返回 false 并什么都不做——**不报错**：
/// 这是锦上添花的能力，缺了只是少两个键位，不该拦着人写字。
pub fn enable() -> bool {
    if !probe() {
        tracing::info!("终端不支持 kitty 键盘协议，按传统模式运行");
        return false;
    }
    let mut out = std::io::stdout();
    if ratatui::crossterm::execute!(out, PushKeyboardEnhancementFlags(flags())).is_err() {
        tracing::warn!("开启 kitty 键盘协议失败，按传统模式运行");
        return false;
    }
    let _ = out.flush();
    ENABLED.store(true, Ordering::SeqCst);
    tracing::info!("已开启 kitty 键盘协议：Ctrl+Shift+S / Ctrl+Tab 可用");
    true
}

/// 关闭（弹栈）。没开过就什么都不做。
pub fn disable() {
    if !ENABLED.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    let _ = ratatui::crossterm::execute!(out, PopKeyboardEnhancementFlags);
    let _ = out.flush();
}

/// 直发弹栈序列，供 panic hook。
///
/// 与 `disable` 的区别：不走 crossterm 的 `execute!`（panic 时它可能正持着锁），
/// 直接把字节写出去。没开过就不发——免得给不相干的终端塞转义。
pub fn emit_pop_sequence() {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    // CSI < u ——弹出键盘增强标志栈。
    let _ = out.write_all(b"\x1b[<u");
    let _ = out.flush();
}

/// 当前是否处于增强模式（供状态展示与 doctor）。
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只取必需的那一位：多要一位就多一类副作用。
    #[test]
    fn only_asks_for_disambiguation() {
        let f = flags();
        assert!(f.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        assert!(
            !f.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES),
            "别要释放事件——会让每次按键算两下"
        );
        assert!(
            !f.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES),
            "别把普通字符也转义——中文上屏的文本会遭殃"
        );
    }

    /// 没开过的时候，关闭与直发都必须是空操作（且不 panic）。
    ///
    /// 测试环境没有真终端，`enable()` 会探测失败并返回 false，正好覆盖这条路径。
    #[test]
    fn disabling_without_enabling_is_a_noop() {
        assert!(!is_enabled());
        disable(); // 不得 panic
        emit_pop_sequence(); // 不得往 stdout 写东西
        assert!(!is_enabled());
    }

    /// 无终端环境下探测应当老实返回 false，而不是把程序拖垮。
    #[test]
    fn probe_is_safe_without_a_terminal() {
        let _ = probe(); // 不得 panic
    }
}
