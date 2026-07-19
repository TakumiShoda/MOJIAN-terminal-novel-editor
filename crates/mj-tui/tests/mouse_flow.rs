//! 鼠标支持的端到端流程。见 doc.md §13。
//!
//! 每个用例都**先渲染一帧再送事件**——命中区域是渲染时记下的，真实主循环
//! 也是这个次序（先画，用户看着画面点，事件才来）。跳过渲染直接送事件，
//! 测的就不是用户会遇到的那条路。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{MouseButton, MouseEventKind};

const W: u16 = 100;
const H: u16 = 30;

struct Fixture {
    dir: tempfile::TempDir,
    #[allow(dead_code)]
    book: BookId,
    ch1: ChapterId,
    ch2: ChapterId,
}

fn setup() -> Fixture {
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
    // 行要够长：短行的话往右点会全部落到行尾，那些用例就成了摆设。
    let body: String = (0..80)
        .map(|i| format!("　　第{i}行：{}\n", "风雪扑面而来他一路奔到城门".repeat(3)))
        .collect();
    store
        .save_body(book.id, &mj_core::model::ChapterBody::new(ch1, &body))
        .unwrap();
    store
        .save_body(
            book.id,
            &mj_core::model::ChapterBody::new(ch2, "　　第二章的正文。\n"),
        )
        .unwrap();
    Fixture {
        dir,
        book: book.id,
        ch1,
        ch2,
    }
}

impl Fixture {
    fn app(&self) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        let mut app = App::new(Store::new(ws, Config::default()), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch1).unwrap();
        app
    }
}

/// 画一帧，让命中区域就位。
fn draw(app: &mut App) {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
}

fn wheel(app: &mut App, up: bool, col: u16, row: u16) {
    let kind = if up {
        MouseEventKind::ScrollUp
    } else {
        MouseEventKind::ScrollDown
    };
    app.mouse_for_test(kind, col, row).unwrap();
}

fn click(app: &mut App, col: u16, row: u16) {
    app.mouse_for_test(MouseEventKind::Down(MouseButton::Left), col, row)
        .unwrap();
}

/// §13：鼠标是可选的，默认不开——开了终端自己的拖选复制就没了。
#[test]
fn mouse_is_off_by_default() {
    assert!(!Config::default().input.mouse);
    let f = setup();
    assert!(!f.app().mouse_enabled(), "默认配置下不该捕获鼠标");
}

/// 配置开了才捕获。
#[test]
fn config_turns_mouse_on() {
    let f = setup();
    let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
    std::fs::write(ws.config_file(), "[input]\nmouse = true\n").unwrap();
    let config = Config::load(&ws.config_file()).unwrap();
    assert!(config.input.mouse);
    let app = App::new(Store::new(ws, config.clone()), config).unwrap();
    assert!(app.mouse_enabled());
}

/// 正文滚轮滚的是**视口**，不动光标——滚两眼别处不该把光标带走。
#[test]
fn wheel_over_editor_scrolls_view_without_moving_cursor() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);
    let (cursor0, top0) = app.editor_pos_for_test().unwrap();
    assert_eq!(top0, 0);

    // 正文在右半边（左边是目录树）。
    wheel(&mut app, false, 70, 10);
    let (cursor1, top1) = app.editor_pos_for_test().unwrap();
    assert!(top1 > top0, "该往下滚：{top0} → {top1}");
    assert_eq!(cursor1, cursor0, "滚轮不该动光标");

    wheel(&mut app, true, 70, 10);
    let (_, top2) = app.editor_pos_for_test().unwrap();
    assert!(top2 < top1, "该滚回来：{top1} → {top2}");
}

/// 滚到顶了再往上滚：停住，不崩、不越界。
#[test]
fn wheel_at_the_top_is_a_no_op() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);
    for _ in 0..20 {
        wheel(&mut app, true, 70, 10);
    }
    assert_eq!(app.editor_pos_for_test().unwrap().1, 0);
}

/// 点目录树里的一行 = 把选中挪过去再按 Enter：是章就打开。
#[test]
fn click_on_tree_opens_that_chapter() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);
    assert_eq!(app.current_chapter_for_test(), Some(f.ch1));

    // 树的内容从 y+1 起（上边框占一行）：第 0 行是卷，第 1、2 行是两章。
    // 点第二章那一行。
    click(&mut app, 5, 3);
    assert_eq!(
        app.current_chapter_for_test(),
        Some(f.ch2),
        "点哪一章就该开哪一章"
    );
}

/// 点在树的空白处：什么都不做，尤其不能崩。
#[test]
fn click_on_empty_tree_area_does_nothing() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);
    let before = app.current_chapter_for_test();
    click(&mut app, 5, H - 3); // 远在最后一章之下
    assert_eq!(app.current_chapter_for_test(), before);
}

/// 点正文把光标放过去，且落点必须是**字素簇边界**。
///
/// 这是最容易写错的一处：正文全是汉字，一个汉字占两列却是三个字节，
/// 拿列号当字节数用，光标就会插进汉字中间（§0：光标按字素簇动）。
#[test]
fn click_in_body_lands_on_a_grapheme_boundary() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);

    // 第 2/5/8 行是折行后的续行，正文实际铺在 x≈32..96，点在这里才落到字上。
    let text = app.buffer_text_for_test().unwrap();
    for col in [40u16, 51, 62, 73, 84] {
        for row in [2u16, 5, 8] {
            click(&mut app, col, row);
            let (cursor, _) = app.editor_pos_for_test().unwrap();
            assert!(
                text.is_char_boundary(cursor),
                "({col},{row}) 落在了字符中间：byte {cursor}"
            );
        }
    }
}

/// 往右点得越远，光标越靠后——列号确实被当成了显示宽度而不是字节数。
#[test]
fn clicking_further_right_moves_the_cursor_further() {
    let f = setup();
    let mut app = f.app();
    draw(&mut app);

    // 同一行（折行后的续行，全是汉字）上左右各点一下，相隔 30 列。
    click(&mut app, 40, 5);
    let (near, _) = app.editor_pos_for_test().unwrap();
    click(&mut app, 70, 5);
    let (far, _) = app.editor_pos_for_test().unwrap();
    assert!(far > near, "点得靠右，光标该更靠后：{near} vs {far}");

    // 一个汉字占 2 列、3 字节：隔 30 列就是 15 个字 = 45 字节。
    // 要是把列号直接当字节数用，这里只会差 30——所以这条断言正是那个错误的照妖镜。
    assert_eq!(
        far - near,
        45,
        "隔 30 列该走 15 个汉字 45 字节；差 {} 说明列宽算错了",
        far - near
    );
}

/// 滚轮滚到浮层上：走键盘那条路，等于按上下键。
#[test]
fn wheel_scrolls_the_top_modal() {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    let f = setup();
    let mut app = f.app();
    // F1 帮助页，它是能上下滚的长列表。
    app.press_for_test(KeyCode::F(1), KeyModifiers::NONE)
        .unwrap();
    draw(&mut app);
    let before = screen(&mut app);
    wheel(&mut app, false, 50, 10);
    assert_ne!(screen(&mut app), before, "滚轮该让帮助页动起来");
}

fn screen(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect()
}
