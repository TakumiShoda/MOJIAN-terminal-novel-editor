//! 渲染快照。见 doc.md §10：各屏幕在 60/80/120/200 列宽下不崩。
//!
//! doc.md §7.2 要求窄至 60 列不崩，故最小宽度取 60。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

use mj_tui::app::App;

fn render_at(width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    let app = App::new();
    term.draw(|f| app.render_for_test(f)).unwrap();

    // Buffer -> 纯文本。
    //
    // CJK 字符占两个单元格（doc.md §2.3）。ratatui 0.30 的 TestBackend 在后继格里
    // 填的是空格而非空串，所以逐格拼接会把「退出」读成「退 出」。
    // 正确做法：按字符的显示宽度跳过它占掉的后继格。
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0u16;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                let w = UnicodeWidthStr::width(sym).max(1) as u16;
                x += w;
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn renders_at_common_widths_without_panicking() {
    // doc.md §10 指定的四档宽度 + §7.2 的 60 列下限。
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let out = render_at(w, h);
        assert!(out.contains("墨简"), "{w}x{h} 应渲染出标题");
    }
}

/// 极端尺寸不得 panic——用户拖窗口时会瞬间经过这些值。
#[test]
fn survives_degenerate_sizes() {
    for (w, h) in [(1, 1), (2, 2), (60, 3), (200, 1)] {
        let _ = render_at(w, h); // 不崩即通过
    }
}

#[test]
fn status_bar_shows_quit_hint() {
    let out = render_at(80, 24);
    assert!(out.contains("退出"), "状态栏应提示退出键:\n{out}");
}
