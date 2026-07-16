//! 中文输入辅助：成对符号自动补全。见 doc.md §6.3。
//!
//! `[MUST]` 输入 `「` 自动补 `」`、`（` 补 `）`、`《` 补 `》`，成对引号状态感知（可关）。
//!
//! 纯函数：输入「敲了什么 + 上下文」，输出「该插入什么」，不碰缓冲也不碰磁盘。

/// 成对符号表。左 → 右。
const PAIRS: &[(char, char)] = &[
    ('「', '」'),
    ('『', '』'),
    ('（', '）'),
    ('《', '》'),
    ('【', '】'),
    ('〈', '〉'),
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
];

/// 状态感知的成对引号：同一个字符既是开也是闭，靠上下文判断。
const SYMMETRIC: &[char] = &['"', '\'', '“', '”', '‘', '’'];

/// 敲下 `c` 之后应做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairAction {
    /// 照常插入这个字符。
    Insert(char),
    /// 插入 `open`，并在其后补 `close`，光标停在两者之间。
    InsertPair(char, char),
    /// 光标正好在 `c` 之前——跳过它而不是插入一个重复的。
    /// 用户敲完 `「你好` 再敲 `」` 时，期待的是「越过去」而非「多一个」。
    Skip(char),
}

/// 左符号对应的右符号。
pub fn closing_for(open: char) -> Option<char> {
    PAIRS.iter().find(|(l, _)| *l == open).map(|(_, r)| *r)
}

/// 是否是右符号。
pub fn is_closing(c: char) -> bool {
    PAIRS.iter().any(|(_, r)| *r == c)
}

/// 判断敲下 `c` 后该做什么。
///
/// `next` 是光标右侧的第一个字符（无则 None）。
///
/// 何时**不**自动补：
/// - 右侧紧邻的是文字（非空白、非右符号）时不补——用户多半是想把
///   后面已有的内容括起来，此时补一个右符号只会挡路。
///   这是「拿不准就不动」（§6.5 的排版原则，同样适用于这里）。
pub fn on_input(c: char, next: Option<char>) -> PairAction {
    // 敲右符号，而右边正好就是它 → 跳过。
    if is_closing(c) && next == Some(c) {
        return PairAction::Skip(c);
    }

    if let Some(close) = closing_for(c) {
        return if should_auto_close(next) {
            PairAction::InsertPair(c, close)
        } else {
            PairAction::Insert(c)
        };
    }

    PairAction::Insert(c)
}

/// 右侧是什么时才自动补右符号。
fn should_auto_close(next: Option<char>) -> bool {
    match next {
        // 行尾/段尾：补。
        None => true,
        // 右边是空白或右符号：补（如在 `（）` 里再套一层）。
        Some(n) => n.is_whitespace() || is_closing(n) || SYMMETRIC.contains(&n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_closes_cjk_brackets_at_line_end() {
        // §6.3 [MUST] 点名的三种。
        assert_eq!(on_input('「', None), PairAction::InsertPair('「', '」'));
        assert_eq!(on_input('（', None), PairAction::InsertPair('（', '）'));
        assert_eq!(on_input('《', None), PairAction::InsertPair('《', '》'));
    }

    #[test]
    fn auto_closes_other_pairs() {
        assert_eq!(on_input('『', None), PairAction::InsertPair('『', '』'));
        assert_eq!(on_input('【', None), PairAction::InsertPair('【', '】'));
        assert_eq!(on_input('(', None), PairAction::InsertPair('(', ')'));
    }

    #[test]
    fn skips_over_existing_closing() {
        // 敲完「你好，光标在 」 之前，再敲 」 应跳过而非再插一个。
        assert_eq!(on_input('」', Some('」')), PairAction::Skip('」'));
        assert_eq!(on_input('）', Some('）')), PairAction::Skip('）'));
    }

    /// 右边是文字时不补——用户多半想把后面的内容括起来。
    #[test]
    fn does_not_close_before_text() {
        assert_eq!(on_input('「', Some('雪')), PairAction::Insert('「'));
        assert_eq!(on_input('（', Some('a')), PairAction::Insert('（'));
    }

    #[test]
    fn closes_before_whitespace() {
        assert_eq!(
            on_input('「', Some(' ')),
            PairAction::InsertPair('「', '」')
        );
        assert_eq!(
            on_input('「', Some('\n')),
            PairAction::InsertPair('「', '」')
        );
        // 全角空格（段首缩进）同理。
        assert_eq!(
            on_input('「', Some('　')),
            PairAction::InsertPair('「', '」')
        );
    }

    /// 嵌套：在 `（|）` 里敲 `「` 应补成 `（「|」）`。
    #[test]
    fn closes_before_another_closing() {
        assert_eq!(
            on_input('「', Some('）')),
            PairAction::InsertPair('「', '」')
        );
    }

    #[test]
    fn ordinary_chars_pass_through() {
        assert_eq!(on_input('雪', None), PairAction::Insert('雪'));
        assert_eq!(on_input('。', Some('雪')), PairAction::Insert('。'));
    }

    /// 敲右符号但右边不是它 → 照常插入（用户在补一个漏掉的右括号）。
    #[test]
    fn closing_without_match_is_inserted() {
        assert_eq!(on_input('」', Some('雪')), PairAction::Insert('」'));
        assert_eq!(on_input('」', None), PairAction::Insert('」'));
    }

    #[test]
    fn closing_for_maps_correctly() {
        assert_eq!(closing_for('「'), Some('」'));
        assert_eq!(closing_for('《'), Some('》'));
        assert_eq!(closing_for('雪'), None);
        assert_eq!(closing_for('」'), None, "右符号没有对应的右符号");
    }

    #[test]
    fn pairs_are_bijective() {
        // 左右不得重复，否则查表会取到错的那个。
        use std::collections::HashSet;
        let lefts: HashSet<_> = PAIRS.iter().map(|(l, _)| l).collect();
        let rights: HashSet<_> = PAIRS.iter().map(|(_, r)| r).collect();
        assert_eq!(lefts.len(), PAIRS.len(), "左符号有重复");
        assert_eq!(rights.len(), PAIRS.len(), "右符号有重复");
    }
}
