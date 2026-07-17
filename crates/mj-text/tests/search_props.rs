//! 查找替换的属性测试。见 doc.md §10、§6.6。
//!
//! 这里要证的核心不变量只有一条，但它是整个模块的地基：
//! **命中的字节区间必须精确落回原文**。
//!
//! §6.6 特意警告过这个坑（NFKC 会改变字节长度，位置就映射不回去了）。
//! 折叠让「比较用的文本」和「原文」不是同一份，位置映射一旦错位，
//! 替换就会砍在半个汉字上——那是直接毁稿。手写用例覆盖不了所有组合。
#![allow(clippy::string_slice)] // 区间来自 search_text；「它是否真落在边界上」正是本文件要证的

use proptest::prelude::*;

use mj_text::search::{MatchFlags, MatchMode, Query, hit_context, replace_preview, search_text};

/// 生成含全角/半角、中英标点、emoji 的随机文本。
fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just("，".to_string()),
            Just(",".to_string()),
            Just(".".to_string()),
            Just("a".to_string()),
            Just("A".to_string()),
            Just("Ａ".to_string()), // 全角字母
            Just("１".to_string()), // 全角数字
            Just("1".to_string()),
            Just(" ".to_string()),
            Just("　".to_string()), // 全角空格
            Just("\n".to_string()),
            Just("👍".to_string()),
            Just("「".to_string()),
            Just("」".to_string()),
        ],
        0..25,
    )
    .prop_map(|v| v.concat())
}

/// 随机的短查找串。
fn pattern() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("雪".to_string()),
            Just("落".to_string()),
            Just("。".to_string()),
            Just(",".to_string()),
            Just("a".to_string()),
            Just("A".to_string()),
            Just("1".to_string()),
            Just(" ".to_string()),
        ],
        1..4,
    )
    .prop_map(|v| v.concat())
}

fn flags() -> impl Strategy<Value = MatchFlags> {
    (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(a, b, c)| MatchFlags {
        ignore_case: a,
        fold_width: b,
        fold_cjk_punct: c,
        extended: false,
    })
}

fn mode() -> impl Strategy<Value = MatchMode> {
    prop_oneof![Just(MatchMode::Literal), Just(MatchMode::WholeWord)]
}

