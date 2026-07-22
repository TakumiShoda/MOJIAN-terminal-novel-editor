//! 挪动章的端到端流程：树上 `Alt+↑/↓`。见 doc.md §6.2 [MUST]（上下移动、跨卷移动）。
//!
//! 走真实按键，断言磁盘上的章序。同两个键既卷内重排、又跨卷边界。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const ALT: KeyModifiers = KeyModifiers::ALT;

struct Fx {
    dir: tempfile::TempDir,
    book: BookId,
    a: ChapterId,
    b: ChapterId,
    c: ChapterId,
}

/// 卷一 [A, B]，卷二 [C]。
fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("书", "作者").unwrap();
    let v1 = store.create_volume(book.id, "卷一", None).unwrap();
    let v2 = store.create_volume(book.id, "卷二", Some(v1)).unwrap();
    let a = store.create_chapter(book.id, v1, "A", None).unwrap();
    let b = store.create_chapter(book.id, v1, "B", Some(a)).unwrap();
    let c = store.create_chapter(book.id, v2, "C", None).unwrap();
    Fx {
        dir,
        book: book.id,
        a,
        b,
        c,
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
    /// 卷内章序（id → 名）：`[("卷一", vec!["A","B"]), ...]`。
    fn layout(&self) -> Vec<(String, Vec<String>)> {
        self.store()
            .load_book(self.book)
            .unwrap()
            .volumes
            .iter()
            .map(|v| {
                (
                    v.title.clone(),
                    v.chapters.iter().map(|c| c.title.clone()).collect(),
                )
            })
            .collect()
    }
    /// 把光标移到某一章（从树顶往下数）。开书默认选中并打开首章。
    fn select(&self, app: &mut App, ch: ChapterId) {
        // 简单起见：靠 focus + 按键定位太脆，直接用测试钩子选。
        app.select_chapter_for_test(ch);
    }
}

fn alt(app: &mut App, up: bool) {
    let code = if up { KeyCode::Up } else { KeyCode::Down };
    app.press_for_test(code, ALT).unwrap();
}

/// 卷内下移：A 往下一位，卷一变 [B, A]。
#[test]
fn move_down_within_volume() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.a);
    alt(&mut app, false);
    assert_eq!(f.layout()[0].1, vec!["B", "A"], "A 应下移到 B 之后");
}

/// 卷内上移：B 往上一位，卷一变 [B, A]。
#[test]
fn move_up_within_volume() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.b);
    alt(&mut app, true);
    assert_eq!(f.layout()[0].1, vec!["B", "A"], "B 应上移到 A 之前");
}

/// 跨卷下移：卷尾的 B 再往下，落到卷二的开头。
#[test]
fn move_down_across_volume_boundary() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.b); // 卷一末章
    alt(&mut app, false);
    let l = f.layout();
    assert_eq!(l[0].1, vec!["A"], "卷一只剩 A");
    assert_eq!(l[1].1, vec!["B", "C"], "B 落到卷二开头");
}

/// 跨卷上移：卷首的 C 再往上，落到卷一的末尾。
#[test]
fn move_up_across_volume_boundary() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.c); // 卷二首章（也是唯一）
    alt(&mut app, true);
    let l = f.layout();
    assert_eq!(l[0].1, vec!["A", "B", "C"], "C 落到卷一末尾");
    assert!(l[1].1.is_empty(), "卷二空了");
}

/// 全书第一章再上移：不动、不崩。
#[test]
fn move_up_at_the_very_top_is_a_no_op() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.a);
    alt(&mut app, true);
    assert_eq!(f.layout()[0].1, vec!["A", "B"], "已在最前，不该变");
}

/// 全书最后一章再下移：不动、不崩。
#[test]
fn move_down_at_the_very_bottom_is_a_no_op() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.c);
    alt(&mut app, false);
    let l = f.layout();
    assert_eq!(l[0].1, vec!["A", "B"]);
    assert_eq!(l[1].1, vec!["C"], "已在最后，不该变");
}

/// 连按 Alt+↓ 一路把 A 从卷一挪到卷二末尾——光标跟着走，动作可连贯。
#[test]
fn repeated_nudges_walk_a_chapter_through_the_book() {
    let f = setup();
    let mut app = f.app();
    f.select(&mut app, f.a);
    alt(&mut app, false); // [B,A] | [C]
    alt(&mut app, false); // [B] | [A,C]
    alt(&mut app, false); // [B] | [C,A]
    let l = f.layout();
    assert_eq!(l[0].1, vec!["B"]);
    assert_eq!(l[1].1, vec!["C", "A"], "A 应一路走到卷二末尾：{l:?}");
}
