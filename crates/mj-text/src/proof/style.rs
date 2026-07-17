//! 文风规则。见 doc.md §6.8 的表。
//!
//! 默认**开**的只有 `comma_chain` 和 `long_sentence`——它们是客观计数，误报少。
//! `word_repeat` / `short_burst` 实现了但默认**关**（§6.8：默认关的规则要让用户
//! 按自己的文风取用）。`pattern_repeat`（句式重复，需 `dict/patterns.tsv`）和
//! `split_clause`（拆散的单句）判定太主观，留待句式表接入后再做，此处不占坑误报。
//!
//! 全部段内判定，段内偏移。
#![allow(clippy::string_slice)] // 句子区间来自 char_indices，构造即落在字符边界

use super::{Category, Issue, Severity, Source};

/// 文风阈值与开关。默认值即 §6.8 表里的值。
#[derive(Debug, Clone, Copy)]
pub struct StyleParams {
    pub comma_chain_on: bool,
    /// 连续逗号数超过它就报（未见句末标点）。
    pub comma_chain_max: usize,
    pub long_sentence_on: bool,
    /// 单句字数上限。
    pub long_sentence_max: usize,
    pub word_repeat_on: bool,
    /// 实词重复的窗口（字符数）与次数阈值。
    pub word_repeat_window: usize,
    pub word_repeat_min: usize,
    pub short_burst_on: bool,
    /// 连续 N 句都短于 M 字。
    pub short_burst_n: usize,
    pub short_burst_m: usize,
}

impl Default for StyleParams {
    fn default() -> Self {
        Self {
            comma_chain_on: true,
            comma_chain_max: 6,
            long_sentence_on: true,
            long_sentence_max: 60,
            word_repeat_on: false,
            word_repeat_window: 300,
            word_repeat_min: 4,
            short_burst_on: false,
            short_burst_n: 5,
            short_burst_m: 8,
        }
    }
}

/// 句末终止符。
fn is_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '…' | '!' | '?')
}

/// 一个句子在段内的字节区间。
#[derive(Debug, Clone, Copy)]
struct Sentence {
    start: usize,
    end: usize,
}

/// 把段落切成句子（以终止符结尾，末尾无终止符的残句也算一句）。
fn sentences(para: &str) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut last_end = 0;
    for (i, c) in para.char_indices() {
        last_end = i + c.len_utf8();
        if is_terminator(c) {
            out.push(Sentence {
                start,
                end: last_end,
            });
            start = last_end;
        }
    }
    if start < para.len() {
        out.push(Sentence {
            start,
            end: last_end.max(para.len()),
        });
    }
    out
}

/// 计入「句长」的字符：CJK 与字母数字。标点、空白不算。
fn is_counted(c: char) -> bool {
    c.is_alphanumeric()
}

pub fn check(para: &str, p: &StyleParams) -> Vec<Issue> {
    let mut out = Vec::new();
    let sents = sentences(para);

    if p.long_sentence_on {
        for s in &sents {
            let body = &para[s.start..s.end];
            let n = body.chars().filter(|c| is_counted(*c)).count();
            if n > p.long_sentence_max {
                out.push(Issue {
                    range: s.start..s.end,
                    severity: Severity::Hint,
                    category: Category::Style,
                    rule_id: "style.long_sentence".into(),
                    message: format!("单句 {n} 字，偏长（阈值 {}）", p.long_sentence_max),
                    suggestions: Vec::new(),
                    source: Source::Rule,
                    confidence: 0.5,
                });
            }
        }
    }

    if p.comma_chain_on {
        for s in &sents {
            let body = &para[s.start..s.end];
            let commas = body.chars().filter(|c| *c == '，' || *c == ',').count();
            if commas > p.comma_chain_max {
                out.push(Issue {
                    range: s.start..s.end,
                    severity: Severity::Hint,
                    category: Category::Style,
                    rule_id: "style.comma_chain".into(),
                    message: format!(
                        "流水句：一句里 {commas} 个逗号未断句（阈值 {}）",
                        p.comma_chain_max
                    ),
                    suggestions: Vec::new(),
                    source: Source::Rule,
                    confidence: 0.5,
                });
            }
        }
    }

    if p.short_burst_on {
        check_short_burst(para, &sents, p, &mut out);
    }

    if p.word_repeat_on {
        check_word_repeat(para, p, &mut out);
    }

    out
}

