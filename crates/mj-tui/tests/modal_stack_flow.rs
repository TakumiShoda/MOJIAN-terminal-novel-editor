//! 浮层栈的分层语义。见 doc.md §7.1。
//!
//! 栈存在的理由就是「确认框叠在查找面板上」这种真·两层场景：
//! Esc 应当**逐层弹出**——先关确认框、露出底下的查找面板，而不是一把全关掉。
//! 从前那条手写死的优先级链（先查 confirm、再 diff、再 history……）碰巧也能
//! 跑对这个用例，但它把 z 序编码进了 if 的顺序；这里把行为钉死，
//! 免得以后加浮层时又悄悄改回去。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

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
    store
        .save_body(
            book.id,
            &mj_core::model::ChapterBody::new(ch, "他推开门，风雪扑面。\n"),
        )
        .unwrap();
    // 再来一章，好让「全书」范围有多章可替换。
    store
        .create_chapter(book.id, vol, "第二章", Some(ch))
        .unwrap();
    Fixture { dir, ch }
}

impl Fixture {
    fn app(&self) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        let store = Store::new(ws, Config::default());
        let mut app = App::new(store, Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }
}

fn stack(app: &App) -> Vec<String> {
    app.modal_stack_for_test()
}

#[test]
fn no_modal_at_rest() {
    let f = setup();
    let app = f.app();
    assert!(stack(&app).is_empty(), "静息态不该有浮层");
}

#[test]
fn opening_a_panel_pushes_one_layer() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap(); // 校对面板
    assert_eq!(stack(&app), vec!["Proof"]);
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(stack(&app).is_empty(), "Esc 关掉后栈空");
}

/// 核心用例：确认框叠在查找面板之上，Esc 逐层弹出。
#[test]
fn confirm_stacks_over_search_and_esc_pops_one_layer() {
    let f = setup();
    let mut app = f.app();

    // Ctrl+H 开查找替换。
    app.press_for_test(KeyCode::Char('h'), CTRL).unwrap();
    assert_eq!(stack(&app), vec!["Search"]);

    // 输入查找串。
    for c in "风雪".chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
    // F4 把范围切到「全书」（当前章 → 当前卷 → 全书）。
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    app.press_for_test(KeyCode::F(4), NONE).unwrap();

    // Alt+A：宽范围替换要先弹确认框，它叠在查找面板上。
    app.press_for_test(KeyCode::Char('a'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(
        stack(&app),
        vec!["Search", "Confirm"],
        "确认框应压在查找面板之上，而不是取代它"
    );

    // Esc：只弹掉最上面那层。
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(
        stack(&app),
        vec!["Search"],
        "Esc 应逐层弹出——露出底下的查找面板，不是一把全关"
    );

    // 再 Esc 才关掉查找面板。
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(stack(&app).is_empty());
}

/// 栈顶那层吃键：确认框开着时，按键不该漏到底下的查找面板去。
#[test]
fn top_layer_consumes_keys() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('h'), CTRL).unwrap();
    for c in "风雪".chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    app.press_for_test(KeyCode::Char('a'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(stack(&app), vec!["Search", "Confirm"]);

    // 在确认框上敲字：不该被查找面板当成输入。
    app.press_for_test(KeyCode::Char('x'), NONE).unwrap();
    assert_eq!(stack(&app), vec!["Search", "Confirm"], "栈形状不该变");
}

/// 角色面板 → e 进表单：表单压在列表之上，Esc 回列表。
#[test]
fn character_form_stacks_over_list() {
    let f = setup();
    // 先建一个角色，列表才有内容可编辑。
    {
        let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
        let mut store = Store::new(ws, Config::default());
        let book = store.list_books().unwrap().remove(0);
        store.create_character(book.id, "沈砚").unwrap();
    }
    let mut app = f.app();

    app.press_for_test(KeyCode::Char('c'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(stack(&app), vec!["Characters"]);

    app.press_for_test(KeyCode::Char('e'), NONE).unwrap();
    assert_eq!(
        stack(&app),
        vec!["Characters", "CharacterForm"],
        "表单叠在列表上"
    );

    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(stack(&app), vec!["Characters"], "Esc 回到列表");
}

/// 历史面板 → Enter 进 diff：diff 叠在历史之上，Esc 回历史。
#[test]
fn diff_stacks_over_history() {
    let f = setup();
    let mut app = f.app();
    // 先打个快照，历史面板才有内容。
    app.press_for_test(KeyCode::F(9), NONE).unwrap();
    app.press_for_test(KeyCode::F(8), NONE).unwrap();
    assert_eq!(stack(&app), vec!["History"]);

    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(stack(&app), vec!["History", "Diff"], "diff 叠在历史上");

    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(stack(&app), vec!["History"], "Esc 回到历史面板");
}
