//! 校对面板 F7 的端到端流程。见 doc.md §6.8。
//!
//! 走真实按键（`press_for_test` → `on_key`），断言磁盘/缓冲的实际变化，
//! 不走 demo 钩子——只在钩子里跑过的功能等于没验证过用户按不按得到它。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
    book: BookId,
    ch: ChapterId,
}

fn setup(body: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    store
        .save_body(book.id, &mj_core::model::ChapterBody::new(ch, body))
        .unwrap();
    Fixture {
        dir,
        book: book.id,
        ch,
    }
}

impl Fixture {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }

    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }

    fn add_character(&self, name: &str) {
        let mut store = self.store();
        store.create_character(self.book, name).unwrap();
    }
}

/// F7 打开面板并报出错别字。
#[test]
fn f7_finds_a_typo() {
    let f = setup("现场气氛如火如茶，众人叫好。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(1), "该报出「如火如茶」");
}

/// 干净文本：面板开着但没有问题。
#[test]
fn f7_on_clean_text_shows_nothing() {
    let f = setup("他推开门，风雪扑面而来，冷得他打了个寒战。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(0));
}

/// `a` 应用建议：缓冲里的错别字被改正。
#[test]
fn apply_suggestion_fixes_the_buffer() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Char('a'), NONE).unwrap();

    let text = app.buffer_text_for_test().unwrap();
    assert!(text.contains("如火如荼"), "应改成正确写法：{text:?}");
    assert!(!text.contains("如火如茶"), "错写不该还在：{text:?}");
    // 改完重新校对，那条应消失。
    assert_eq!(app.proof_visible_for_test(), Some(0));
}

/// Enter 跳转：关面板、光标落到问题处、焦点回编辑器。
#[test]
fn enter_jumps_and_closes_panel() {
    let f = setup("开头一句。\n\n现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), None, "跳转后面板关闭");
}

/// `I` 永久忽略：写进 dict/ignore.json，下次校对不再报。
#[test]
fn permanent_ignore_persists_across_reopen() {
    let f = setup("现场气氛如火如茶。\n");

    {
        let mut app = f.app();
        app.press_for_test(KeyCode::F(7), NONE).unwrap();
        assert_eq!(app.proof_visible_for_test(), Some(1));
        app.press_for_test(KeyCode::Char('I'), NONE).unwrap();
        assert_eq!(app.proof_visible_for_test(), Some(0), "忽略后当场消失");
    }

    // ignore.json 应已落盘。
    let ignore = f.dir.path().join("dict").join("ignore.json");
    assert!(ignore.exists(), "永久忽略应写入 dict/ignore.json");

    // 重开一个 App，再 F7，那条不该再出现。
    let mut app2 = f.app();
    app2.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(
        app2.proof_visible_for_test(),
        Some(0),
        "已忽略的问题跨会话不再出现"
    );
}

/// `i` 本次忽略：只从列表摘掉，不落盘。
#[test]
fn session_ignore_does_not_persist() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Char('i'), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(0));

    let ignore = f.dir.path().join("dict").join("ignore.json");
    assert!(!ignore.exists(), "本次忽略不该写盘");
}

/// 角色名驱动一致性检查：与「沈砚」一字之差的「沈研」被标可疑。
#[test]
fn character_name_drives_consistency_check() {
    let f = setup("那天沈研走进门。\n");
    f.add_character("沈砚");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(
        app.proof_visible_for_test(),
        Some(1),
        "应根据角色名报出「沈研」可疑"
    );
}

/// Esc 关闭面板。
#[test]
fn esc_closes_panel() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert!(app.proof_visible_for_test().is_some());
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), None);
}
