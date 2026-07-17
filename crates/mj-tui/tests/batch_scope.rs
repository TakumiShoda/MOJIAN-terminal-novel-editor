//! 全卷/全书范围的排版与替换。见 doc.md §6.5、§6.6。
//!
//! 全程走**真实按键**（`press_for_test` → `on_key`），不走 demo 钩子：
//! 只在钩子里跑过的功能等于没验证过用户按不按得到它。
//!
//! 断言落在**磁盘上的正文**——批量作业的全部意义就是把字改到文件里去，
//! 内存里对不对不作数。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const ALT: KeyModifiers = KeyModifiers::ALT;
const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
    book: BookId,
    /// 第一卷两章、第二卷一章。
    v1: Vec<ChapterId>,
    v2: Vec<ChapterId>,
}

/// 三章，各含一处「雪」。
fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());

    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol1 = store.create_volume(book.id, "第一卷 风起", None).unwrap();
    let vol2 = store.create_volume(book.id, "第二卷 雪落", None).unwrap();

    let mut mk = |vol, title: &str, after: Option<ChapterId>, body: &str| {
        let ch = store.create_chapter(book.id, vol, title, after).unwrap();
        store
            .save_body(book.id, &mj_core::model::ChapterBody::new(ch, body))
            .unwrap();
        ch
    };
    let a = mk(vol1, "第一章 雪夜", None, "　　雪落了一夜。\n");
    let b = mk(vol1, "第二章 相遇", Some(a), "　　雪停了。\n");
    let c = mk(vol2, "第三章 远行", None, "　　雪又下了起来。\n");

    Fixture {
        dir,
        book: book.id,
        v1: vec![a, b],
        v2: vec![c],
    }
}

impl Fixture {
    /// 每次都从磁盘重开一个 Store。
    ///
    /// 不是为了绕过 `Store: !Clone`——而是因为断言的对象本来就该是**磁盘上的字**。
    /// 复用同一个 Store 有可能读到内存里的残影，那就测不出「到底存进去没有」。
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }

    /// 打开书，并把编辑焦点停在**第一卷第一章**——「当前卷」范围据此判定。
    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.v1[0]).unwrap();
        app
    }

    fn body(&self, ch: ChapterId) -> String {
        self.store()
            .load_body(self.book, ch)
            .unwrap()
            .text
            .to_string()
    }
}

/// 打开查找替换、设好查找串与替换串、把范围切到 `scope`。
fn open_replace(app: &mut App, query: &str, to: &str, scope: mj_tui::batch::Scope) {
    app.open_search_for_demo(true, query);
    app.set_replace_text_for_test(to);
    // 有界：面板没开时 search_scope_for_test 返回 None，无界 while 会挂死。
    for _ in 0..3 {
        if app.search_scope_for_test() == Some(scope) {
            return;
        }
        app.press_for_test(KeyCode::F(4), NONE).unwrap();
    }
    assert_eq!(
        app.search_scope_for_test(),
        Some(scope),
        "F4 没能把范围切过去"
    );
}

/// F4 真的能把范围切到全书——键位本身也要测。
#[test]
fn f4_cycles_scope() {
    let f = setup();
    let mut app = f.app();
    app.open_search_for_demo(true, "雪");

    use mj_tui::batch::Scope;
    assert_eq!(
        app.search_scope_for_test(),
        Some(Scope::Chapter),
        "默认当前章"
    );
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    assert_eq!(app.search_scope_for_test(), Some(Scope::Volume));
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    assert_eq!(app.search_scope_for_test(), Some(Scope::Book));
    app.press_for_test(KeyCode::F(4), NONE).unwrap();
    assert_eq!(
        app.search_scope_for_test(),
        Some(Scope::Chapter),
        "循环回去"
    );
}

/// 全书替换：三章都得改到，而且是改在**磁盘上**。
#[test]
fn book_scope_replace_touches_every_chapter() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);

    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap(); // 确认框
    app.drain_batch_for_test().unwrap();

    for ch in f.v1.iter().chain(&f.v2) {
        let body = f.body(*ch);
        assert!(body.contains('霜'), "该章没被替换：{body:?}");
        assert!(!body.contains('雪'), "该章还留着旧字：{body:?}");
    }
}

/// 当前卷范围：第一卷的两章改，第二卷那章**不许动**。
#[test]
fn volume_scope_leaves_other_volumes_alone() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Volume);

    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap();
    app.drain_batch_for_test().unwrap();

    for ch in &f.v1 {
        assert!(f.body(*ch).contains('霜'), "本卷的章该改");
    }
    assert!(
        f.body(f.v2[0]).contains('雪'),
        "别的卷不该被动到：{:?}",
        f.body(f.v2[0])
    );
}

