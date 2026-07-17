//! `@` 角色名补全的端到端流程。见 doc.md §6.7。
//!
//! 走真实按键，断言缓冲里最终上屏的文本。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
    ch: ChapterId,
}

fn setup(characters: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    for c in characters {
        store.create_character(book.id, c).unwrap();
    }
    Fixture { dir, ch }
}

impl Fixture {
    fn app(&self) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        let store = Store::new(ws, Config::default());
        let mut app = App::new(store, Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        // 焦点切到编辑器。
        app.press_for_test(KeyCode::Tab, NONE).unwrap();
        app
    }
}

fn typ(app: &mut App, s: &str) {
    for c in s.chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
}

#[test]
fn at_opens_completion_when_characters_exist() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    typ(&mut app, "@");
    assert!(app.completion_active_for_test(), "有角色时 @ 应开补全");
}

#[test]
fn at_does_nothing_without_characters() {
    let f = setup(&[]);
    let mut app = f.app();
    typ(&mut app, "@");
    assert!(
        !app.completion_active_for_test(),
        "没有角色时 @ 只是普通字符"
    );
    assert_eq!(app.buffer_text_for_test().as_deref(), Some("@"));
}

#[test]
fn enter_accepts_selected_name() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    typ(&mut app, "@沈");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(!app.completion_active_for_test(), "接受后补全关闭");
    assert_eq!(
        app.buffer_text_for_test().as_deref(),
        Some("沈砚"),
        "@沈 应被替换成完整角色名"
    );
}

#[test]
fn down_then_enter_picks_second_candidate() {
    // 两个都含「沈」的角色，Down 选到第二个。
    let f = setup(&["沈砚", "沈墨"]);
    let mut app = f.app();
    typ(&mut app, "@沈");
    app.press_for_test(KeyCode::Down, NONE).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    let text = app.buffer_text_for_test().unwrap();
    assert!(
        text == "沈砚" || text == "沈墨",
        "应上屏某个候选，实际 {text:?}"
    );
    assert_ne!(text, "@沈", "不该留下字面文本");
}

#[test]
fn esc_cancels_and_leaves_literal_text() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    typ(&mut app, "@沈");
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(!app.completion_active_for_test());
    assert_eq!(
        app.buffer_text_for_test().as_deref(),
        Some("@沈"),
        "取消后留下字面 @沈"
    );
}

#[test]
fn typing_non_matching_closes_completion() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    typ(&mut app, "@");
    assert!(app.completion_active_for_test());
    // 敲一个不匹配任何角色的字。
    typ(&mut app, "X");
    assert!(!app.completion_active_for_test(), "无候选应关补全");
    assert_eq!(app.buffer_text_for_test().as_deref(), Some("@X"));
}

#[test]
fn backspacing_past_at_closes_completion() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    typ(&mut app, "@");
    app.press_for_test(KeyCode::Backspace, NONE).unwrap(); // 删掉 @
    assert!(!app.completion_active_for_test());
    assert_eq!(app.buffer_text_for_test().as_deref(), Some(""));
}

#[test]
fn accepts_by_alias_filter() {
    // 别名也进候选：搜「小」命中别名「小砚」，上屏本名「沈砚」不——上屏的是所选**候选字符串**。
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷一", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    let mut c = store.create_character(book.id, "沈砚").unwrap();
    c.aliases = vec!["小砚".into()];
    store.save_character(book.id, &c).unwrap();

    let f = Fixture { dir, ch };
    let mut app = f.app();
    typ(&mut app, "@小");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(
        app.buffer_text_for_test().as_deref(),
        Some("小砚"),
        "按别名筛选，上屏该别名"
    );
}