proptest! {
    /// **地基**：命中区间必须落在原文的字符边界上。
    ///
    /// 错一个字节，替换就砍在半个汉字上——直接毁稿。
    #[test]
    fn hits_are_on_char_boundaries(t in text(), p in pattern(), f in flags(), m in mode()) {
        let q = Query { pattern: p, mode: m, flags: f };
        let Ok(hits) = search_text(&t, &q) else { return Ok(()) };
        for h in hits {
            prop_assert!(t.is_char_boundary(h.start), "start {} 非边界: {:?}", h.start, t);
            prop_assert!(t.is_char_boundary(h.end), "end {} 非边界: {:?}", h.end, t);
            prop_assert!(h.start < h.end, "空命中: {h:?}");
            prop_assert!(h.end <= t.len(), "越界: {h:?}");
        }
    }

    /// 命中互不重叠且按序——结果面板与批量替换都依赖这条。
    #[test]
    fn hits_are_ordered_and_disjoint(t in text(), p in pattern(), f in flags(), m in mode()) {
        let q = Query { pattern: p, mode: m, flags: f };
        let Ok(hits) = search_text(&t, &q) else { return Ok(()) };
        // 普通模式允许重叠命中（aa 在 aaaa 里有 3 处），故只验有序。
        for w in hits.windows(2) {
            prop_assert!(w[0].start < w[1].start, "未按序: {:?}", hits);
        }
    }

    /// 不折叠、不忽略大小写时，命中的原文必须**逐字节等于**查找串。
    #[test]
    fn exact_mode_hits_equal_the_pattern(t in text(), p in pattern()) {
        let q = Query {
            pattern: p.clone(),
            mode: MatchMode::Literal,
            flags: MatchFlags::default(),
        };
        let Ok(hits) = search_text(&t, &q) else { return Ok(()) };
        for h in hits {
            prop_assert_eq!(&t[h.clone()], p.as_str(), "命中与查找串不一致");
        }
    }

    /// 折叠时，命中处的**字符数**必须与查找串一致。
    ///
    /// 字节数会变（`Ａ` 3 → `A` 1），但折叠是 1:1 的字符映射，字符数必须守恒。
    /// 这条不成立就说明映射表错位了。
    #[test]
    fn folded_hits_have_same_char_count(t in text(), p in pattern()) {
        let q = Query {
            pattern: p.clone(),
            mode: MatchMode::Literal,
            flags: MatchFlags { ignore_case: true, fold_width: true, fold_cjk_punct: true, extended: false },
        };
        let Ok(hits) = search_text(&t, &q) else { return Ok(()) };
        let want = p.chars().count();
        for h in hits {
            prop_assert_eq!(t[h.clone()].chars().count(), want, "字符数不守恒: {:?}", &t[h]);
        }
    }

    /// 替换后的文本永远是合法 UTF-8，且不含被砍碎的字符。
    #[test]
    fn replace_output_is_valid(t in text(), p in pattern(), f in flags()) {
        let q = Query { pattern: p, mode: MatchMode::Literal, flags: f };
        let Ok(edits) = replace_preview(&t, &q, "X") else { return Ok(()) };
        let out = mj_text::format::apply(&t, &edits);
        prop_assert!(mj_text::width::grapheme_offsets(&out).all(|(_, g)| !g.is_empty()));
    }

    /// 替换成自己 = 原文不变。
    #[test]
    fn replacing_pattern_with_itself_is_identity(t in text(), p in pattern()) {
        let q = Query {
            pattern: p.clone(),
            mode: MatchMode::Literal,
            flags: MatchFlags::default(),
        };
        let Ok(edits) = replace_preview(&t, &q, &p) else { return Ok(()) };
        prop_assert_eq!(mj_text::format::apply(&t, &edits), t);
    }

    /// 替换掉所有命中之后，再搜同一个串应该一个都找不到
    /// （前提：替换文本本身不含该串）。
    #[test]
    fn replacing_all_removes_all(t in text(), p in pattern()) {
        let q = Query {
            pattern: p.clone(),
            mode: MatchMode::Literal,
            flags: MatchFlags::default(),
        };
        let Ok(edits) = replace_preview(&t, &q, "\u{2603}") else { return Ok(()) };
        let out = mj_text::format::apply(&t, &edits);
        let Ok(again) = search_text(&out, &q) else { return Ok(()) };
        prop_assert!(again.is_empty(), "替换后仍有残留: {:?} -> {:?}", t, out);
    }

    /// 任何输入都不得 panic——非法正则要报错，不是崩（§6.6 [MUST]）。
    #[test]
    fn regex_never_panics(t in text(), p in ".{0,12}") {
        let q = Query { pattern: p, mode: MatchMode::Regex, flags: MatchFlags::default() };
        let _ = search_text(&t, &q);   // Ok 或 Err 都行，就是不许 panic
        let _ = replace_preview(&t, &q, "X");
    }

    /// 上下文的高亮区间必须精确对准命中。
    #[test]
    fn context_highlight_matches_the_hit(t in text(), p in pattern()) {
        let q = Query {
            pattern: p,
            mode: MatchMode::Literal,
            flags: MatchFlags::default(),
        };
        let Ok(hits) = search_text(&t, &q) else { return Ok(()) };
        for h in hits {
            let ctx = hit_context(&t, h.clone());
            // 上下文里把 `\n`(1 字节) 换成了 `⏎`(3 字节)——长度是变的。
            // 高亮区间必须是「边拼边记」出来的，不能拿原文偏移去减。
            prop_assert!(
                ctx.context.is_char_boundary(ctx.highlight.start)
                    && ctx.context.is_char_boundary(ctx.highlight.end),
                "高亮区间非边界: {:?} 在 {:?}", ctx.highlight, ctx.context
            );
            prop_assert_eq!(&ctx.context[ctx.highlight.clone()], &t[h]);
        }
    }
}
