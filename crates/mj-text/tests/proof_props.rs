//! 校对引擎的属性测试。见 doc.md §10、§6.8。
//!
//! 校对结果会驱动 UI 着色、一键应用建议——`Issue.range` 一旦落在半个汉字上，
//! 「应用建议」就会砍碎正文（§0 静默毁稿）。手写用例覆盖不全，靠 proptest 保底。
//!
//! 证四条不变量：
//! 1. 任意输入不 panic；
//! 2. 每条 `range` 精确落在整章正文的字符边界上，且切出来非空；
//! 3. 确定性：同一输入跑两次结果一致（UI 增量刷新、缓存都指望这个）；
//! 4. cancel 生效：中断令牌置位后必然返回 Cancelled。
#![allow(clippy::string_slice)] // 区间来自校对器；「是否真落在边界上」正是本文件要证的
#![allow(clippy::unwrap_used, clippy::expect_used)] // 测试里 panic 即失败信号

use proptest::prelude::*;

use mj_text::proof::{
    CancelToken, ProofContext, ProofError, Proofreader, RuleProofreader, split_paragraphs,
};

/// 生成含标点、全半角、换行、专名近似、错别字诱饵的随机中文文本。
fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("他"),
            Just("她"),
            Just("走"),
            Just("的"),
            Just("地"),
            Just("得"),
            Just("很"),
            Just("快"),
            Just("沈"),
            Just("砚"),
            Just("研"),
            Just("如火如茶"),
            Just("如火如荼"),
            Just("，"),
            Just("。"),
            Just(","),
            Just("."),
            Just("「"),
            Just("」"),
            Just("……"),
            Just("…"),
            Just("\n"),
            Just("　"),
            Just(" "),
            Just("👍"),
            Just("👨‍👩‍👧"), // ZWJ 家庭 emoji：18 字节一个字素簇
            Just("é"),  // e + 组合重音，多标量字素
            Just("𠀀"), // 扩展 B 区 CJK（4 字节）
            Just("\r\n"),
        ],
        0..40,
    )
    .prop_map(|v| v.concat())
}

fn names() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(
        prop_oneof![Just("沈砚".to_string()), Just("苏妲己".to_string())],
        0..3,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(800))]

    /// 不 panic；range 落在字符边界、非空、在文本内。
    #[test]
    fn ranges_are_valid_and_nonempty(s in text(), ns in names()) {
        let paras = split_paragraphs(&s);
        let ctx = ProofContext::new(ns);
        let issues = RuleProofreader::builtin()
            .check(&paras, &ctx, &CancelToken::new())
            .expect("未取消不该返回错误");
        for issue in &issues {
            prop_assert!(issue.range.start < issue.range.end, "空区间：{:?}", issue.range);
            prop_assert!(issue.range.end <= s.len(), "越界：{:?} / {}", issue.range, s.len());
            prop_assert!(
                s.get(issue.range.clone()).is_some(),
                "range 不在字符边界：{:?} rule={}",
                issue.range, issue.rule_id
            );
        }
    }

    /// 确定性：同输入两次结果一致。
    #[test]
    fn deterministic(s in text(), ns in names()) {
        let paras = split_paragraphs(&s);
        let ctx = ProofContext::new(ns);
        let pr = RuleProofreader::builtin();
        let a = pr.check(&paras, &ctx, &CancelToken::new()).unwrap();
        let b = pr.check(&paras, &ctx, &CancelToken::new()).unwrap();
        prop_assert_eq!(a, b);
    }

    /// 建议非空时，应用它不该越过边界（建议是纯文本，range 可切）。
    #[test]
    fn suggestions_are_applicable(s in text()) {
        let paras = split_paragraphs(&s);
        let issues = RuleProofreader::builtin()
            .check(&paras, &ProofContext::default(), &CancelToken::new())
            .unwrap();
        for issue in &issues {
            for sug in &issue.suggestions {
                // 模拟「一键应用」：把 range 换成建议，结果仍是合法 UTF-8 字符串。
                let mut applied = s.clone();
                applied.replace_range(issue.range.clone(), sug);
                prop_assert!(applied.is_char_boundary(0));
            }
        }
    }
}

/// cancel 置位后必返回 Cancelled（放在 proptest 外，一个用例足矣）。
#[test]
fn cancel_is_honored() {
    let paras = split_paragraphs("如火如茶。他跑的很快。");
    let cancel = CancelToken::new();
    cancel.cancel();
    let r = RuleProofreader::builtin().check(&paras, &ProofContext::default(), &cancel);
    assert_eq!(r, Err(ProofError::Cancelled));
}
