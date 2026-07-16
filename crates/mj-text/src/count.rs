//! 字数统计。见 doc.md §6.4。
//!
//! **M1 只实现 `words` 缓存所需的「含标点」口径**；四口径全量、增量统计、
//! 与 `WordCount` 结构体是 M2 的内容（§11）。此处先把口径定义钉死，
//! 免得 M1 的缓存值与 M2 的统计对不上。

use unicode_segmentation::UnicodeSegmentation;

/// 「含标点」口径（§6.4）：
/// 去除所有换行、制表、行首缩进空白后，按 grapheme cluster 计数。
/// 半角空格计入（英文语境需要），全角空格 U+3000 不计入（视为缩进符）。
///
/// 按 grapheme 而非 char：`é`（e + 组合符）是一个字，emoji 家族也是一个字。
/// 按 char 会把它们算成 2、4 个（§0 禁令 5 的同源问题）。
pub fn count_with_punct(text: &str) -> usize {
    text.graphemes(true).filter(|g| is_counted(g)).count()
}

/// M1 内部用：`save_body` 写进 front matter 的 `words` 缓存值。
///
/// 就是「含标点」口径——§5.2 的 `words: 3128` 与状态栏「本章 3,128」同源，
/// 两处必须是同一个数，否则用户会发现文件里的数和界面上的数对不上。
pub fn count_han_and_punct(text: &str) -> usize {
    count_with_punct(text)
}

/// 该 grapheme 是否计入字数。
fn is_counted(g: &str) -> bool {
    // 注意 `\r\n` 是**一个** grapheme cluster，不是两个——
    // 故不能假设「多码位 grapheme 一定是组合字/emoji」而直接计数。
    // 判据改为：整个 cluster 全由不计入的字符组成，则不计。
    !g.chars().all(is_skipped)
}

/// 不计入字数的字符。
fn is_skipped(c: char) -> bool {
    matches!(
        c,
        // 换行、制表：不是字。
        '\n' | '\r' | '\t'
        // 全角空格：中文段首缩进符，不是字（§6.4 明言）。
        // 半角空格则计入（§6.4：英文语境需要），故不在此列。
        | '\u{3000}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_chinese_chars() {
        assert_eq!(count_with_punct("雪落了一夜"), 5);
    }

    #[test]
    fn counts_punctuation() {
        assert_eq!(count_with_punct("雪落了一夜。"), 6);
    }

    /// 全角空格是缩进符，不计入——这是中文正文最常见的段首形态。
    #[test]
    fn excludes_fullwidth_space() {
        assert_eq!(
            count_with_punct("　　雪落了一夜。"),
            6,
            "段首两个全角空格不计"
        );
    }

    #[test]
    fn excludes_newlines_and_tabs() {
        assert_eq!(count_with_punct("雪\n落\t了"), 3);
        assert_eq!(count_with_punct("雪\r\n落"), 2);
    }

    /// 半角空格计入：英文语境需要（§6.4）。
    #[test]
    fn includes_halfwidth_space() {
        assert_eq!(count_with_punct("a b"), 3);
    }

    /// 组合字符算一个字，不是两个。
    #[test]
    fn counts_combining_marks_as_one() {
        // e + U+0301（组合锐音符）
        assert_eq!(count_with_punct("e\u{301}"), 1);
    }

    /// emoji（含 ZWJ 家族序列）算一个字。
    #[test]
    fn counts_emoji_as_one() {
        assert_eq!(count_with_punct("👨‍👩‍👧"), 1, "ZWJ 家族应算一个");
        assert_eq!(count_with_punct("雪👍"), 2);
    }

    #[test]
    fn empty_text_is_zero() {
        assert_eq!(count_with_punct(""), 0);
        assert_eq!(count_with_punct("\n\n"), 0);
        assert_eq!(count_with_punct("　　"), 0, "只有缩进符");
    }

    /// 缓存值与「含标点」口径必须同源——否则文件里的 words 与状态栏对不上。
    #[test]
    fn cached_count_matches_with_punct() {
        let samples = ["　　雪落了一夜。", "", "a b c", "雪\n落"];
        for s in samples {
            assert_eq!(count_han_and_punct(s), count_with_punct(s), "{s:?}");
        }
    }

    #[test]
    fn realistic_paragraph() {
        let text = "　　雪落了一夜。\n　　他推开门，风裹着雪灌进来，冷得刺骨。\n";
        // 第一段 6（雪落了一夜。）+ 第二段 18（他推开门，风裹着雪灌进来，冷得刺骨。）
        assert_eq!(count_with_punct(text), 6 + 18);
    }
}
