//! CJK 宽度 / grapheme 工具。见 doc.md §2.3、§6.3。
//!
//! 三条铁律：
//! - 光标一律按 grapheme cluster 移动，不按字节、不按 char（§0 禁令 5）；
//! - 布局一律按显示宽度算，CJK = 2 列；
//! - 折行遵守中文禁则：行首不得是收尾标点，行尾不得是起始标点。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 行首禁则字符：不得出现在折行后的行首。
///
/// 即「收尾类」标点——句号、逗号、右括号、右引号等。它们若被推到下一行行首，
/// 排版上是明显的错误。
const NO_LINE_START: &[char] = &[
    '。', '，', '、', '；', '：', '？', '！', '）', '》', '」', '』', '】', '〉', '·', '…', '—',
    '～', '‥', '﹏', '.', ',', ';', ':', '?', '!', ')', ']', '}', '"', '\'',
];

/// 行尾禁则字符：不得出现在折行前的行尾。
///
/// 即「起始类」标点——左括号、左引号等。它们若孤零零留在行尾，下一行才是被引的内容，
/// 读起来是断的。
const NO_LINE_END: &[char] = &['（', '《', '「', '『', '【', '〈', '(', '[', '{'];

/// 该字符是否不能出现在行首。
pub fn forbidden_at_line_start(c: char) -> bool {
    NO_LINE_START.contains(&c)
}

/// 该字符是否不能出现在行尾。
pub fn forbidden_at_line_end(c: char) -> bool {
    NO_LINE_END.contains(&c)
}

/// 字符串的显示宽度（CJK 计 2）。
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 按 grapheme 切分。光标移动、删除都必须走它（§0 禁令 5）。
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    s.graphemes(true)
}

/// grapheme 个数。
pub fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}

/// 把段落按显示宽度折成若干行，遵守中文禁则（§6.3）。
///
/// `width` 为可用列数。返回每行的**字节区间**，便于映射回原文（光标定位需要）。
///
/// 折行点优先级：
/// 1. 中文标点之后 / 空格处（自然断点）；
/// 2. 无自然断点时按宽度硬折；
/// 3. 两者都要让位于禁则——宁可让某行短一点。
///
/// **禁则不是绝对的**：当一行从头到尾都是起始标点（如 `（「「`），
/// 无论折在哪里行尾都是禁则字符，此时只能放弃——真实排版软件也如此。
/// 保证的是「有可行解时必定满足禁则」，而非「任何输入都满足」。
pub fn wrap_line(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    if width == 0 || text.is_empty() {
        // 宽度为 0（栏宽未知/极窄）时不折行，交由调用方裁剪——
        // 折成无数空行只会更糟。
        //
        // clippy 的 single_range_in_vec_init 提醒「单元素 Range 的 Vec 多半是笔误」，
        // 但此处返回值的类型就是「行的区间列表」，一行也是合法结果。
        #[allow(clippy::single_range_in_vec_init)]
        return vec![0..text.len()];
    }

    let mut lines = Vec::new();
    let mut line_start = 0usize;

    while line_start < text.len() {
        let cut = find_cut(text, line_start, width);
        debug_assert!(cut > line_start, "折行未推进，将死循环");
        debug_assert!(text.is_char_boundary(cut), "折点不在字符边界");
        lines.push(line_start..cut);
        line_start = cut;
    }

    if lines.is_empty() {
        lines.push(0..text.len());
    }
    lines
}

/// 求从 `start` 开始的这一行应在哪里断开。返回值必定 > start 且落在 grapheme 边界。
///
/// 决策顺序：
/// 1. 按宽度找到「放不下的第一个 grapheme」，得到硬折点；
/// 2. 有自然断点（空格/标点之后）就回退到那里；
/// 3. 应用禁则调整；
/// 4. 保证推进（至少一个 grapheme）。
fn find_cut(text: &str, start: usize, width: usize) -> usize {
    let rest = match text.get(start..) {
        Some(r) => r,
        None => return text.len(),
    };

    let mut line_width = 0usize;
    let mut hard_cut = None; // 放不下的第一个 grapheme 的绝对偏移
    let mut last_break = None; // 最近的自然断点（绝对偏移）

    for (rel, g) in rest.grapheme_indices(true) {
        let abs = start + rel;
        let gw = display_width(g);
        let c = g.chars().next().unwrap_or(' ');

        if line_width + gw > width {
            // 行尾空白放不下时不折行：空格在行尾不可见，
            // 为它把「hello world」拆开得不偿失。
            if c.is_whitespace() {
                last_break = Some(abs + g.len());
                continue;
            }
            hard_cut = Some(abs);
            break;
        }

        line_width += gw;
        if is_break_opportunity(c) && !forbidden_at_line_end(c) {
            last_break = Some(abs + g.len());
        }
    }

    // 整行都放得下。
    let Some(hard) = hard_cut else {
        return text.len();
    };

    let mut cut = last_break.unwrap_or(hard);

    // 禁则一：下一行行首不得是收尾标点 —— 把它们吞进本行（悬挂）。
    // 判据看**折点处**的字符，而非「放不下的那个」：回退到自然断点后，
    // 落在行首的是另一个字符（proptest 以 "雪 。雪" 宽 4 抓到过）。
    cut = hang_closing_punct(text, cut);

    // 禁则二：本行行尾不得是起始标点 —— 提前折行把它推到下一行。
    // 需循环回退：`「「（` 退一格后行尾仍违规（proptest 以 "雪「「（「" 抓到过）。
    cut = retreat_from_opening_punct(text, start, cut);

    // 保证推进：禁则调整后若回到起点，说明本行无可行解
    // （如整行都是起始标点），此时接受违规，硬折一个 grapheme。
    if cut <= start {
        return next_grapheme_boundary(text, start);
    }
    cut.min(text.len())
}

