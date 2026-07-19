//! 命令面板与帮助页。见 doc.md §7.3。
//!
//! §7.3 把命令面板标成「最重要的一条」：所有功能都必须能从这里触达。
//! 故这里最要紧的一条测试是 `every_command_actually_runs`——
//! 命令表里有、点了却没反应的命令，比不在表里更糟。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use mj_tui::commands::{COMMANDS, Command};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
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

fn typ(app: &mut App, s: &str) {
    for c in s.chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
}

#[test]
fn ctrl_p_opens_palette() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Palette"]);
}

#[test]
fn esc_closes_palette() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(app.modal_stack_for_test().is_empty());
}

/// 从面板里筛出命令并执行：面板先关，命令自己开的浮层留下。
#[test]
fn palette_runs_the_selected_command() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
    typ(&mut app, "校对");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(
        app.modal_stack_for_test(),
        vec!["Proof"],
        "面板应先关掉，只留命令打开的校对面板"
    );
}

/// 敲键位也能找到命令（记得住键的人不必再想名字）。
#[test]
fn palette_finds_command_by_keybinding() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
    typ(&mut app, "F3");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Stats"]);
}

#[test]
fn palette_with_no_match_does_nothing_on_enter() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
    typ(&mut app, "查无此命令");
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(
        app.modal_stack_for_test().is_empty(),
        "面板关掉，不执行任何命令"
    );
}

/// **命令表里的每一条都必须真的能跑。**
///
/// 表里有而 run_command 没分支（或分支是空壳）的命令，在用户看来就是
/// 「点了没反应」——那比不在表里更糟。这里逐条执行一遍，要求不报错、不 panic。
#[test]
fn every_command_actually_runs() {
    for spec in COMMANDS {
        let f = setup();
        let mut app = f.app();
        // Quit 会置退出标志；单独验证它的效果，其余只要求跑通。
        let r = app.run_command(spec.cmd);
        assert!(
            r.is_ok(),
            "命令「{}」执行失败：{:?}",
            spec.name,
            r.err().map(|e| e.to_string())
        );
    }
}

#[test]
fn quit_command_sets_the_flag() {
    let f = setup();
    let mut app = f.app();
    assert!(!app.should_quit_for_test());
    app.run_command(Command::Quit).unwrap();
    assert!(app.should_quit_for_test(), "退出命令应置退出标志");
}

#[test]
fn new_chapter_command_creates_one_on_disk() {
    let f = setup();
    let mut app = f.app();
    app.run_command(Command::NewChapter).unwrap();

    let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
    let store = Store::new(ws, Config::default());
    let book = store.list_books().unwrap().remove(0);
    let n: usize = book.volumes.iter().map(|v| v.chapters.len()).sum();
    assert_eq!(n, 2, "应在磁盘上多出一章");
}

/// 上/下一章按**全书阅读顺序**跨卷走——读者读的是一条线。
///
/// 这两条对应 §7.3 的 Ctrl+Tab，那个键位要终端支持 kitty 协议才到得了程序；
/// 命令本身在任何终端下都能从命令面板触达，故这里直接测命令。
#[test]
fn chapter_navigation_crosses_volumes() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("书", "作者").unwrap();
    let v1 = store.create_volume(book.id, "第一卷", None).unwrap();
    let c1 = store.create_chapter(book.id, v1, "一", None).unwrap();
    let c2 = store.create_chapter(book.id, v1, "二", Some(c1)).unwrap();
    let v2 = store.create_volume(book.id, "第二卷", Some(v1)).unwrap();
    let c3 = store.create_chapter(book.id, v2, "三", None).unwrap();

    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    let mut app = App::new(Store::new(ws, Config::default()), Config::default()).unwrap();
    app.open_first_book_for_demo().unwrap();
    app.open_chapter_for_test(c1).unwrap();

    app.run_command(Command::NextChapter).unwrap();
    assert_eq!(app.current_chapter_for_test(), Some(c2));

    // 卷末再往后，应当跨进下一卷。
    app.run_command(Command::NextChapter).unwrap();
    assert_eq!(app.current_chapter_for_test(), Some(c3), "该跨到第二卷");

    // 最后一章再往后：停住并提示，不该绕回开头。
    app.run_command(Command::NextChapter).unwrap();
    assert_eq!(app.current_chapter_for_test(), Some(c3));

    app.run_command(Command::PrevChapter).unwrap();
    assert_eq!(app.current_chapter_for_test(), Some(c2), "往回也要跨卷");
}

#[test]
fn toggle_tree_command_flips_it() {
    let f = setup();
    let mut app = f.app();
    let before = app.show_tree_for_test();
    app.run_command(Command::ToggleTree).unwrap();
    assert_ne!(app.show_tree_for_test(), before);
}

// ---- 帮助页 ----

#[test]
fn f1_opens_help() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(1), NONE).unwrap();
    assert_eq!(app.modal_stack_for_test(), vec!["Help"]);
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert!(app.modal_stack_for_test().is_empty());
}

/// 把屏幕读回成文本。
///
/// 全角字占两格：ratatui 把字放在头一格，第二格是**占位空格**。天真地把每格
/// 符号拼起来会得到「保 存」，于是搜「保存」永远搜不到。按显示宽度跳过占位格。
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

/// 帮助页上必须能看到每条命令的名字——它是从命令表生成的，理应如此，
/// 这里从**渲染出来的屏幕**上验证，而不是只验证数据结构。
#[test]
fn help_screen_shows_command_names() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(1), NONE).unwrap();
    // 高一点，好让整页一屏放下。
    let text = screen_text(&mut app, 120, 60);

    for spec in COMMANDS {
        assert!(
            text.contains(spec.name),
            "帮助页上看不到命令「{}」",
            spec.name
        );
    }
}

/// 帮助页给出的键位必须是**真能按的**那个。
///
/// §7.3 的注：Ctrl+Shift+S 在传统键盘模式下根本到不了，实际入口是 F9。
/// 帮助页要是写 Ctrl+Shift+S，用户按了没反应只会以为功能坏了。
#[test]
fn help_shows_reachable_snapshot_key() {
    let f = setup();
    let mut app = f.app();
    app.press_for_test(KeyCode::F(1), NONE).unwrap();
    let text = screen_text(&mut app, 120, 60);
    assert!(text.contains("F9"), "打快照的键位该写 F9");
    assert!(
        !text.contains("Ctrl+Shift+S"),
        "帮助页不该给一个按不出来的键"
    );
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
fn palette_and_help_render_across_widths() {
    // §7.2 [MUST]：窄至 60 列不崩。
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let f = setup();
        let mut app = f.app();
        app.press_for_test(KeyCode::Char('p'), CTRL).unwrap();
        assert!(draw_ok(&mut app, w, h), "命令面板在 {w}x{h} 撕屏了");

        let mut app2 = f.app();
        app2.press_for_test(KeyCode::F(1), NONE).unwrap();
        assert!(draw_ok(&mut app2, w, h), "帮助页在 {w}x{h} 撕屏了");
    }
}
