//! 折行的属性测试。见 doc.md §10、§6.3。
//!
//! 折行是编辑器里最容易在边角出错的算法：禁则、宽度、断点回退三者互相牵制。
//! 手写用例只能覆盖想得到的组合——「丢字」「死循环」「切出乱码」都藏在想不到的地方。

use proptest::prelude::*;

use mj_text::width::{display_width, forbidden_at_line_end, forbidden_at_line_start, wrap_line};

/// 生成含中文、标点、英文、空格、emoji 的随机文本。
fn cjk_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just("，".to_string()),
            Just("「".to_string()),
            Just("」".to_string()),
            Just("（".to_string()),
            Just("）".to_string()),
            Just("a".to_string()),
            Just(" ".to_string()),
            Just("　".to_string()), // 全角空格
            Just("👍".to_string()),
            Just("e\u{301}".to_string()), // 组合字符
        ],
        0..30,
    )
    .prop_map(|v| v.concat())
}

proptest! {
    /// 折行不得丢字、不得重复：拼回去必须逐字节等于原文。
    /// 这是最重要的一条——丢字就是丢稿。
    #[test]
    fn wrap_never_loses_text(text in cjk_text(), width in 1usize..30) {
        let lines = wrap_line(&text, width);
        let joined: String = lines.iter().map(|r| mj_text::width::slice(&text, r.clone()).unwrap_or_default()).collect();
        prop_assert_eq!(joined, text);
    }

    /// 所有区间必须首尾相接、落在字符边界、覆盖全文。
    #[test]
    fn wrap_ranges_are_well_formed(text in cjk_text(), width in 1usize..30) {
        let lines = wrap_line(&text, width);
        let mut expect = 0usize;
        for r in &lines {
            prop_assert_eq!(r.start, expect);
            prop_assert!(r.end >= r.start);
            prop_assert!(text.is_char_boundary(r.start));
            prop_assert!(text.is_char_boundary(r.end));
            expect = r.end;
        }
        prop_assert_eq!(expect, text.len());
    }

    /// 禁则：行首不得是收尾标点。
    ///
    /// 例外：整行只有这一个字符时无处可推（宽度过窄），此时允许。
    #[test]
    fn kinsoku_line_start_holds(text in cjk_text(), width in 2usize..30) {
        let lines = wrap_line(&text, width);
        for (i, r) in lines.iter().enumerate() {
            if i == 0 { continue; } // 首行的行首就是段首，不受禁则约束
            let line = mj_text::width::slice(&text, r.clone()).unwrap_or_default();
            let Some(c) = line.chars().next() else { continue };
            if forbidden_at_line_start(c) {
                // 允许的例外：该行仅此一字，或宽度容不下更多。
                prop_assert!(
                    line.chars().count() == 1 || display_width(line) <= 2,
                    "行首禁则被破坏: 行={:?} 全部={:?}", line, lines
                );
            }
        }
    }

    /// 禁则：行尾不得是起始标点（末行除外——它后面没有内容了）。
    ///
    /// 但禁则**存在无解的情况**：整行全是起始标点（`（「「`）时，
    /// 折在任何位置行尾都违规，只能放弃。故这里断言的是
    /// 「该行存在非起始标点时，行尾必定不是起始标点」。
    #[test]
    fn kinsoku_line_end_holds_when_satisfiable(text in cjk_text(), width in 2usize..30) {
        let lines = wrap_line(&text, width);
        let n = lines.len();
        for (i, r) in lines.iter().enumerate() {
            if i + 1 == n { continue; } // 末行行尾不构成禁则问题
            let line = mj_text::width::slice(&text, r.clone()).unwrap_or_default();
            let Some(c) = line.chars().next_back() else { continue };
            if forbidden_at_line_end(c) {
                // 无解的判据：整行都是起始标点，退无可退。
                let all_forbidden = line.chars().all(forbidden_at_line_end);
                prop_assert!(
                    all_forbidden,
                    "存在可行解却违反行尾禁则: 行={:?} 全部={:?}", line, lines
                );
            }
        }
    }

    /// 不得死循环、不得产出空行海：行数应受文本长度约束。
    #[test]
    fn wrap_terminates_with_sane_line_count(text in cjk_text(), width in 1usize..30) {
        let lines = wrap_line(&text, width);
        // 每行至少含一个 grapheme（除非原文为空），故行数不超过 grapheme 数。
        let gcount = mj_text::width::grapheme_count(&text).max(1);
        prop_assert!(lines.len() <= gcount, "行数 {} 超过 grapheme 数 {}", lines.len(), gcount);
    }
}
