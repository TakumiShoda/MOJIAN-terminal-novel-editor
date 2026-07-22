//! 删除的端到端流程：树上 `d`（卷/章，敲 y）/ 书架 `d`（书，输书名）。
//! 见 doc.md §6.1、§6.2、§0（可撤销）。
//!
//! 重点验**确认闸门**：确认串不符时绝不删。走真实按键，断言磁盘。
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
    ch2: ChapterId,
}

fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch1 = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    let ch2 = store
        .create_chapter(book.id, vol, "第二章", Some(ch1))
        .unwrap();
    Fx {
        dir,
        book: book.id,
        vol,
        ch1,
        ch2,
    }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    fn app_in_tree(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.focus_tree_for_test();
        app
    }
    fn app_in_shelf(&self) -> App {
        App::new(self.store(), Config::default()).unwrap()
    }
    /// 该章现在在不在树里（磁盘为准）。
    fn chapter_exists(&self, ch: ChapterId) -> bool {
        self.store()
            .load_book(self.book)
            .map(|b| {
                b.volumes
                    .iter()
                    .flat_map(|v| &v.chapters)
                    .any(|c| c.id == ch)
            })
            .unwrap_or(false)
    }
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
}

/// 树上选中章 → `d` → 敲 y → Enter → 章进 trash，别的章不动。
#[test]
fn delete_chapter_with_y() {
    let f = setup();
    let mut app = f.app_in_tree();
    // 光标在第一章上。
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    assert!(app.input_value_for_test().is_some(), "该弹确认框");
    typed(&mut app, "y");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    assert!(!f.chapter_exists(f.ch1), "确认后该章应删除");
    assert!(f.chapter_exists(f.ch2), "别的章不该被牵连");
    // §0：进 trash，不是真删。
    let trashed = f
        .dir
        .path()
        .join("books")
        .join(f.book.to_string())
        .join("trash")
        .join("chapters")
        .join(format!("{}.md", f.ch1));
    assert!(trashed.exists(), "删掉的章应进 trash");
}

/// 敲的不是 y（比如手滑敲了 n）→ 不删。
#[test]
fn delete_chapter_cancelled_by_wrong_key() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    typed(&mut app, "n");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(f.chapter_exists(f.ch1), "没敲 y 就不该删");
}

/// Esc 取消删除。
#[test]
fn delete_chapter_esc_cancels() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    typed(&mut app, "y");
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(f.chapter_exists(f.ch1), "Esc 了就不该删");
}

/// 删的是打开着的那一章：编辑器要合上，不能对着 trash 里的文件。
#[test]
fn deleting_the_open_chapter_closes_the_editor() {
    let f = setup();
    let mut app = f.app_in_tree();
    // 开书默认打开首章 = ch1，光标也在 ch1 上。
    assert_eq!(app.current_chapter_for_test(), Some(f.ch1));
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    typed(&mut app, "y");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(
        app.current_chapter_for_test(),
        None,
        "删掉正打开的章后，编辑器应合上"
    );
}

/// 删卷：连里面的章一起进 trash。
#[test]
fn delete_volume_with_y() {
    let f = setup();
    let mut app = f.app_in_tree();
    app.press_for_test(KeyCode::Up, NONE).unwrap(); // 到卷
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    typed(&mut app, "y");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    assert!(b.volumes.is_empty(), "卷删了树上不该还有");
    let trashed = f
        .dir
        .path()
        .join("books")
        .join(f.book.to_string())
        .join("trash")
        .join("volumes")
        .join(f.vol.to_string());
    assert!(trashed.exists(), "整卷应进 trash");
}

/// 书架删书：§6.1 [MUST] 必须输入**完整书名**才删。
#[test]
fn delete_book_requires_exact_title() {
    let f = setup();

    // 敲错的名字：不删。
    {
        let mut app = f.app_in_shelf();
        app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
        typed(&mut app, "雪夜");
        app.press_for_test(KeyCode::Enter, NONE).unwrap();
        assert!(
            f.store()
                .list_books()
                .unwrap()
                .iter()
                .any(|b| b.id == f.book),
            "书名不全就不该删"
        );
        let t = app.toast_for_test().unwrap_or("");
        assert!(t.contains("书名不符"), "要提示书名不符：{t}");
    }

    // 敲对完整书名：删。
    {
        let mut app = f.app_in_shelf();
        app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
        typed(&mut app, "雪夜行");
        app.press_for_test(KeyCode::Enter, NONE).unwrap();
        assert!(
            !f.store()
                .list_books()
                .unwrap()
                .iter()
                .any(|b| b.id == f.book),
            "输对书名应删除"
        );
        // §0：整本进工作区 trash。
        let trashed = Workspace::resolve(Some(f.dir.path().to_path_buf()))
            .unwrap()
            .trash_dir()
            .join("books")
            .join(f.book.to_string());
        assert!(trashed.exists(), "删掉的书应整本进 trash");
    }
}

/// 敲 y 想删书是不行的——书必须输全名（y 不等于书名）。
#[test]
fn typing_y_does_not_delete_a_book() {
    let f = setup();
    let mut app = f.app_in_shelf();
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    typed(&mut app, "y");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(
        f.store()
            .list_books()
            .unwrap()
            .iter()
            .any(|b| b.id == f.book),
        "y 不是书名，不该删书"
    );
}
