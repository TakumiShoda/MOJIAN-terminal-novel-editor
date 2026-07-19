//! 键位重绑定真的生效吗。见 doc.md §7.3。
//!
//! `[MUST]` 键位全部可在 `[keymap]` 里重绑定 + 冲突检测。
//! 单测证的是键位表本身；这里证**按下去真有反应**——从前那串写死的
//! `KeyCode::F(7) => ...` 会让配置形同虚设，而那种坏法单测看不出来。
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
    /// 写一份带 `[keymap]` 的配置再开 App。
    fn app_with_keymap(&self, keymap_toml: &str) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        std::fs::write(ws.config_file(), format!("[keymap]\n{keymap_toml}\n")).unwrap();
        let config = Config::load(&ws.config_file()).unwrap();
        let store = Store::new(ws, config.clone());
        let mut app = App::new(store, config).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }
}

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

/// 默认键位照常生效。
#[test]
fn default_bindings_still_work() {
    let f = setup();
    let mut app = f.app_with_keymap("");
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Proof"]);
}

/// §7.3 [MUST]：重绑定生效——新键能用，老键失效。
#[test]
fn rebinding_takes_effect() {
    let f = setup();
    let mut app = f.app_with_keymap("proof = \"F6\"");

    app.press_for_test(KeyCode::F(6), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Proof"], "新键应打开校对");

    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert!(
        app.modal_stack_for_test().is_empty(),
        "老键位应当失效，否则重绑定只是加了一个键"
    );
}

/// 带修饰键的重绑定。
#[test]
fn rebinding_with_modifiers() {
    let f = setup();
    let mut app = f.app_with_keymap("characters = \"Ctrl+G\"");
    app.press_for_test(KeyCode::Char('g'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Characters"]);
}

/// §7.3 [MUST]：冲突要**报警**——只写日志等于没报，用户不会去翻日志。
#[test]
fn conflict_is_surfaced_to_the_user() {
    let f = setup();
    let mut app = f.app_with_keymap("proof = \"F6\"\nhistory = \"F6\"");
    let text = screen_text(&mut app, 120, 24);
    assert!(text.contains("F6"), "首屏该提示键位冲突：{text}");

    // 且两者都退回默认键位。
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Proof"], "校对回到 F7");
}

/// 帮助页要印**当前生效的**键位，不能还印默认值——那是一张骗人的表。
#[test]
fn help_shows_the_rebound_key() {
    let f = setup();
    let mut app = f.app_with_keymap("proof = \"F6\"");
    app.press_for_test(KeyCode::F(1), NONE).unwrap();
    let text = screen_text(&mut app, 120, 60);

    // 找到「校对当前章」那一行，确认它写的是 F6。
    let line = text
        .lines()
        .find(|l| l.contains("校对当前章"))
        .expect("帮助页该有校对这一条");
    assert!(line.contains("F6"), "帮助页该印当前键位：{line}");
    assert!(!line.contains("F7"), "不该还印默认键位：{line}");
}

/// 配置写错不该让程序打不开——退回默认，并提示。
#[test]
fn bad_binding_falls_back_and_still_runs() {
    let f = setup();
    let mut app = f.app_with_keymap("proof = \"Hyper+Q\"");
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Proof"], "该退回默认键位");
}

/// 命令面板的入口不进键位表——它是通往所有命令的路，绑没了就找不回其余命令了。
#[test]
fn palette_entry_cannot_be_rebound_away() {
    let f = setup();
    // 就算把别的命令绑到 Ctrl+P，命令面板也得还能开。
    let mut app = f.app_with_keymap("stats = \"Ctrl+P\"");
    app.press_for_test(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        app.modal_stack_for_test(),
        vec!["Palette"],
        "Ctrl+P 必须始终能打开命令面板"
    );
}
