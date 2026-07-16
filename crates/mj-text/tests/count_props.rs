//! 字数统计的属性测试。见 doc.md §10、§6.4。
//!
//! §6.4 明言：「增量统计……与全量结果必须严格一致（属性测试保证）」。
//! §10 把这条列为**发布门禁**，不允许标 ignore。
//!
//! 为什么值得这样较真：用户会拿这个数去和发布平台对。差一个字，
//! 他信任的就不是数字而是自己的猜测——那这个功能就废了。

use proptest::prelude::*;

use mj_text::count::{WordCount, count, count_incremental};

/// 生成含中英标点、emoji、组合字符的随机段落。
fn paragraph() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just("，".to_string()),
            Just("！".to_string()),
            Just("……".to_string()),
            Just("「".to_string()),
            Just("」".to_string()),
            Just("a".to_string()),
            Just("hello".to_string()),
            Just(" ".to_string()),
            Just("　".to_string()), // 全角空格
            Just("👍".to_string()),
            Just("👨‍👩‍👧".to_string()),       // ZWJ 家族
            Just("e\u{301}".to_string()), // 组合字符
            Just("1".to_string()),
            Just("+".to_string()),
        ],
        0..15,
    )
    .prop_map(|v| v.concat())
}

fn paragraphs() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(paragraph(), 0..6)
}

/// 除 paragraphs 外的五项（增量统计不负责段落数——见 count_incremental 的文档）。
fn same_except_paragraphs(a: &WordCount, b: &WordCount) -> bool {
    a.with_punct == b.with_punct
        && a.no_punct == b.no_punct
        && a.han == b.han
        && a.latin_words == b.latin_words
        && a.sentences == b.sentences
}

proptest! {
    /// **发布门禁**（§10）：增量统计与全量结果严格一致。
    ///
    /// 场景：把一整篇的某些段落换成新的，增量算出来的必须与重算全篇相同。
    #[test]
    fn incremental_equals_full(
        keep in paragraphs(),
        old in paragraphs(),
        new in paragraphs(),
    ) {
        // 旧全文 = keep + old；新全文 = keep + new。
        let old_text = [keep.clone(), old.clone()].concat().join("\n");
        let new_text = [keep.clone(), new.clone()].concat().join("\n");

        let prev = count(&old_text);
        let old_refs: Vec<&str> = old.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new.iter().map(String::as_str).collect();
        let inc = count_incremental(prev, &old_refs, &new_refs);

        let full = count(&new_text);
        prop_assert!(
            same_except_paragraphs(&inc, &full),
            "增量 {:?} != 全量 {:?}\n旧={:?}\n新={:?}", inc, full, old_text, new_text
        );
    }

    /// 统计是纯函数：同一输入永远同一结果。
    #[test]
    fn count_is_deterministic(text in paragraph()) {
        prop_assert_eq!(count(&text), count(&text));
    }

    /// no_punct 永远不超过 with_punct——它是后者的子集。
    #[test]
    fn no_punct_never_exceeds_with_punct(text in paragraph()) {
        let wc = count(&text);
        prop_assert!(wc.no_punct <= wc.with_punct);
        prop_assert!(wc.han <= wc.no_punct, "汉字都不是标点，必在 no_punct 内");
    }

    /// 字数不会凭空多出来：不可能超过 grapheme 总数。
    #[test]
    fn with_punct_never_exceeds_grapheme_count(text in paragraph()) {
        let wc = count(&text);
        prop_assert!(wc.with_punct <= mj_text::width::grapheme_count(&text));
    }

    /// 拼接两段的字数 = 各自字数之和（换行分隔，不产生新字）。
    ///
    /// 这条保证了「本卷 = 各章之和」「全书 = 各卷之和」的加法成立——
    /// 状态栏的三个数字若不自洽，用户立刻会发现。
    #[test]
    fn counts_are_additive_across_paragraphs(a in paragraph(), b in paragraph()) {
        let joined = format!("{a}\n{b}");
        let sum = count(&a) + count(&b);
        let whole = count(&joined);

        prop_assert_eq!(whole.with_punct, sum.with_punct);
        prop_assert_eq!(whole.no_punct, sum.no_punct);
        prop_assert_eq!(whole.han, sum.han);
    }

    /// 空白与换行不产生字数。
    #[test]
    fn whitespace_adds_nothing(text in paragraph()) {
        let base = count(&text);
        let padded = count(&format!("\n\n{text}\n\n"));
        prop_assert_eq!(padded.with_punct, base.with_punct);
        prop_assert_eq!(padded.han, base.han);
    }
}
