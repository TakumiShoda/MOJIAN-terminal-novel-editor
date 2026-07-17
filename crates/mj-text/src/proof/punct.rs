//! 标点规则。见 doc.md §6.8。
//!
//! 四类：
//! - 引号/括号/书名号未配对（栈式平衡检查，方向性符号才查）；
//! - 句末缺标点；
//! - 中英标点混用（半角标点夹在 CJK 之间）；
//! - 省略号个数异常（`…` 应成偶数，即 `……`）。
//!
//! 都在**段内**判定，产出段内偏移，由 `rules.rs` 加基址还原成整章坐标。

use super::{Category, Issue, Severity, Source};

/// 方向性成对符号：左 → 右。ASCII `"` `'` 是对称的，方向分不清，不做平衡检查。
const PAIRS: &[(char, char)] = &[
    ('「', '」'),
    ('『', '』'),
    ('（', '）'),
    ('《', '》'),
    ('【', '】'),
    ('〈', '〉'),
    ('“', '”'),
    ('‘', '’'),
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
];

fn opener(c: char) -> Option<char> {
    PAIRS.iter().find(|(l, _)| *l == c).map(|(_, r)| *r)
}

fn is_closer(c: char) -> bool {
    PAIRS.iter().any(|(_, r)| *r == c)
}

/// 可以合法结尾的字符：句末标点 + 各种右符号（对话常以 `」` 收尾）。
const TERMINALS: &[char] = &[
    '。', '！', '？', '…', '”', '’', '」', '』', '）', '】', '》', '〉', '—',
];

use super::is_cjk;

/// 半角标点及其全角建议。夹在 CJK 之间时提示改全角。
fn halfwidth_suggestion(c: char) -> Option<char> {
    match c {
        ',' => Some('，'),
        '.' => Some('。'),
        '?' => Some('？'),
        '!' => Some('！'),
        ':' => Some('：'),
        ';' => Some('；'),
        _ => None,
    }
}

/// 段内全部标点问题。`para` 是单段正文，偏移相对段首。
pub fn check(para: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    check_pairs(para, &mut out);
    check_mixed_halfwidth(para, &mut out);
    check_ellipsis(para, &mut out);
    check_sentence_end(para, &mut out);
    out
}

/// 栈式平衡：未闭合的左符号、多余的右符号都报。
fn check_pairs(para: &str, out: &mut Vec<Issue>) {
    // 栈里存 (期望的右符号, 左符号的字节位置)。
    let mut stack: Vec<(char, usize)> = Vec::new();
    for (i, c) in para.char_indices() {
        if let Some(close) = opener(c) {
            stack.push((close, i));
        } else if is_closer(c) {
            match stack.last() {
                Some((expected, _)) if *expected == c => {
                    stack.pop();
                }
                _ => {
                    // 没有对应左符号的右符号。
                    out.push(Issue {
                        range: i..i + c.len_utf8(),
                        severity: Severity::Warning,
                        category: Category::Punct,
                        rule_id: "punct.unpaired".into(),
                        message: format!("多余的「{c}」，没有对应的左符号"),
                        suggestions: Vec::new(),
                        source: Source::Rule,
                        confidence: 0.85,
                    });
                }
            }
        }
    }
    // 栈里剩下的都是没闭合的左符号。
    for (close, pos) in stack {
        let open = PAIRS
            .iter()
            .find(|(_, r)| *r == close)
            .map(|(l, _)| *l)
            .unwrap_or(close);
        out.push(Issue {
            range: pos..pos + open.len_utf8(),
            severity: Severity::Warning,
            category: Category::Punct,
            rule_id: "punct.unpaired".into(),
            message: format!("「{open}」没有闭合，缺一个「{close}」"),
            suggestions: Vec::new(),
            source: Source::Rule,
            confidence: 0.85,
        });
    }
}

/// 半角标点夹在 CJK 之间——典型的输入法没切全角。数字里的 `.`（3.14）两侧非 CJK，不误报。
fn check_mixed_halfwidth(para: &str, out: &mut Vec<Issue>) {
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    for (idx, &(i, c)) in chars.iter().enumerate() {
        let Some(full) = halfwidth_suggestion(c) else {
            continue;
        };
        let prev = idx.checked_sub(1).map(|k| chars[k].1);
        let next = chars.get(idx + 1).map(|&(_, c)| c);
        // 前或后紧挨 CJK 才算「中文里的半角标点」。
        let touches_cjk = prev.is_some_and(is_cjk) || next.is_some_and(is_cjk);
        if touches_cjk {
            out.push(Issue {
                range: i..i + c.len_utf8(),
                severity: Severity::Hint,
                category: Category::Punct,
                rule_id: "punct.halfwidth".into(),
                message: format!("中文里宜用全角「{full}」而非半角「{c}」"),
                suggestions: vec![full.to_string()],
                source: Source::Rule,
                confidence: 0.7,
            });
        }
    }
}

