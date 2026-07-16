//! 排版的属性测试。见 doc.md §6.5、§10。
//!
//! §6.5 把幂等列为核心约束之首，§10 把它列为**发布门禁**，不允许标 ignore：
//!   `format(format(x)) == format(x)`
//!
//! 为什么这条这么重要：排版是在动用户的正文。不幂等意味着「按两次 F5 得到
//! 两个不同的稿子」——那用户就再也不敢按它了。

use proptest::prelude::*;

use mj_text::format::{FormatOptions, ParagraphIndent, apply, format, plan};

/// 生成含各种排版陷阱的随机文本。
fn messy_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just("，".to_string()),
            Just("!".to_string()),
            Just("?".to_string()),
            Just(",".to_string()),
            Just(".".to_string()),
            Just("...".to_string()),
            Just("…".to_string()),
            Just("……".to_string()),
            Just("。。。".to_string()),
            Just("-".to_string()),
            Just("--".to_string()),
            Just("—".to_string()),
            Just("——".to_string()),
            Just("\"".to_string()),
            Just("'".to_string()),
            Just("「".to_string()),
            Just("」".to_string()),
            Just("（".to_string()),
            Just("(".to_string()),
            Just(")".to_string()),
            Just(" ".to_string()),
            Just("　".to_string()), // 全角空格
            Just("  ".to_string()),
            Just("\n".to_string()),
            Just("\n\n".to_string()),
            Just("\n\n\n".to_string()),
            Just("a".to_string()),
            Just("abc".to_string()),
            Just("don't".to_string()),
            Just("v2.0".to_string()),
            Just("2026".to_string()),
            Just("２０２６".to_string()), // 全角数字
            Just("ａｂ".to_string()),     // 全角字母
            Just("\t".to_string()),
            Just("👍".to_string()),
            Just("e\u{301}".to_string()),
        ],
        0..25,
    )
    .prop_map(|v| v.concat())
}

/// 随机开关组合——默认关的规则也要覆盖到。
fn any_options() -> impl Strategy<Value = FormatOptions> {
    (
        any::<bool>(),
        any::<bool>(),
        prop_oneof![
            Just(ParagraphIndent::FullWidthTwo),
            Just(ParagraphIndent::None),
            Just(ParagraphIndent::Keep),
        ],
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(a, b, c, d, e, f, g, h, i, j, k, l)| FormatOptions {
            trim_trailing: a,
            collapse_blank: b,
            paragraph_indent: c,
            unify_ellipsis: d,
            unify_dash: e,
            punct_to_full_width: f,
            unify_quotes: g,
            cjk_latin_space: h,
            full_width_digits: i,
            strip_inline_space: j,
            repeat_punct: k,
            line_join: l,
        })
}

proptest! {
    /// **发布门禁**（§10）：排版幂等，默认配置。
    #[test]
    fn format_is_idempotent_with_defaults(text in messy_text()) {
        let opts = FormatOptions::default();
        let once = format(&text, &opts);
        let twice = format(&once, &opts);
        prop_assert_eq!(&once, &twice, "原文={:?}", text);
    }

    /// **发布门禁**：任意开关组合下都幂等。
    ///
    /// 默认关的规则（cjk_latin_space / repeat_punct / line_join）一旦被用户打开，
    /// 同样不能破坏幂等——它们是配置项，不是实验品。
    #[test]
    fn format_is_idempotent_with_any_options(text in messy_text(), opts in any_options()) {
        let once = format(&text, &opts);
        let twice = format(&once, &opts);
        prop_assert_eq!(&once, &twice, "原文={:?} 配置={:?}", text, opts);
    }

    /// 排好的文本再排一次，`plan` 应为空——这是幂等的更强形式：
    /// 不只是结果相同，而是压根不该再产生改动。
    #[test]
    fn formatted_text_yields_no_further_edits(text in messy_text()) {
        let opts = FormatOptions::default();
        let once = format(&text, &opts);
        let edits = plan(&once, &opts);
        prop_assert!(edits.is_empty(), "已排版的文本仍有改动: {:?} -> {:?}", once, edits);
    }

    /// 编辑之间永不重叠、按序排列（§6.5 实现要点）。
    #[test]
    fn plan_edits_never_overlap(text in messy_text(), opts in any_options()) {
        let edits = plan(&text, &opts);
        for w in edits.windows(2) {
            prop_assert!(
                w[0].range.end <= w[1].range.start,
                "编辑重叠: {:?} 与 {:?}", w[0], w[1]
            );
        }
    }

    /// 所有编辑的区间必须落在字符边界上——否则 apply 会切出乱码。
    #[test]
    fn plan_edits_are_on_char_boundaries(text in messy_text(), opts in any_options()) {
        for e in plan(&text, &opts) {
            prop_assert!(text.is_char_boundary(e.range.start), "start 非边界: {e:?}");
            prop_assert!(text.is_char_boundary(e.range.end), "end 非边界: {e:?}");
        }
    }

    /// 排版结果永远是合法 UTF-8，且不含孤立的半个字符。
    #[test]
    fn format_output_is_valid(text in messy_text(), opts in any_options()) {
        let out = format(&text, &opts);
        prop_assert!(mj_text::width::grapheme_offsets(&out).all(|(_, g)| !g.is_empty()));
    }

    /// 空编辑列表 = 原文不变。
    #[test]
    fn apply_with_no_edits_is_identity(text in messy_text()) {
        prop_assert_eq!(apply(&text, &[]), text);
    }

    /// 全关配置下排版不动一个字——用户把开关全关了，就该什么都不发生。
    #[test]
    fn all_rules_off_changes_nothing(text in messy_text()) {
        prop_assert_eq!(format(&text, &FormatOptions::none()), text);
    }

    /// 排版不该凭空吞掉汉字。标点会被改写（这是排版的本职），
    /// 但一个汉字都不该消失。
    #[test]
    fn format_never_loses_han_characters(text in messy_text()) {
        let opts = FormatOptions::default();
        let before = mj_text::count::count(&text).han;
        let after = mj_text::count::count(&format(&text, &opts)).han;
        prop_assert_eq!(before, after, "汉字数变了: {:?}", text);
    }
}
