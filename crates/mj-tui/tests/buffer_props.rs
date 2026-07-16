//! 编辑缓冲的属性测试。见 doc.md §10、§0 禁令 5。
//!
//! 光标的不变量必须对**任意**文本成立，不只是我想得到的那些。
//! 手写用例已经漏过一次：8 字节窗口对 ZWJ emoji 家族（18 字节）不够，
//! 直接 panic。这类边界只有随机生成才碰得到。

use proptest::prelude::*;

use mj_tui::editor::Buffer;

/// 生成含中文、组合字符、emoji、换行的随机文本。
fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just("a".to_string()),
            Just(" ".to_string()),
            Just("\n".to_string()),
            Just("　".to_string()),
            Just("👍".to_string()),
            Just("👨‍👩‍👧".to_string()),       // ZWJ 家族，18 字节
            Just("e\u{301}".to_string()), // 组合字符
            Just("🇯🇵".to_string()),       // 旗帜（区域指示符对）
        ],
        0..25,
    )
    .prop_map(|v| v.concat())
}

/// 随机的光标操作序列。
#[derive(Debug, Clone)]
enum Op {
    Left,
    Right,
    Home,
    End,
    MoveTo(usize),
    Insert(String),
    DeleteBack,
    DeleteFwd,
    Undo,
    Redo,
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    proptest::collection::vec(
        prop_oneof![
            Just(Op::Left),
            Just(Op::Right),
            Just(Op::Home),
            Just(Op::End),
            (0usize..40).prop_map(Op::MoveTo),
            prop_oneof![
                Just("雪".to_string()),
                Just("👨‍👩‍👧".to_string()),
                Just("\n".to_string())
            ]
            .prop_map(Op::Insert),
            Just(Op::DeleteBack),
            Just(Op::DeleteFwd),
            Just(Op::Undo),
            Just(Op::Redo),
        ],
        0..30,
    )
}

fn run(b: &mut Buffer, ops: &[Op]) {
    for op in ops {
        match op {
            Op::Left => b.move_left(),
            Op::Right => b.move_right(),
            Op::Home => b.move_home(),
            Op::End => b.move_end(),
            Op::MoveTo(n) => b.move_to(*n),
            Op::Insert(s) => b.insert(s),
            Op::DeleteBack => b.delete_backward(),
            Op::DeleteFwd => b.delete_forward(),
            Op::Undo => {
                b.undo();
            }
            Op::Redo => {
                b.redo();
            }
        }
    }
}

proptest! {
    /// 任意操作序列都不得 panic，且光标始终落在合法位置。
    /// 这条是整个编辑器的地基：光标停在半个字符上，后续所有 rope 操作都会炸。
    #[test]
    fn cursor_stays_valid_under_any_ops(t in text(), o in ops()) {
        let mut b = Buffer::new(&t, 500);
        run(&mut b, &o);
        prop_assert!(b.cursor() <= b.len_bytes(), "光标越界");
        // 能取到字符串即证明所有内部偏移合法（否则 ropey 会 panic）。
        let _ = b.contents();
    }

    /// 文本内容始终是合法 UTF-8 且不含孤立的半个字符。
    #[test]
    fn text_stays_valid_utf8(t in text(), o in ops()) {
        let mut b = Buffer::new(&t, 500);
        run(&mut b, &o);
        let s = b.contents();
        // String 本身保证 UTF-8；再验证 grapheme 切分不产生空片段。
        prop_assert!(mj_text::width::grapheme_offsets(&s).all(|(_, g)| !g.is_empty()));
    }

    /// 撤销到底必定回到初始文本——不多不少。
    /// 这是「改坏了能退回去」的最低保证。
    #[test]
    fn undo_all_restores_original(t in text(), o in ops()) {
        let mut b = Buffer::new(&t, 500);
        // 只做编辑，不做 undo/redo，否则「撤销到底」的语义不成立。
        let edits: Vec<Op> = o.into_iter()
            .filter(|op| !matches!(op, Op::Undo | Op::Redo))
            .collect();
        run(&mut b, &edits);
        while b.undo() {}
        prop_assert_eq!(b.contents(), t);
    }

    /// undo 之后 redo，必定回到 undo 之前的状态。
    #[test]
    fn redo_after_undo_is_identity(t in text(), o in ops()) {
        let mut b = Buffer::new(&t, 500);
        let edits: Vec<Op> = o.into_iter()
            .filter(|op| !matches!(op, Op::Undo | Op::Redo))
            .collect();
        run(&mut b, &edits);
        let after_edits = b.contents();

        while b.undo() {}
        while b.redo() {}
        prop_assert_eq!(b.contents(), after_edits);
    }

    /// 光标永远落在 grapheme 边界：从该位置切分文本不得切碎任何 cluster。
    #[test]
    fn cursor_is_always_on_grapheme_boundary(t in text(), o in ops()) {
        let mut b = Buffer::new(&t, 500);
        run(&mut b, &o);

        let s = b.contents();
        let cursor = b.cursor();
        // 光标位置必须出现在 grapheme 起始偏移集合里（或正好是文末）。
        let is_boundary = cursor == s.len()
            || mj_text::width::grapheme_offsets(&s).any(|(off, _)| off == cursor);
        prop_assert!(is_boundary, "光标 {} 不在 grapheme 边界，文本={:?}", cursor, s);
    }
}
