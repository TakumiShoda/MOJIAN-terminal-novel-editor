//! 外观设置页。见 doc.md §6.10、§2.1。
//!
//! 重点验两件 `[MUST]`：换主题当场生效并持久化；字体不可用时给灰态 + 原因 + 配置片段。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
    ch: ChapterId,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    Fixture { dir, ch }
}

impl Fixture {
    fn app(&self) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        let config = Config::load(&ws.config_file()).unwrap();
        let store = Store::new(ws, config.clone());
        let mut app = App::new(store, config).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }

    fn config_text(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("config.toml")).unwrap_or_default()
    }
}

/// 把屏幕读回文本（按显示宽度跳过全角字的占位格）。
fn screen_text(app: &mut App, w: u16, h: u16) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|fr| app.render_for_test(fr)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut x = 0;
        while x < buf.area.width {
            let s = buf[(x, y)].symbol();
            out.push_str(s);
            x += (UnicodeWidthStr::width(s) as u16).max(1);
        }
        out.push('\n');
    }
    out
}

#[test]
fn f2_opens_appearance() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(2), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Settings"]);
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(app.modal_stack_for_test().is_empty());
}

/// 换主题必须**当场生效**——看不到效果就没法挑主题。
#[test]
fn changing_theme_repaints_immediately() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(2), NONE).unwrap();
    let before = screen_text(&mut app, 100, 30);
    // 光标默认在「主题」行，→ 换下一个。
    app.press_for_test(KeyCode::Right, NONE).unwrap();
    let after = screen_text(&mut app, 100, 30);
    assert_ne!(before, after, "换主题后画面应当立刻不同");
}

/// 关页面时把主题写回 config.toml，重启后仍是它。
#[test]
fn theme_persists_to_config() {
    let f = setup();
    {
        let mut app = f.app();
        app.press_for_test(KeyCode::F(2), NONE).unwrap();
        app.press_for_test(KeyCode::Right, NONE).unwrap();
        app.press_for_test(KeyCode::Esc, NONE).unwrap();
    }
    let text = f.config_text();
    assert!(text.contains("theme"), "配置里该有 theme：{text}");
    // 默认是 sepia，换过之后不该还是它。
    assert!(!text.contains("theme = \"sepia\""), "主题没写回去：{text}");

    // 重开一次，设置页上显示的应是新主题。
    let mut app2 = f.app();
    app2.press_for_test(KeyCode::F(2), NONE).unwrap();
    let shown = screen_text(&mut app2, 100, 30);
    assert!(!shown.contains("‹ sepia ›"), "重启后主题应是改过的那个");
}

/// 没改主题就不该写配置文件——别无端产生一个文件。
#[test]
fn opening_and_closing_without_change_writes_nothing() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(2), NONE).unwrap();
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(f.config_text().is_empty(), "没改动不该写 config.toml");
}

/// §6.10 [MUST]：字体不可用时要有一句话原因，并给出配置片段。
///
/// 测试环境没有 TERM，探测结果是「未知终端」——正好是「什么都改不了」那一档。
#[test]
fn unsupported_font_shows_reason() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(2), NONE).unwrap();
    let text = screen_text(&mut app, 110, 40);
    assert!(text.contains("字体"), "{text}");
    assert!(
        text.contains("不支持运行时更改字体"),
        "要给出一句话原因：{text}"
    );
}

/// 只读的版面项要说明去哪儿改。
#[test]
fn readonly_rows_point_at_config_file() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(2), NONE).unwrap();
    let text = screen_text(&mut app, 110, 40);
    assert!(text.contains("config.toml"), "{text}");
}

/// 命令面板里能找到「外观」并打开它（§7.3：所有功能都要能从这里触达）。
#[test]
fn reachable_from_command_palette() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();
    for c in "外观".chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Settings"]);
}

#[test]
fn renders_across_widths() {
    // §7.2 [MUST]：窄至 60 列不崩。
    for (w, h) in [(60, 20), (80, 24), (120, 30)] {
        let f = setup();
        let mut app = f.app();
        app.press_for_test(KeyCode::F(2), NONE).unwrap();
        let text = screen_text(&mut app, w, h);
        assert!(!text.contains('\u{fffd}'), "外观页在 {w}x{h} 撕屏了");
    }
}