/// 把折点处的收尾标点吞进本行，直到行首不再违规或到达文末。
fn hang_closing_punct(text: &str, mut cut: usize) -> usize {
    while cut < text.len() {
        let Some(g) = text.get(cut..).and_then(|s| s.graphemes(true).next()) else {
            break;
        };
        let Some(c) = g.chars().next() else { break };
        if !forbidden_at_line_start(c) {
            break;
        }
        cut += g.len();
    }
    cut
}

/// 从行尾的起始标点处回退，直到行尾不再违规或退到行首。
fn retreat_from_opening_punct(text: &str, start: usize, mut cut: usize) -> usize {
    while cut > start {
        let Some(prev) = last_grapheme_before(text, cut) else {
            break;
        };
        let Some(c) = prev.chars().next() else { break };
        if !forbidden_at_line_end(c) {
            break;
        }
        cut -= prev.len();
    }
    cut
}

/// 该字符处是否是自然断点（其后可折行）。
fn is_break_opportunity(c: char) -> bool {
    c.is_whitespace() || forbidden_at_line_start(c)
}

/// 取 `offset` 之前的最后一个 grapheme。
fn last_grapheme_before(text: &str, offset: usize) -> Option<&str> {
    text.get(..offset)?.graphemes(true).next_back()
}

/// 光标向前移动一个 grapheme，返回新的字节偏移。
pub fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    text.get(offset..)
        .and_then(|rest| rest.graphemes(true).next())
        .map(|g| offset + g.len())
        .unwrap_or(offset)
}

/// 光标向后移动一个 grapheme，返回新的字节偏移。
pub fn prev_grapheme_boundary(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .and_then(|head| head.graphemes(true).next_back())
        .map(|g| offset - g.len())
        .unwrap_or(offset)
}

