//! 改名的端到端流程：树上 `r` / 书架 `r` → 输入框 → 改盘。见 doc.md §6.1、§6.2。
//!
//! 走真实按键（`press_for_test` → `on_key`），断言磁盘上的实际变化——
//! 只在内部函数里验等于没验用户按不按得到它。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId, VolumeId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fx {
    dir: tempfile::TempDir,
    book: BookId,
    vol: VolumeId,
    ch1: ChapterId,
}

fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch1 = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    store
        .create_chapter(book.id, vol, "第二章", Some(ch1))
        .unwrap();
    Fx {
        dir,
        book: book.id,
        vol,
        ch1,
    }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    /// 打开到工作区（书树可见），焦点在树上。
    fn app_in_tree(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.focus_tree_for_test();
        app
    }
    /// 停在书架。
    fn app_in_shelf(&self) -> App {
        App::new(self.store(), Config::default()).unwrap()
    }
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
}

/// 树上选中章 → `r` → 预填原名 → 改 → Enter → 磁盘上标题变了、id 不变。
///
/// 开书默认打开首章、树光标停在第一章上，故直接 `r` 就是给第一章改名。
#[test]
fn rename_chapter_from_tree() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    assert_eq!(
        app.input_value_for_test().as_deref(),
        Some("第一章"),
        "改名要预填原名"
    );

    // 全删掉，敲新名。
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    typed(&mut app, "楔子");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    assert!(app.input_value_for_test().is_none(), "确认后输入框该关");
    // 磁盘为准：重开 Store 看。
    let b = f.store().load_book(f.book).unwrap();
    let ch = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch1)
        .unwrap();
    assert_eq!(ch.title, "楔子", "章名应已改到盘上");
    assert_eq!(ch.id, f.ch1, "id 不能变");
}

/// 树上选中卷 → `r` → 改名。
#[test]
fn rename_volume_from_tree() {
    let f = setup();
    let mut app = f.app_in_tree();
    // 光标在第一章上，往上一格到它所属的卷。
    app.press_for_test(KeyCode::Up, NONE).unwrap();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    assert_eq!(app.input_value_for_test().as_deref(), Some("第一卷"));
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    typed(&mut app, "序卷");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let v = b.volumes.iter().find(|v| v.id == f.vol).unwrap();
    assert_eq!(v.title, "序卷");
    assert_eq!(v.chapters.len(), 2, "改名不该弄丢章");
}

/// 书架上 `r` 给书改名。
#[test]
fn rename_book_from_shelf() {
    let f = setup();
    let mut app = f.app_in_shelf();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    assert_eq!(app.input_value_for_test().as_deref(), Some("雪夜行"));
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    typed(&mut app, "归途");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    assert_eq!(b.title, "归途");
    assert_eq!(b.author, "沈砚", "改书名不该动作者");
}

/// Esc 取消：一个字都不该落盘。
#[test]
fn esc_cancels_without_writing() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    typed(&mut app, "乱改的");
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(app.input_value_for_test().is_none(), "Esc 后输入框该关");

    let b = f.store().load_book(f.book).unwrap();
    let ch = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch1)
        .unwrap();
    assert_eq!(ch.title, "第一章", "取消了就不该改盘");
}

/// 空名字不改：清空后 Enter，保持原名并提示。
#[test]
fn empty_name_is_rejected() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let ch = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch1)
        .unwrap();
    assert_eq!(ch.title, "第一章", "空名字不该覆盖原名");
}

/// 输入框活着时，`d`/`j`/`r` 都是往名字里打的字，不触发别的动作。
#[test]
fn keys_go_into_the_box_not_the_tree() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    // 这几个键平时是删除/移动/导航，此刻都该只是字符。
    typed(&mut app, "djr");
    assert_eq!(
        app.input_value_for_test().as_deref(),
        Some("djr"),
        "输入框活着时这些键应作为字符进框"
    );
    // 树没被这些键搅动：两章都还在。
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    let b = f.store().load_book(f.book).unwrap();
    assert_eq!(b.volumes[0].chapters.len(), 2);
}

/// 改完名，内存里的书树立刻反映新名（不必重开）。
#[test]
fn in_memory_tree_updates_after_rename() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    for _ in 0..3 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    typed(&mut app, "新名字");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(
        app.tree_titles_for_test().iter().any(|t| t == "新名字"),
        "改完名内存里的树就该更新：{:?}",
        app.tree_titles_for_test()
    );
}