/// 省略号：`…`（U+2026）应成对出现（`……`）。连续 `…` 为奇数个则提示。
fn check_ellipsis(para: &str, out: &mut Vec<Issue>) {
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    let mut idx = 0;
    while idx < chars.len() {
        if chars[idx].1 != '…' {
            idx += 1;
            continue;
        }
        let start = chars[idx].0;
        let run_begin = idx;
        while idx < chars.len() && chars[idx].1 == '…' {
            idx += 1;
        }
        let count = idx - run_begin;
        if count % 2 == 1 {
            let end = chars[idx - 1].0 + '…'.len_utf8();
            out.push(Issue {
                range: start..end,
                severity: Severity::Hint,
                category: Category::Punct,
                rule_id: "punct.ellipsis".into(),
                message: "省略号应成偶数个「…」（即「……」）".into(),
                suggestions: Vec::new(),
                source: Source::Rule,
                confidence: 0.6,
            });
        }
    }
}

/// 句末缺标点：段落最后一个非空字符不是任何终止符。
///
/// 用 Hint：作者可能有意留断句、或这段本就是标题式短语。只提醒，不武断。
fn check_sentence_end(para: &str, out: &mut Vec<Issue>) {
    let trimmed = para.trim_end();
    let Some(last) = trimmed.chars().next_back() else {
        return;
    };
    if TERMINALS.contains(&last) || last == '"' || last == '\'' {
        return;
    }
    // 只对「像句子」的段落提醒：含 CJK 且有一定长度，免得对纯符号/编号行乱报。
    let cjk_count = trimmed.chars().filter(|c| is_cjk(*c)).count();
    if cjk_count < 4 {
        return;
    }
    let end = trimmed.len();
    let start = end - last.len_utf8();
    out.push(Issue {
        range: start..end,
        severity: Severity::Hint,
        category: Category::Punct,
        rule_id: "punct.sentence_end".into(),
        message: "段末似乎缺少句号或其他句末标点".into(),
        suggestions: Vec::new(),
        source: Source::Rule,
        confidence: 0.5,
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn ids(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.rule_id.as_str()).collect()
    }

    #[test]
    fn unclosed_quote_is_flagged() {
        let issues = check("他说：「你来了。");
        assert!(ids(&issues).contains(&"punct.unpaired"), "{issues:?}");
        let un = issues
            .iter()
            .find(|i| i.rule_id == "punct.unpaired")
            .unwrap();
        assert_eq!(un.matched("他说：「你来了。"), "「");
    }

    #[test]
    fn balanced_quotes_are_clean() {
        let issues = check("他说：「你来了。」");
        assert!(
            !ids(&issues).contains(&"punct.unpaired"),
            "配对的不该报：{issues:?}"
        );
    }

    #[test]
    fn stray_closer_is_flagged() {
        let issues = check("你来了」。");
        let un = issues
            .iter()
            .find(|i| i.rule_id == "punct.unpaired")
            .unwrap();
        assert!(un.message.contains("多余"), "{}", un.message);
    }

    #[test]
    fn nested_pairs_balance() {
        let issues = check("他说：「我看过《雪夜行》。」");
        assert!(!ids(&issues).contains(&"punct.unpaired"), "{issues:?}");
    }

    #[test]
    fn halfwidth_comma_between_cjk_is_hinted() {
        let issues = check("你好,世界。");
        let h = issues
            .iter()
            .find(|i| i.rule_id == "punct.halfwidth")
            .unwrap();
        assert_eq!(h.suggestions, vec!["，".to_string()]);
    }

    #[test]
    fn decimal_point_is_not_flagged() {
        // 3.14 两侧是数字，不是 CJK，不该报半角。
        let issues = check("圆周率约 3.14 左右。");
        assert!(!ids(&issues).contains(&"punct.halfwidth"), "{issues:?}");
    }

    #[test]
    fn odd_ellipsis_hinted_even_is_clean() {
        assert!(
            ids(&check("我想想…")).contains(&"punct.ellipsis"),
            "单个 … 该提示"
        );
        assert!(
            !ids(&check("我想想……")).contains(&"punct.ellipsis"),
            "…… 不该报"
        );
    }

    #[test]
    fn missing_end_punct_hinted() {
        let issues = check("他推开门走进了风雪里");
        assert!(ids(&issues).contains(&"punct.sentence_end"), "{issues:?}");
    }

    #[test]
    fn dialogue_ending_in_close_quote_is_fine() {
        let issues = check("他说：「我来了。」");
        assert!(
            !ids(&issues).contains(&"punct.sentence_end"),
            "以」结尾不算缺标点"
        );
    }

    #[test]
    fn short_label_line_is_not_nagged() {
        // 太短、不像句子，别报缺标点。
        let issues = check("第一章");
        assert!(!ids(&issues).contains(&"punct.sentence_end"), "{issues:?}");
    }

    #[test]
    fn ranges_stay_on_char_boundaries() {
        let text = "他说：「你来了,世界…";
        for issue in check(text) {
            assert!(
                text.get(issue.range.clone()).is_some(),
                "{}: {:?}",
                issue.rule_id,
                issue.range
            );
        }
    }
}
