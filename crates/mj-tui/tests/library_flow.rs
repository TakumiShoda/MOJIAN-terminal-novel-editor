//! 书架管理：建书向导（`n` 书名→作者）、置顶 `p`、归档 `a`。见 doc.md §6.1 [MUST]。
//!
//! 走真实按键，断言磁盘/书架顺序。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fx {
    dir: tempfile::TempDir,
}

/// 起一个空工作区（书架为空，从零建书）。
fn empty() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    Fx { dir }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    fn app(&self) -> App {
        App::new(self.store(), Config::default()).unwrap()
    }
    fn book_titles(&self) -> Vec<String> {
        self.store()
            .list_books()
            .unwrap()
            .into_iter()
            .map(|b| b.title)
            .collect()
    }
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
}

/// `n` → 书名 → Enter → 作者 → Enter → 磁盘上出现一本带书名/作者的书。
#[test]
fn wizard_creates_a_named_book_with_author() {
    let f = empty();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap();
    typed(&mut app, "雪夜行");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    // 第二步：作者。预填「佚名」，全删了敲真名。
    for _ in 0..2 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    typed(&mut app, "沈砚");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    let b = &f.store().list_books().unwrap()[0];
    assert_eq!(b.title, "雪夜行", "书名应为向导里输入的");
    assert_eq!(b.author, "沈砚", "作者应为向导里输入的");
    // 总建第一卷第一章，新书直接能写。
    assert_eq!(b.volumes.len(), 1, "应自带第一卷");
    assert_eq!(b.volumes[0].chapters.len(), 1, "应自带第一章");
}

/// 作者留空 = 佚名（预填值直接回车）。
#[test]
fn wizard_default_author_is_anon() {
    let f = empty();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap();
    typed(&mut app, "无名书");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap(); // 作者用预填「佚名」

    assert_eq!(f.store().list_books().unwrap()[0].author, "佚名");
}

/// 书名为空不建书，也不进第二步。
#[test]
fn wizard_rejects_empty_title() {
    let f = empty();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap(); // 空书名
    assert!(f.book_titles().is_empty(), "空书名不该建出书");
    assert!(app.input_value_for_test().is_none(), "不该进作者那一步");
}

/// 第一步 Esc 取消，什么都不建。
#[test]
fn wizard_esc_cancels() {
    let f = empty();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap();
    typed(&mut app, "半途而废");
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(f.book_titles().is_empty(), "取消了不该建书");
}

/// 置顶：`p` 把书顶到最前。
///
/// 不假设中文的默认排序（Rust 按码点排，跟拼音/字典序不是一回事）——
/// 只按相对位置：把**末尾**那本置顶，它就该窜到最前。
#[test]
fn pin_sorts_book_to_top() {
    let f = empty();
    let mut s = f.store();
    s.create_book("甲书", "作者").unwrap();
    s.create_book("乙书", "作者").unwrap();

    let before = f.book_titles();
    let last = before[1].clone();

    let mut app = f.app();
    app.press_for_test(KeyCode::Down, NONE).unwrap(); // 移到末尾那本
    app.press_for_test(KeyCode::Char('p'), NONE).unwrap();

    assert_eq!(
        f.book_titles().first(),
        Some(&last),
        "置顶后末尾那本该到最前：{:?} → {:?}",
        before,
        f.book_titles()
    );
}

/// 归档：`a` 把书沉到最底。同样只看相对位置——把**最前**那本归档，它就该沉底。
#[test]
fn archive_sorts_book_to_bottom() {
    let f = empty();
    let mut s = f.store();
    s.create_book("甲书", "作者").unwrap();
    s.create_book("乙书", "作者").unwrap();

    let before = f.book_titles();
    let first = before[0].clone();

    let mut app = f.app();
    app.press_for_test(KeyCode::Char('a'), NONE).unwrap(); // 归档最前那本

    assert_eq!(
        f.book_titles().last(),
        Some(&first),
        "归档后最前那本该沉最底：{:?} → {:?}",
        before,
        f.book_titles()
    );
    // 归档不删——书还在。
    assert_eq!(f.book_titles().len(), 2);
}

/// `p`/`a` 是开关：再按一次取消。
#[test]
fn pin_toggles_off() {
    let f = empty();
    f.store().create_book("独一本", "作者").unwrap();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), NONE).unwrap();
    assert!(f.store().list_books().unwrap()[0].pinned);
    app.press_for_test(KeyCode::Char('p'), NONE).unwrap();
    assert!(
        !f.store().list_books().unwrap()[0].pinned,
        "再按一次该取消置顶"
    );
}
