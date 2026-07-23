//! Bracketed paste：粘贴整段一次到手。见 doc.md §2.3 [MUST]。
//!
//! 关键不是「能粘」，而是「一次事件、一次插入」——否则粘 3000 字触发 3000 次重排。
//! 这里从行为侧验：一次 paste 后正文正确、且 auto-pair 不逐字符掺进来。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fx {
    dir: tempfile::TempDir,
    ch: ChapterId,
}

fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();
    Fx { dir, ch }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    /// 打开到章、焦点切到编辑器（正常要在树里回车才切过去）。
    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app.focus_editor_for_test();
        app
    }
}

/// 粘一整段中文，缓冲里应一字不差地出现它。
#[test]
fn paste_inserts_the_whole_text() {
    let f = setup();
    let mut app = f.app();
    let text = "他推开门，风雪扑面而来，冷得他打了个寒战。";
    app.paste_for_test(text);
    let buf = app.buffer_text_for_test().unwrap();
    assert!(buf.contains(text), "粘贴内容应整段进缓冲：{buf:?}");
}

/// 粘贴含成对引号时**不触发 auto-pair**——auto-pair 是给逐字敲用的。
/// 粘 `「` 不该变成 `「」`，否则粘来的原文就被篡改了。
#[test]
fn paste_does_not_trigger_auto_pairing() {
    let f = setup();
    let mut app = f.app();
    // 开着 auto_pair（默认开）。逐字敲「会补」」，但粘贴不该。
    app.paste_for_test("他说「你好」，然后走了");
    let buf = app.buffer_text_for_test().unwrap();
    // 原文只有一对引号；若 auto-pair 掺了手，会多出闭引号。
    assert_eq!(buf.matches('「').count(), 1, "粘贴不该多补开引号：{buf:?}");
    assert_eq!(buf.matches('」').count(), 1, "粘贴不该多补闭引号：{buf:?}");
}

/// 粘一大段（3000 字），一次事件搞定、内容完整——这正是 [MUST] 要防的场景。
#[test]
fn large_paste_lands_intact_in_one_shot() {
    let f = setup();
    let mut app = f.app();
    let big: String = "字".repeat(3000);
    app.paste_for_test(&big);
    let buf = app.buffer_text_for_test().unwrap();
    assert_eq!(buf.matches('字').count(), 3000, "3000 字应完整落入");
}

/// 多行粘贴保留换行（粘一段带分段的稿子）。
#[test]
fn paste_preserves_newlines() {
    let f = setup();
    let mut app = f.app();
    app.paste_for_test("第一段。\n\n第二段。");
    let buf = app.buffer_text_for_test().unwrap();
    assert!(
        buf.contains("第一段。\n\n第二段。"),
        "多行结构要保住：{buf:?}"
    );
}

/// 焦点在目录树时，粘贴不该往正文里灌字。
#[test]
fn paste_ignored_when_tree_focused() {
    let f = setup();
    let mut app = f.app();
    app.focus_tree_for_test();
    let before = app.buffer_text_for_test().unwrap_or_default();
    app.paste_for_test("不该进正文");
    assert_eq!(
        app.buffer_text_for_test().unwrap_or_default(),
        before,
        "树聚焦时粘贴不该改正文"
    );
}

/// 改名输入框活着时，粘贴进的是名字（去掉换行），不进正文。
#[test]
fn paste_goes_into_the_input_box_when_open() {
    let f = setup();
    let mut app = f.app();
    app.focus_tree_for_test();
    // 光标在章上，r 改名。
    app.press_for_test(KeyCode::Char('r'), NONE).unwrap();
    for _ in 0..1 {
        app.press_for_test(KeyCode::Backspace, NONE).unwrap();
    }
    app.paste_for_test("新章名\n带换行");
    // 换行被去掉，粘进名字框。
    assert_eq!(
        app.input_value_for_test().as_deref(),
        Some("新章名带换行"),
        "粘进名字框、去掉换行"
    );
}
