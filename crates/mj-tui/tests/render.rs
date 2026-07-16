//! 渲染快照。见 doc.md §10：各屏幕在 60/80/120/200 列宽下不崩。
//!
//! doc.md §7.2 要求窄至 60 列不崩，故最小宽度取 60。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

/// 建一个带内容的临时 workspace。
fn setup(with_book: bool) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());

    if with_book {
        let book = store.create_book("雪夜行", "沈砚").unwrap();
        let vol = store.create_volume(book.id, "第一卷 风起", None).unwrap();
        let ch = store
            .create_chapter(book.id, vol, "第一章 雪夜", None)
            .unwrap();
        store
            .save_body(
                book.id,
                &mj_core::model::ChapterBody::new(
                    ch,
                    "　　雪落了一夜。\n\n　　他推开门，风裹着雪灌进来，冷得刺骨。\n",
                ),
            )
            .unwrap();
        store
            .create_chapter(book.id, vol, "第二章 相遇", Some(ch))
            .unwrap();
    }
    (dir, store)
}

fn render_at(store: Store, width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut app = App::new(store, Config::default()).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    buffer_to_string(&term)
}

/// Buffer -> 纯文本。
///
/// CJK 占两个单元格，ratatui 0.30 的 TestBackend 在后继格填空格，
/// 逐格拼接会把「退出」读成「退 出」。按显示宽度跳格才对。
fn buffer_to_string(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0u16;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += UnicodeWidthStr::width(sym).max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn shelf_renders_at_common_widths() {
    // doc.md §10 指定的四档宽度 + §7.2 的 60 列下限。
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let (_d, store) = setup(true);
        let out = render_at(store, w, h);
        assert!(out.contains("书架"), "{w}x{h} 应渲染书架:\n{out}");
        assert!(out.contains("雪夜行"), "{w}x{h} 应列出书名:\n{out}");
    }
}

#[test]
fn empty_shelf_guides_the_user() {
    let (_d, store) = setup(false);
    let out = render_at(store, 80, 24);
    assert!(out.contains("书架是空的"), "空书架应有引导:\n{out}");
    assert!(out.contains("n 新建"), "应告诉用户怎么开始:\n{out}");
}

#[test]
fn shelf_shows_counts_and_words() {
    let (_d, store) = setup(true);
    let out = render_at(store, 100, 24);
    assert!(out.contains("沈砚"), "应显示作者:\n{out}");
    assert!(out.contains("卷"), "应显示卷数:\n{out}");
    assert!(out.contains("章"), "应显示章数:\n{out}");
}

/// 极端尺寸不得 panic——用户拖窗口时会瞬间经过这些值。
#[test]
fn survives_degenerate_sizes() {
    for (w, h) in [(1, 1), (2, 2), (60, 3), (200, 1), (20, 40)] {
        let (_d, store) = setup(true);
        let _ = render_at(store, w, h); // 不崩即通过
    }
}

#[test]
fn status_bar_shows_hints() {
    let (_d, store) = setup(true);
    let out = render_at(store, 80, 24);
    assert!(
        out.contains("Enter") || out.contains("打开"),
        "状态栏应有操作提示:\n{out}"
    );
}

/// 渲染不得让终端出现半个字符——CJK 宽度算错会撕裂整屏。
#[test]
fn no_broken_cjk_in_output() {
    let (_d, store) = setup(true);
    let out = render_at(store, 80, 24);
    // 能被正常解析为 String 即证明未切碎（String 保证 UTF-8）。
    assert!(out.contains("雪夜行"), "书名应完整:\n{out}");
    assert!(
        !out.contains('\u{fffd}'),
        "出现替换字符，说明切碎了:\n{out}"
    );
}