/// 按 `wrap_line` 返回的区间取出该行文本。
///
/// 区间由 `wrap_line` 保证落在 grapheme 边界上，故取用是安全的。
/// 提供这个函数是为了让调用方不必自己对正文做字节切片——
/// 那既触发 §0 禁令 5 的检查，也把「凭什么安全」的问题推给了每个调用点。
/// 越界或非边界时返回 None 而非 panic。
pub fn slice(text: &str, range: std::ops::Range<usize>) -> Option<&str> {
    text.get(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试内取行文本。区间来自 wrap_line，必定合法。
    fn line_at(text: &str, r: std::ops::Range<usize>) -> &str {
        slice(text, r).unwrap_or_default()
    }

    #[test]
    fn cjk_is_double_width() {
        assert_eq!(display_width("雪"), 2);
        assert_eq!(display_width("a"), 1);
        assert_eq!(display_width("雪a"), 3);
        assert_eq!(display_width("　"), 2, "全角空格也是 2");
    }

    #[test]
    fn counts_graphemes_not_chars() {
        // e + 组合锐音符 = 1 个 grapheme，2 个 char
        assert_eq!(grapheme_count("e\u{301}"), 1);
        assert_eq!("e\u{301}".chars().count(), 2);
        assert_eq!(grapheme_count("👨‍👩‍👧"), 1, "ZWJ 家族是一个 grapheme");
    }

    #[test]
    fn moves_cursor_by_grapheme() {
        let s = "雪a";
        assert_eq!(next_grapheme_boundary(s, 0), 3, "雪 占 3 字节");
        assert_eq!(next_grapheme_boundary(s, 3), 4);
        assert_eq!(prev_grapheme_boundary(s, 4), 3);
        assert_eq!(prev_grapheme_boundary(s, 3), 0);
    }

    #[test]
    fn cursor_movement_saturates_at_ends() {
        let s = "雪";
        assert_eq!(prev_grapheme_boundary(s, 0), 0, "开头再往前还是开头");
        assert_eq!(next_grapheme_boundary(s, 3), 3, "结尾再往后还是结尾");
    }

    /// 组合字符不得被拆开——否则光标会停在半个字符上，删除会切出乱码。
    #[test]
    fn cursor_does_not_split_combining_char() {
        let s = "e\u{301}x";
        assert_eq!(next_grapheme_boundary(s, 0), 3, "e+组合符应整体跳过");
    }

    #[test]
    fn wraps_by_display_width() {
        // 宽度 6 = 3 个中文字
        let text = "雪落了一夜";
        let lines = wrap_line(text, 6);
        let rendered: Vec<&str> = lines.iter().map(|r| line_at(text, r.clone())).collect();
        assert_eq!(rendered, ["雪落了", "一夜"]);
    }

    /// 禁则：行首不得出现句号。
    #[test]
    fn kinsoku_no_period_at_line_start() {
        // 宽度 6，「雪落了。」的句号本会被推到行首。
        let text = "雪落了。他推开门";
        let lines = wrap_line(text, 6);
        for r in &lines {
            let line = line_at(text, r.clone());
            if let Some(c) = line.chars().next() {
                assert!(
                    !forbidden_at_line_start(c),
                    "行首出现禁则字符 `{c}`，行={line:?}，全部={lines:?}"
                );
            }
        }
    }

    /// 禁则：行尾不得出现左引号。
    #[test]
    fn kinsoku_no_open_quote_at_line_end() {
        let text = "他说「雪落了一夜」";
        for width in [4, 6, 8, 10] {
            let lines = wrap_line(text, width);
            for r in &lines {
                let line = line_at(text, r.clone());
                if let Some(c) = line.chars().next_back() {
                    assert!(
                        !forbidden_at_line_end(c),
                        "宽度 {width} 时行尾出现禁则字符 `{c}`，行={line:?}"
                    );
                }
            }
        }
    }

    /// 折行不得丢字或重复——拼回去必须等于原文。
    #[test]
    fn wrap_preserves_all_text() {
        let texts = [
            "雪落了一夜。他推开门，风裹着雪灌进来，冷得刺骨。",
            "他说「雪落了」，然后走了。",
            "abc def ghi",
            "混合 mixed 文本 text 测试",
            "",
            "单",
        ];
        for text in texts {
            for width in [1, 2, 3, 5, 8, 20, 100] {
                let lines = wrap_line(text, width);
                let joined: String = lines.iter().map(|r| line_at(text, r.clone())).collect();
                assert_eq!(joined, text, "宽度 {width} 下折行丢字了: {text:?}");
            }
        }
    }

    /// 折行区间必须落在字符边界上，且首尾相接无空洞。
    #[test]
    fn wrap_ranges_are_contiguous_and_aligned() {
        let text = "雪落了一夜。他推开门，风裹着雪灌进来。";
        for width in [1, 4, 7, 13] {
            let lines = wrap_line(text, width);
            let mut expect = 0usize;
            for r in &lines {
                assert_eq!(r.start, expect, "区间不连续: {lines:?}");
                assert!(text.is_char_boundary(r.start), "start 不在字符边界");
                assert!(text.is_char_boundary(r.end), "end 不在字符边界");
                expect = r.end;
            }
            assert_eq!(expect, text.len(), "未覆盖到文末");
        }
    }

    /// 极窄宽度不得死循环或 panic——用户拖窗口会经过这些值。
    #[test]
    fn survives_degenerate_widths() {
        for width in [0, 1] {
            let lines = wrap_line("雪落了一夜。", width);
            assert!(!lines.is_empty());
        }
    }

    /// 以下三例均由 proptest 发现，手写用例全都漏掉了。留作回归。
    #[test]
    fn kinsoku_regressions_found_by_proptest() {
        // 1. 连续起始标点：退一格不够，必须循环回退。
        let text = "雪「「（「";
        for r in wrap_line(text, 6) {
            let line = line_at(text, r);
            if let Some(c) = line.chars().next_back()
                && forbidden_at_line_end(c)
            {
                assert!(
                    line.chars().all(forbidden_at_line_end),
                    "行尾禁则被破坏: {line:?}"
                );
            }
        }

        // 2. 空格产生断点后，收尾标点仍跑到了行首。
        let text = "雪 。雪";
        let lines = wrap_line(text, 4);
        for (i, r) in lines.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let line = line_at(text, r.clone());
            if let Some(c) = line.chars().next() {
                assert!(!forbidden_at_line_start(c), "行首禁则被破坏: {line:?}");
            }
        }

        // 3. 全是收尾标点 + 极窄宽度：不得 panic，不得丢字。
        let text = "。。";
        let joined: String = wrap_line(text, 1)
            .iter()
            .map(|r| line_at(text, r.clone()))
            .collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn breaks_at_spaces_for_latin() {
        let text = "hello world foo";
        let lines = wrap_line(text, 11);
        let rendered: Vec<&str> = lines.iter().map(|r| line_at(text, r.clone())).collect();
        assert_eq!(rendered[0].trim_end(), "hello world");
    }
}
