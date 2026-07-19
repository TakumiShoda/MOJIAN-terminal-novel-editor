//! 主题真的落到屏幕上了吗。见 doc.md §6.10。
//!
//! 单测证「TOML → 颜色」的映射，这里证**渲染管线确实用了它**——
//! 主题解析对了但渲染仍写死颜色，是最容易发生也最不容易发现的那种坏法。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

/// 建一个带书的 workspace，可指定主题名与自定义主题文件。
fn app_with_theme(theme: &str, user_theme: Option<(&str, &str)>) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    if let Some((name, text)) = user_theme {
        std::fs::write(ws.theme_file(name), text).unwrap();
    }
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    store.create_chapter(book.id, vol, "第一章", None).unwrap();

    let mut config = Config::default();
    config.appearance.theme = theme.into();
    let store = Store::new(
        Workspace::resolve(Some(dir.path().to_path_buf())).unwrap(),
        config.clone(),
    );
    let app = App::new(store, config).unwrap();
    (dir, app)
}

/// 画一帧，返回屏幕上出现过的全部前景色与背景色。
fn colors_on_screen(app: &mut App, w: u16, h: u16) -> (Vec<Color>, Vec<Color>) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut fg = Vec::new();
    let mut bg = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if !fg.contains(&cell.fg) {
                fg.push(cell.fg);
            }
            if !bg.contains(&cell.bg) {
                bg.push(cell.bg);
            }
        }
    }
    (fg, bg)
}

/// sepia 的底色必须真的铺到屏幕上——这是「外观预设」观感的主要来源（§2.1）。
#[test]
fn sepia_background_reaches_the_screen() {
    let (_d, mut app) = app_with_theme("sepia", None);
    app.open_first_book_for_demo().unwrap();
    let (_fg, bg) = colors_on_screen(&mut app, 100, 30);
    // sepia 的 bg 是 #f4ecd8。真彩下应原样出现；256 色下是对应的近似索引。
    let want_true = Color::Rgb(0xf4, 0xec, 0xd8);
    let want_256 = Color::Indexed(mj_tui::theme::Rgb(0xf4, 0xec, 0xd8).to_ansi256());
    assert!(
        bg.contains(&want_true) || bg.contains(&want_256),
        "屏幕上没有 sepia 底色，实际背景色：{bg:?}"
    );
}

/// 换主题必须换出不同的画面——否则说明渲染写死了颜色。
#[test]
fn different_themes_paint_differently() {
    let (_d1, mut sepia) = app_with_theme("sepia", None);
    sepia.open_first_book_for_demo().unwrap();
    let (_, bg_sepia) = colors_on_screen(&mut sepia, 100, 30);

    let (_d2, mut hc) = app_with_theme("high_contrast", None);
    hc.open_first_book_for_demo().unwrap();
    let (_, bg_hc) = colors_on_screen(&mut hc, 100, 30);

    assert_ne!(bg_sepia, bg_hc, "换主题后画面配色应当不同");
}

/// 用户 themes/<name>.toml 覆盖同名内置主题。
#[test]
fn user_theme_file_overrides_builtin() {
    // 自建一个把 accent 改成纯品红的 "sepia"。
    let (_d, mut app) = app_with_theme(
        "sepia",
        Some(("sepia", "name = \"我的 sepia\"\naccent = \"#ff00ff\"\n")),
    );
    app.open_first_book_for_demo().unwrap();
    let (fg, _bg) = colors_on_screen(&mut app, 100, 30);
    let magenta_true = Color::Rgb(0xff, 0x00, 0xff);
    let magenta_256 = Color::Indexed(mj_tui::theme::Rgb(0xff, 0x00, 0xff).to_ansi256());
    assert!(
        fg.contains(&magenta_true) || fg.contains(&magenta_256),
        "用户主题的 accent 没生效，前景色：{fg:?}"
    );
}

/// 主题写坏了不该打不开稿子——回落内置配色，照常渲染。
#[test]
fn broken_user_theme_still_renders() {
    let (_d, mut app) = app_with_theme("sepia", Some(("sepia", "这不是 toml [[[")));
    app.open_first_book_for_demo().unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    // 不 panic 即通过。
    term.draw(|f| app.render_for_test(f)).unwrap();
}

/// 未知主题名回落，不 panic。
#[test]
fn unknown_theme_name_still_renders() {
    let (_d, mut app) = app_with_theme("查无此主题", None);
    app.open_first_book_for_demo().unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
}

/// 所有内置主题在各档宽度下都画得出来且不撕屏。
#[test]
fn all_builtin_themes_render_across_widths() {
    for name in mj_tui::theme::builtin_names() {
        for (w, h) in [(60, 20), (80, 24), (120, 30)] {
            let (_d, mut app) = app_with_theme(name, None);
            app.open_first_book_for_demo().unwrap();
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.render_for_test(f)).unwrap();
            let buf = term.backend().buffer().clone();
            let text: String = (0..buf.area.height)
                .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                .map(|(x, y)| buf[(x, y)].symbol().to_string())
                .collect();
            assert!(!text.contains('\u{fffd}'), "主题 {name} 在 {w}x{h} 撕屏了");
        }
    }
}
