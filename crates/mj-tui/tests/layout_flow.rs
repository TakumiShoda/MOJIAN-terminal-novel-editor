//! 外观预设真正作用于排版，以及专注模式。见 doc.md §6.10、§7.2、§7.3。
//!
//! 这些值此前只在设置页里只读展示、并不影响绘制——那等于没做。
//! 这里从**渲染出来的屏幕**验证它们确实改变了版面。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::ChapterId;
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
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
    Fixture { dir, ch }
}

impl Fixture {
    fn app_with(&self, tweak: impl FnOnce(&mut Config)) -> App {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        let mut config = Config::default();
        tweak(&mut config);
        let store = Store::new(ws, config.clone());
        let mut app = App::new(store, config).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }
}

/// 逐行读回屏幕（按显示宽度跳过全角字占位格）。
fn rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|fr| app.render_for_test(fr)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let s = buf[(x, y)].symbol();
            line.push_str(s);
            x += (UnicodeWidthStr::width(s) as u16).max(1);
        }
        out.push(line);
    }
    out
}

/// 找到正文首行在屏幕上的左起列（用于判断留白/居中是否生效）。
fn text_indent(app: &mut App, w: u16, h: u16, needle: &str) -> Option<usize> {
    for line in rows(app, w, h) {
        if let Some(pos) = line.find(needle) {
            return Some(pos);
        }
    }
    None
}

const BODY: &str = "　　雪落了一夜。他推开门，风裹着雪灌进来，冷得刺骨。\n";

/// 栏宽收窄后正文应当**居中**，左边出现明显留白。
#[test]
fn column_width_narrows_and_centers_text() {
    let f = setup(BODY);
    // 撑满：正文靠左（只隔边框与留白）。
    let mut wide = f.app_with(|c| {
        c.appearance.column_width = 0;
        c.appearance.margin = 0;
    });
    let wide_indent = text_indent(&mut wide, 120, 20, "雪落了一夜").unwrap();

    // 收到 20 全角字（=40 列）：在 120 列的屏幕上应被推到中间。
    let mut narrow = f.app_with(|c| {
        c.appearance.column_width = 20;
        c.appearance.margin = 0;
    });
    let narrow_indent = text_indent(&mut narrow, 120, 20, "雪落了一夜").unwrap();

    assert!(
        narrow_indent > wide_indent + 10,
        "收窄后正文应居中：撑满 {wide_indent} 列 vs 收窄 {narrow_indent} 列"
    );
}

/// 左右留白生效。
#[test]
fn margin_pushes_text_inward() {
    let f = setup(BODY);
    let mut none = f.app_with(|c| {
        c.appearance.column_width = 0;
        c.appearance.margin = 0;
    });
    let a = text_indent(&mut none, 100, 20, "雪落了一夜").unwrap();

    let mut wide = f.app_with(|c| {
        c.appearance.column_width = 0;
        c.appearance.margin = 8;
    });
    let b = text_indent(&mut wide, 100, 20, "雪落了一夜").unwrap();
    assert!(b > a, "留白应把正文往里推：{a} → {b}");
}

/// 行号显示。
#[test]
fn line_numbers_appear_when_enabled() {
    let f = setup("第一段。\n第二段。\n第三段。\n");
    let mut off = f.app_with(|c| c.appearance.line_number = false);
    let off_rows = rows(&mut off, 100, 20).join("\n");

    let mut on = f.app_with(|c| c.appearance.line_number = true);
    let on_rows = rows(&mut on, 100, 20).join("\n");

    assert_ne!(off_rows, on_rows, "开行号后画面应当不同");
    // 行号是从 1 数起的段落号。
    let has_numbered = rows(&mut on, 100, 20)
        .iter()
        .any(|l| l.contains("1 ") && l.contains("第一段"));
    assert!(has_numbered, "首段该带行号 1：\n{on_rows}");
}

/// 段间距撑出空行：同样的三段，开了段间距之后占的行数更多。
#[test]
fn paragraph_spacing_adds_blank_rows() {
    let f = setup("第一段。\n第二段。\n第三段。\n");
    let count_nonempty = |app: &mut App| -> usize {
        rows(app, 100, 20)
            .iter()
            .filter(|l| l.contains("第") && l.contains("段"))
            .count()
    };

    let mut tight = f.app_with(|c| c.appearance.paragraph_spacing = 0);
    let mut loose = f.app_with(|c| c.appearance.paragraph_spacing = 1);
    assert_eq!(count_nonempty(&mut tight), 3, "三段都该在");
    assert_eq!(count_nonempty(&mut loose), 3, "加空行不该吞掉段落");

    // 第一段与第二段之间应多出一行空白。
    let find_row = |app: &mut App, s: &str| -> usize {
        rows(app, 100, 20)
            .iter()
            .position(|l| l.contains(s))
            .unwrap()
    };
    let gap_tight = find_row(&mut tight, "第二段") - find_row(&mut tight, "第一段");
    let gap_loose = find_row(&mut loose, "第二段") - find_row(&mut loose, "第一段");
    assert_eq!(gap_tight, 1);
    assert_eq!(gap_loose, 2, "段间距 1 应让两段之间隔开一行");
}

/// 专注模式：收起目录树，正文按 focus_column_width 收窄。
#[test]
fn focus_mode_hides_tree_and_narrows() {
    let f = setup(BODY);
    let mut app = f.app_with(|c| {
        c.appearance.column_width = 0;
        c.editor.focus_column_width = 20;
    });
    assert!(app.show_tree_for_test(), "默认显示目录树");
    let before = text_indent(&mut app, 120, 20, "雪落了一夜").unwrap();

    app.press_for_test(KeyCode::F(11), NONE).unwrap();
    assert!(!app.show_tree_for_test(), "专注模式应收起目录树");
    let after = text_indent(&mut app, 120, 20, "雪落了一夜").unwrap();
    assert!(
        after > before,
        "专注模式下正文应收窄居中：{before} → {after}"
    );

    // 再按一次退出。
    app.press_for_test(KeyCode::F(11), NONE).unwrap();
    assert!(app.show_tree_for_test(), "再按 F11 应退出专注模式");
}

/// 命令面板里能找到专注模式（§7.3：所有功能都要能从这里触达）。
#[test]
fn focus_mode_reachable_from_palette() {
    let f = setup(BODY);
    let mut app = f.app_with(|_| {});
    app.press_for_test(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();
    for c in "专注".chars() {
        app.press_for_test(KeyCode::Char(c), NONE).unwrap();
    }
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert!(!app.show_tree_for_test(), "命令面板里也该能进专注模式");
}

/// §7.2 [MUST]：窄至 60 列不崩——留白与栏宽的减法不得翻负。
#[test]
fn survives_narrow_terminals() {
    let f = setup(BODY);
    for (w, h) in [(60, 10), (40, 8), (20, 6)] {
        let mut app = f.app_with(|c| {
            c.appearance.margin = 8;
            c.appearance.column_width = 40;
            c.appearance.line_number = true;
            c.appearance.paragraph_spacing = 1;
        });
        let text = rows(&mut app, w, h).join("\n");
        assert!(!text.contains('\u{fffd}'), "{w}x{h} 撕屏了");
    }
}
