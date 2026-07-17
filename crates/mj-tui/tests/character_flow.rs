//! 角色速查侧栏 Alt+C 的端到端流程。见 doc.md §6.7。
//!
//! 走真实按键，断言磁盘上的角色卡实际增删。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::BookId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;
const ALT: KeyModifiers = KeyModifiers::ALT;

struct Fixture {
    dir: tempfile::TempDir,
    book: BookId,
}

fn setup(names: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    store.create_chapter(book.id, vol, "第一章", None).unwrap();
    for n in names {
        store.create_character(book.id, n).unwrap();
    }
    Fixture { dir, book: book.id }
}

impl Fixture {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }

    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app
    }

    fn character_count(&self) -> usize {
        self.store().list_characters(self.book).unwrap().len()
    }
}

fn open_panel(app: &mut App) {
    app.press_for_test(KeyCode::Char('c'), ALT).unwrap();
}

#[test]
fn alt_c_opens_and_lists_characters() {
    let f = setup(&["沈砚", "苏妲己", "周暮"]);
    let mut app = f.app();
    open_panel(&mut app);
    assert_eq!(app.character_filtered_for_test(), Some(3));
}

#[test]
fn search_filters_the_list() {
    let f = setup(&["沈砚", "苏妲己", "周暮"]);
    let mut app = f.app();
    open_panel(&mut app);
    app.press_for_test(KeyCode::Char('/'), NONE).unwrap();
    app.press_for_test(KeyCode::Char('周'), NONE).unwrap();
    assert_eq!(app.character_filtered_for_test(), Some(1));
    assert_eq!(
        app.character_current_name_for_test().as_deref(),
        Some("周暮")
    );
}

#[test]
fn n_creates_a_character_on_disk() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    open_panel(&mut app);
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap();
    assert_eq!(f.character_count(), 2, "磁盘上应多一张卡");
    assert_eq!(app.character_filtered_for_test(), Some(2), "面板应刷新");
}

#[test]
fn d_deletes_to_trash() {
    let f = setup(&["沈砚", "路人甲"]);
    let mut app = f.app();
    open_panel(&mut app);
    // 光标在首个（按名排序：沈砚 在 路人甲 前？以 char 序为准，断言删了一个即可）。
    let before = app.character_current_name_for_test().unwrap();
    app.press_for_test(KeyCode::Char('d'), NONE).unwrap();
    assert_eq!(f.character_count(), 1, "磁盘上应少一张卡");

    // 被删的那张进了 trash（§0 可撤销）。
    let trash = f
        .dir
        .path()
        .join("books")
        .join(f.book.to_string())
        .join("trash")
        .join("characters");
    let count = std::fs::read_dir(&trash).map(|d| d.count()).unwrap_or(0);
    assert_eq!(count, 1, "删掉的 {before} 应在 trash 里");
}

#[test]
fn typing_c_in_search_does_not_leak_to_new() {
    // 搜索模式下按 n 应作为搜索字符，不触发新建。
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    open_panel(&mut app);
    app.press_for_test(KeyCode::Char('/'), NONE).unwrap();
    app.press_for_test(KeyCode::Char('n'), NONE).unwrap(); // 作为搜索输入
    assert_eq!(f.character_count(), 1, "搜索里的 n 不该新建角色");
}

#[test]
fn esc_closes_panel() {
    let f = setup(&["沈砚"]);
    let mut app = f.app();
    open_panel(&mut app);
    assert!(app.character_filtered_for_test().is_some());
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(app.character_filtered_for_test(), None);
}

fn draw_ok(app: &mut App, w: u16, h: u16) -> bool {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    !text.contains('\u{fffd}')
}

#[test]
fn panel_renders_across_widths() {
    let f = setup(&["沈砚", "苏妲己"]);
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let mut app = f.app();
        open_panel(&mut app);
        assert!(draw_ok(&mut app, w, h), "角色面板在 {w}x{h} 撕屏了");
    }
}
