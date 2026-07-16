//! 字数统计。见 doc.md §6.4。
//!
//! 口径定义必须写进 UI 的说明浮层——用户拿这个数去和发布平台对，
//! 对不上时他需要知道**我们是怎么数的**，而不是怀疑自己的稿子少了字。
//!
//! 全部按 grapheme cluster 计数（§0 禁令 5）：`é`（e + 组合符）是一个字，
//! emoji 家族也是一个字。按 char 数会把它们算成 2 个、4 个。

use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory as G, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

/// 四口径 + 段落/句子数（§6.4）。
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WordCount {
    /// 含标点：去除所有换行、制表、行首缩进空白后，按 grapheme cluster 计数。
    /// 半角空格计入（英文语境需要），全角空格 U+3000 不计入（视为缩进符）。
    pub with_punct: usize,
    /// 不含标点：在 with_punct 基础上，再排除 Unicode 类别 P*、S*、Z*。
    pub no_punct: usize,
    /// 纯汉字数：CJK 统一汉字及扩展区。
    pub han: usize,
    /// 英文单词数：按 `\b[A-Za-z']+\b`。
    pub latin_words: usize,
    pub paragraphs: usize,
    /// 句数：以 。！？…… 及其后引号收尾。
    pub sentences: usize,
}

impl std::ops::Add for WordCount {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self {
            with_punct: self.with_punct + o.with_punct,
            no_punct: self.no_punct + o.no_punct,
            han: self.han + o.han,
            latin_words: self.latin_words + o.latin_words,
            paragraphs: self.paragraphs + o.paragraphs,
            sentences: self.sentences + o.sentences,
        }
    }
}