/// 连续 N 句都短于 M 字（短句堆砌）。报在这一串短句的整段区间上。
fn check_short_burst(para: &str, sents: &[Sentence], p: &StyleParams, out: &mut Vec<Issue>) {
    let len = |s: &Sentence| {
        para[s.start..s.end]
            .chars()
            .filter(|c| is_counted(*c))
            .count()
    };
    let mut i = 0usize;
    while i < sents.len() {
        if len(&sents[i]) < p.short_burst_m {
            let begin = i;
            while i < sents.len() && len(&sents[i]) < p.short_burst_m {
                i += 1;
            }
            let run = i - begin;
            if run >= p.short_burst_n {
                out.push(Issue {
                    range: sents[begin].start..sents[i - 1].end,
                    severity: Severity::Hint,
                    category: Category::Style,
                    rule_id: "style.short_burst".into(),
                    message: format!("连续 {run} 个短句，节奏偏碎"),
                    suggestions: Vec::new(),
                    source: Source::Rule,
                    confidence: 0.4,
                });
            }
        } else {
            i += 1;
        }
    }
}

/// 同一实词在窗口内高频重复。
///
/// 不引分词：以**相邻 CJK 二字组**作实词的近似（「忽然」「叹了」这类）。够抓住
/// 「忽然……忽然……忽然」式的口头禅堆叠。本规则默认关，作者按自己文风取用，
/// 近似的粗糙由此可接受。
fn check_word_repeat(para: &str, p: &StyleParams, out: &mut Vec<Issue>) {
    // (首字节, 二字组)。仅当相邻两字都是 CJK 且contiguous 才成组（标点断开不算）。
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    let mut bigrams: Vec<(usize, String)> = Vec::new();
    for w in chars.windows(2) {
        let (i, a) = w[0];
        let (_, b) = w[1];
        if super::is_cjk(a) && super::is_cjk(b) {
            bigrams.push((i, format!("{a}{b}")));
        }
    }

    let mut reported: Vec<String> = Vec::new();
    for (idx, (pos, gram)) in bigrams.iter().enumerate() {
        if reported.contains(gram) {
            continue;
        }
        let count = bigrams[idx..]
            .iter()
            .take_while(|(p2, _)| p2.saturating_sub(*pos) <= p.word_repeat_window)
            .filter(|(_, g)| g == gram)
            .count();
        if count >= p.word_repeat_min {
            reported.push(gram.clone());
            out.push(Issue {
                range: *pos..*pos + gram.len(),
                severity: Severity::Hint,
                category: Category::Style,
                rule_id: "style.word_repeat".into(),
                message: format!("「{gram}」在 {} 字内出现 {count} 次", p.word_repeat_window),
                suggestions: Vec::new(),
                source: Source::Rule,
                confidence: 0.4,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn ids(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.rule_id.as_str()).collect()
    }

    #[test]
    fn long_sentence_flagged() {
        let long: String = "很".repeat(70) + "。";
        let issues = check(&long, &StyleParams::default());
        assert!(ids(&issues).contains(&"style.long_sentence"), "{issues:?}");
    }

    #[test]
    fn short_sentence_is_clean() {
        let issues = check("他来了。", &StyleParams::default());
        assert!(!ids(&issues).contains(&"style.long_sentence"));
    }

    #[test]
    fn comma_chain_flagged() {
        let issues = check(
            "他走过来，看了看，笑了笑，摇了摇头，叹了口气，转过身，坐下来，又停下。",
            &StyleParams::default(),
        );
        assert!(ids(&issues).contains(&"style.comma_chain"), "{issues:?}");
    }

    #[test]
    fn few_commas_are_clean() {
        let issues = check("他走过来，笑了笑。", &StyleParams::default());
        assert!(!ids(&issues).contains(&"style.comma_chain"));
    }

    #[test]
    fn word_repeat_off_by_default() {
        let text = "忽然他忽然停下，忽然又走，忽然回头。";
        assert!(!ids(&check(text, &StyleParams::default())).contains(&"style.word_repeat"));
    }

    #[test]
    fn word_repeat_fires_when_enabled() {
        let p = StyleParams {
            word_repeat_on: true,
            word_repeat_min: 3,
            ..StyleParams::default()
        };
        let text = "忽然他忽然停下，忽然又走。";
        let issues = check(text, &p);
        assert!(ids(&issues).contains(&"style.word_repeat"), "{issues:?}");
    }

    #[test]
    fn short_burst_fires_when_enabled() {
        let p = StyleParams {
            short_burst_on: true,
            short_burst_n: 3,
            short_burst_m: 6,
            ..StyleParams::default()
        };
        let text = "他来。她走。风停。雪落。天黑。";
        let issues = check(text, &p);
        assert!(ids(&issues).contains(&"style.short_burst"), "{issues:?}");
    }

    #[test]
    fn ranges_valid() {
        let text = "他走过来，看了看，笑了笑，摇了摇头，叹了口气，转身，又停下。";
        for issue in check(text, &StyleParams::default()) {
            assert!(text.get(issue.range.clone()).is_some());
        }
    }
}