/// §0：破坏性操作要能退回去。§6.6 [MUST]：撤销本次批量替换。
#[test]
fn alt_u_rolls_the_whole_batch_back() {
    let f = setup();
    let before: Vec<String> = f.v1.iter().chain(&f.v2).map(|c| f.body(*c)).collect();

    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap();
    app.drain_batch_for_test().unwrap();

    app.press_for_test(KeyCode::Char('u'), ALT).unwrap();

    let after: Vec<String> = f.v1.iter().chain(&f.v2).map(|c| f.body(*c)).collect();
    assert_eq!(after, before, "撤销后每一章都该一字不差地回到操作前");
}

/// 确认框默认停在「取消」：Enter 不该把整本书改了。
#[test]
fn confirm_defaults_to_cancel_so_enter_does_nothing() {
    let f = setup();
    let before = f.body(f.v2[0]);

    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap(); // 默认在「取消」上
    app.drain_batch_for_test().unwrap();

    assert_eq!(f.body(f.v2[0]), before, "默认落在取消上，Enter 不该动稿子");
}

/// Esc 取消确认框，同样什么都不该发生。
#[test]
fn esc_cancels_confirm() {
    let f = setup();
    let before = f.body(f.v2[0]);

    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    app.drain_batch_for_test().unwrap();

    assert_eq!(f.body(f.v2[0]), before);
}

/// 当前章范围不弹确认框——每次替换一句话都要按 y，用户会疯。
/// 走的是老路径（改内存缓冲），存盘后才落到磁盘。
#[test]
fn chapter_scope_skips_confirm() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Chapter);

    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    assert!(!app.confirm_open_for_test(), "当前章范围不该弹确认框");

    // 老路径改的是内存缓冲，存盘才落磁盘。先 Esc 关掉查找面板，
    // 否则 Ctrl+S 会被面板吃掉，存不下去。
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    app.press_for_test(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(f.body(f.v1[0]).contains('霜'), "当前章该被改掉");
    assert!(f.body(f.v2[0]).contains('雪'), "别的章不该被动到");
}

/// §6.6 [MUST]：执行前每章各打一条快照。撤销要靠它，F8 也要看得到。
#[test]
fn every_touched_chapter_gets_a_snapshot() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap();
    app.drain_batch_for_test().unwrap();

    for ch in f.v1.iter().chain(&f.v2) {
        let snaps = app.snapshot_texts_for_test(*ch);
        assert!(
            snaps.iter().any(|t| t.contains('雪')),
            "第 {ch} 章没留下操作前的快照，撤销就无从谈起"
        );
    }
}

/// 把 app 在给定尺寸下画出来，返回是否含替换字符（切碎的信号）。
fn draw_ok(app: &mut App, w: u16, h: u16) -> bool {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    let buf = term.backend().buffer().clone();
    // 拼出整屏文本，出现 U+FFFD 说明 CJK 宽度算错、字被切了。
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    !text.contains('\u{fffd}')
}

/// §10：确认框在 §7.2 的各档宽度（含 60 列下限）下都不撕屏、不 panic。
#[test]
fn confirm_dialog_renders_across_widths() {
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let f = setup();
        let mut app = f.app();
        open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
        app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
        assert!(app.confirm_open_for_test(), "确认框该开着");
        assert!(draw_ok(&mut app, w, h), "确认框在 {w}x{h} 下把字切碎了");
    }
}

/// 极端尺寸：拖窗口会瞬间经过这些值，确认框不得 panic。
#[test]
fn confirm_dialog_survives_degenerate_sizes() {
    for (w, h) in [(1, 1), (2, 2), (60, 3), (200, 1), (10, 40)] {
        let f = setup();
        let mut app = f.app();
        open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
        app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
        let _ = draw_ok(&mut app, w, h); // 不 panic 即通过
    }
}

/// 批量进度界面同样要能画出来（作业跑起来的那一帧）。
#[test]
fn batch_progress_renders() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "雪", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap();
    // 此刻作业已建好、还没 drain——正是进度界面在场的时候。
    for (w, h) in [(60, 20), (200, 50)] {
        assert!(draw_ok(&mut app, w, h), "进度界面在 {w}x{h} 下把字切碎了");
    }
}

/// 空查找串不该开一个改遍全书的作业。
#[test]
fn empty_query_does_not_start_a_job() {
    let f = setup();
    let mut app = f.app();
    open_replace(&mut app, "", "霜", mj_tui::batch::Scope::Book);
    app.press_for_test(KeyCode::Char('a'), ALT).unwrap();

    assert!(
        app.toast_for_test().is_some_and(|t| t.contains("查找")),
        "该提示先输入查找内容，实际：{:?}",
        app.toast_for_test()
    );
    assert!(f.body(f.v2[0]).contains('雪'), "什么都不该改");
}