impl std::ops::AddAssign for WordCount {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

/// 全量统计。
pub fn count(text: &str) -> WordCount {
    let mut wc = WordCount {
        paragraphs: count_paragraphs(text),
        sentences: count_sentences(text),
        latin_words: count_latin_words(text),
        ..Default::default()
    };

    for g in text.graphemes(true) {
        if !is_counted(g) {
            continue;
        }
        wc.with_punct += 1;
        if !is_punct_or_symbol(g) {
            wc.no_punct += 1;
        }
        if is_han(g) {
            wc.han += 1;
        }
    }
    wc
}

/// 「含标点」口径。`save_body` 写进 front matter 的 `words` 就是它——
/// 与状态栏「本章 3,128」同源，两处必须是同一个数。
pub fn count_with_punct(text: &str) -> usize {
    text.graphemes(true).filter(|g| is_counted(g)).count()
}

/// M1 起沿用的名字，保留以免调用方全改。
pub fn count_han_and_punct(text: &str) -> usize {
    count_with_punct(text)
}

/// 增量统计：只对受影响的段落重算（§6.4）。
///
/// 与全量结果必须严格一致——由属性测试保证。
///
/// 注意 `paragraphs` 不能这样加减：段落数取决于整篇的换行结构，
/// 改一段可能让段落数不变（改内容）或改变（增删空行），
/// 而调用方给的只是「哪些段变了」。故这里只增量算前五项，
/// 段落数由调用方另行提供。
pub fn count_incremental(prev: WordCount, old_paras: &[&str], new_paras: &[&str]) -> WordCount {
    let mut wc = prev;

    for p in old_paras {
        let c = count_paragraph_only(p);
        wc.with_punct = wc.with_punct.saturating_sub(c.with_punct);
        wc.no_punct = wc.no_punct.saturating_sub(c.no_punct);
        wc.han = wc.han.saturating_sub(c.han);
        wc.latin_words = wc.latin_words.saturating_sub(c.latin_words);
        wc.sentences = wc.sentences.saturating_sub(c.sentences);
    }
    for p in new_paras {
        let c = count_paragraph_only(p);
        wc.with_punct += c.with_punct;
        wc.no_punct += c.no_punct;
        wc.han += c.han;
        wc.latin_words += c.latin_words;
        wc.sentences += c.sentences;
    }
    wc
}

/// 单段的统计（不含 paragraphs——那是整篇的属性）。
fn count_paragraph_only(p: &str) -> WordCount {
    let mut wc = count(p);
    wc.paragraphs = 0;
    wc
}

/// 该 grapheme 是否计入字数。
fn is_counted(g: &str) -> bool {
    // `\r\n` 是**一个** grapheme cluster，不是两个——
    // 故判据是「整个 cluster 全由不计入的字符组成则不计」，
    // 而不能假设「多码位 cluster 一定是组合字/emoji」。
    !g.chars().all(is_skipped)
}

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

/// 是否属于 P*（标点）、S*（符号）、Z*（分隔符）——no_punct 要排除的。
///
/// 判据看 cluster 的**首字符**：组合序列的类别由基字符决定
/// （`é` 的基字符是字母，不该因为带了组合符就被当成符号）。
fn is_punct_or_symbol(g: &str) -> bool {
    let Some(c) = g.chars().next() else {
        return false;
    };
    matches!(
        get_general_category(c),
        // P*
        G::ConnectorPunctuation
            | G::DashPunctuation
            | G::ClosePunctuation
            | G::FinalPunctuation
            | G::InitialPunctuation
            | G::OtherPunctuation
            | G::OpenPunctuation
            // S*
            | G::CurrencySymbol
            | G::ModifierSymbol
            | G::MathSymbol
            | G::OtherSymbol
            // Z*
            | G::LineSeparator
            | G::ParagraphSeparator
            | G::SpaceSeparator
    )
}

/// 是否是汉字（CJK 统一汉字及扩展区）。
fn is_han(g: &str) -> bool {
    g.chars().next().is_some_and(|c| {
        let u = c as u32;
        // 基本区
        (0x4E00..=0x9FFF).contains(&u)
            // 扩展 A
            || (0x3400..=0x4DBF).contains(&u)
            // 扩展 B~F（辅助平面）
            || (0x20000..=0x2EBEF).contains(&u)
            // 兼容表意文字
            || (0xF900..=0xFAFF).contains(&u)
    })
}

/// 段落数：非空行的条数。
///
/// 连续空行不算多个段落——它们是排版留白，不是内容。
fn count_paragraphs(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

/// 英文单词数：`\b[A-Za-z']+\b`。
///
/// 不引 regex：这条规则简单到手写更快，且 count 在编辑时是热路径
/// （§6.4 要求单次 < 1ms）。
fn count_latin_words(text: &str) -> usize {
    let mut n = 0;
    let mut prev_letter = false;
    for c in text.chars() {
        let is_letter = c.is_ascii_alphabetic() || c == '\'';
        // 词的第一个字母处 +1。
        if is_letter && !prev_letter {
            n += 1;
        }
        prev_letter = is_letter;
    }
    n
}

/// 句数：以 。！？…… 及其后引号收尾（§6.4）。
fn count_sentences(text: &str) -> usize {
    const ENDERS: &[char] = &['。', '！', '？', '.', '!', '?', '…'];
    let mut n = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !ENDERS.contains(&c) {
            continue;
        }
        // 吞掉连续的结束符：`……` 是一个省略号，`！！！` 是一句。
        while chars.peek().is_some_and(|n| ENDERS.contains(n)) {
            chars.next();
        }
        // 吞掉其后的引号：`「你来了。」` 的句号在引号内，整体算一句。
        while chars
            .peek()
            .is_some_and(|n| matches!(n, '」' | '』' | '"' | '”' | '’' | '\'' | '）' | ')'))
        {
            chars.next();
        }
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    // ---- with_punct ----

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

    #[test]
    fn includes_halfwidth_space() {
        assert_eq!(count_with_punct("a b"), 3);
    }

    #[test]
    fn counts_combining_marks_as_one() {
        assert_eq!(count_with_punct("e\u{301}"), 1);
    }

    #[test]
    fn counts_emoji_as_one() {
        assert_eq!(count_with_punct("👨‍👩‍👧"), 1, "ZWJ 家族应算一个");
    }

    // ---- no_punct ----

    #[test]
    fn no_punct_excludes_chinese_punctuation() {
        // 雪落了一夜。他推开门，风灌进来。= 13 字 + 3 个标点（。，。）
        let wc = count("雪落了一夜。他推开门，风灌进来。");
        assert_eq!(wc.with_punct, 16);
        assert_eq!(wc.no_punct, 13, "应排除 3 个标点");
        assert_eq!(wc.han, 13);
    }

    #[test]
    fn no_punct_excludes_symbols_and_spaces() {
        let wc = count("a b+c");
        // with_punct: a,空格,b,+,c = 5
        assert_eq!(wc.with_punct, 5);
        // no_punct 排除空格(Z*)与 +(S*)
        assert_eq!(wc.no_punct, 3);
    }

    /// emoji 属 S*，按 §6.4 的定义应从 no_punct 排除。
    #[test]
    fn no_punct_excludes_emoji() {
        let wc = count("雪👍");
        assert_eq!(wc.with_punct, 2);
        assert_eq!(wc.no_punct, 1, "emoji 是 S* 类别");
    }

    /// 组合字符的类别由基字符决定，不该因带组合符被误判为符号。
    #[test]
    fn no_punct_keeps_combining_letters() {
        let wc = count("e\u{301}");
        assert_eq!(wc.no_punct, 1, "é 是字母不是符号");
    }

    // ---- han ----

    #[test]
    fn counts_han_only() {
        let wc = count("雪落了一夜。abc 123");
        assert_eq!(wc.han, 5, "只数汉字");
    }

    #[test]
    fn han_excludes_kana_and_latin() {
        let wc = count("雪ゆきsnow");
        assert_eq!(wc.han, 1, "假名与拉丁字母不是汉字");
    }

    #[test]
    fn han_includes_extension_a() {
        // U+3400 扩展 A 区首字
        let wc = count("㐀");
        assert_eq!(wc.han, 1);
    }

    // ---- latin_words ----

    #[test]
    fn counts_latin_words() {
        assert_eq!(count("hello world").latin_words, 2);
        assert_eq!(count("don't stop").latin_words, 2, "撇号在词内");
    }

    #[test]
    fn latin_words_ignores_chinese() {
        assert_eq!(count("雪落了一夜").latin_words, 0);
    }

    #[test]
    fn latin_words_across_punctuation() {
        assert_eq!(count("hello, world!").latin_words, 2);
        assert_eq!(count("a-b").latin_words, 2, "连字符分词");
    }

    // ---- paragraphs / sentences ----

    #[test]
    fn counts_paragraphs_ignoring_blank_lines() {
        let text = "　　第一段。\n\n　　第二段。\n\n\n　　第三段。";
        assert_eq!(count(text).paragraphs, 3, "连续空行不算段落");
    }

    #[test]
    fn counts_sentences() {
        assert_eq!(count("雪落了。他推开门。").sentences, 2);
        assert_eq!(count("你来了？是的！").sentences, 2);
    }

    /// `……` 是一个省略号，不是两句。
    #[test]
    fn ellipsis_is_one_sentence() {
        assert_eq!(count("他说……").sentences, 1);
        assert_eq!(count("真的！！！").sentences, 1, "连续感叹号算一句");
    }

    /// 句末标点在引号内：整体算一句（§6.4「及其后引号收尾」）。
    #[test]
    fn closing_quote_after_period_is_same_sentence() {
        assert_eq!(count("「你来了。」").sentences, 1);
        assert_eq!(count("「你来了。」他说。").sentences, 2);
    }

    // ---- 综合 ----

    #[test]
    fn realistic_paragraph() {
        let text = "　　雪落了一夜。\n　　他推开门，风裹着雪灌进来，冷得刺骨。\n";
        let wc = count(text);
        // 第一段：雪落了一夜。= 5 字 + 1 标点 = 6
        // 第二段：他推开门，风裹着雪灌进来，冷得刺骨。= 15 字 + 3 标点 = 18
        assert_eq!(wc.with_punct, 6 + 18);
        assert_eq!(wc.no_punct, 5 + 15);
        assert_eq!(wc.han, 5 + 15, "只数汉字，不含标点");
        assert_eq!(wc.paragraphs, 2);
        assert_eq!(wc.sentences, 2);
    }

    #[test]
    fn empty_text_is_all_zero() {
        assert_eq!(count(""), WordCount::default());
        assert_eq!(count("\n\n"), WordCount::default());
        assert_eq!(count("　　"), WordCount::default(), "只有缩进符");
    }

    #[test]
    fn cached_count_matches_with_punct() {
        for s in ["　　雪落了一夜。", "", "a b c", "雪\n落"] {
            assert_eq!(count_han_and_punct(s), count(s).with_punct, "{s:?}");
        }
    }

    // ---- 加法 ----

    #[test]
    fn word_counts_add() {
        let a = count("雪落。");
        let b = count("他来。");
        let sum = a + b;
        assert_eq!(sum.with_punct, a.with_punct + b.with_punct);
        assert_eq!(sum.han, 4);
        assert_eq!(sum.sentences, 2);
    }

    // ---- 增量 ----

    #[test]
    fn incremental_matches_full_for_simple_edit() {
        let old_text = "　　雪落了一夜。";
        let new_text = "　　雪落了一夜。他推开门。";

        let prev = count(old_text);
        let inc = count_incremental(prev, &["　　雪落了一夜。"], &["　　雪落了一夜。他推开门。"]);
        let full = count(new_text);

        assert_eq!(inc.with_punct, full.with_punct);
        assert_eq!(inc.no_punct, full.no_punct);
        assert_eq!(inc.han, full.han);
        assert_eq!(inc.sentences, full.sentences);
    }

    #[test]
    fn incremental_handles_deletion() {
        let prev = count("　　第一段。\n　　第二段。");
        let inc = count_incremental(prev, &["　　第二段。"], &[]);
        let full = count("　　第一段。");
        assert_eq!(inc.with_punct, full.with_punct);
        assert_eq!(inc.han, full.han);
    }

    /// 删多了不该下溢 panic（release 下开着 overflow-checks）。
    #[test]
    fn incremental_saturates_instead_of_underflowing() {
        let prev = WordCount::default();
        let inc = count_incremental(prev, &["很长的一段文字"], &[]);
        assert_eq!(inc.with_punct, 0, "应饱和到 0 而非下溢");
    }
}
