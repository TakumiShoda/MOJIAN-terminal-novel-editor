//! 树上多选：Space 勾选 → `s` 批量改状态、状态栏显示选中数。见 doc.md §6.2 [MUST]。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::model::ChapterStatus;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fx {
    dir: tempfile::TempDir,
    book: BookId,
    ch1: ChapterId,
    ch2: ChapterId,
}

fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch1 = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    let ch2 = store
        .create_chapter(book.id, vol, "第二章", Some(ch1))
        .unwrap();
    Fx {
        dir,
        book: book.id,
        ch1,
        ch2,
    }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.focus_tree_for_test();
        app
    }
    fn status(&self, ch: ChapterId) -> ChapterStatus {
        self.store()
            .load_book(self.book)
            .unwrap()
            .volumes
            .iter()
            .flat_map(|v| &v.chapters)
            .find(|c| c.id == ch)
            .unwrap()
            .status
    }
}

/// 没勾选：`s` 推进选中那一章的状态（草稿→已改）。
#[test]
fn s_advances_selected_chapter() {
    let f = setup();
    let mut app = f.app();
    // 光标在第一章上。
    assert_eq!(f.status(f.ch1), ChapterStatus::Draft);
    app.press_for_test(KeyCode::Char('s'), NONE).unwrap();
    assert_eq!(f.status(f.ch1), ChapterStatus::Revised, "该推进到已改");
    assert_eq!(f.status(f.ch2), ChapterStatus::Draft, "没选的不动");
}

/// 状态循环：草稿→已改→定稿→草稿。
#[test]
fn status_cycles_back_to_draft() {
    let f = setup();
    let mut app = f.app();
    for want in [
        ChapterStatus::Revised,
        ChapterStatus::Done,
        ChapterStatus::Draft,
    ] {
        app.press_for_test(KeyCode::Char('s'), NONE).unwrap();
        assert_eq!(f.status(f.ch1), want);
    }
}

/// 勾选两章后 `s`：两章都推进，各自 +1 步。
#[test]
fn s_batch_advances_all_checked() {
    let f = setup();
    let mut app = f.app();
    // 勾第一章，下移勾第二章。
    app.press_for_test(KeyCode::Char(' '), NONE).unwrap();
    app.press_for_test(KeyCode::Down, NONE).unwrap();
    app.press_for_test(KeyCode::Char(' '), NONE).unwrap();
    app.press_for_test(KeyCode::Char('s'), NONE).unwrap();

    assert_eq!(
        f.status(f.ch1),
        ChapterStatus::Revised,
        "勾选的第一章该推进"
    );
    assert_eq!(
        f.status(f.ch2),
        ChapterStatus::Revised,
        "勾选的第二章也该推进"
    );
}

/// 勾选后状态栏显示「选中 N 章」。
#[test]
fn status_bar_shows_selected_count() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char(' '), NONE).unwrap(); // 勾第一章

    // 按显示宽度读屏：CJK 占两格、第二格是空格，逐格拼会在汉字间塞空格搜不到。
    use unicode_width::UnicodeWidthStr as _;
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    term.draw(|fr| app.render_for_test(fr)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        let mut x = 0;
        while x < buf.area.width {
            let s = buf[(x, y)].symbol();
            screen.push_str(s);
            x += (s.width() as u16).max(1);
        }
        screen.push('\n');
    }
    assert!(
        screen.contains("选中 1 章"),
        "状态栏该显示选中数：\n{screen}"
    );
}

/// Esc 先清勾选，不直接跳回书架。
#[test]
fn esc_clears_checks_before_leaving() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char(' '), NONE).unwrap(); // 勾选
    app.press_for_test(KeyCode::Esc, NONE).unwrap(); // 该只清勾选
    // 还在工作区（没回书架）：再按 s 还能推进选中章。
    app.press_for_test(KeyCode::Char('s'), NONE).unwrap();
    assert_eq!(
        f.status(f.ch1),
        ChapterStatus::Revised,
        "Esc 只清了勾选、没离开工作区，s 仍作用于选中章"
    );
}
