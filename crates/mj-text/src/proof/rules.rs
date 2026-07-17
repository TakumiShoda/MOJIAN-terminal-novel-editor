//! 本地规则引擎：把混淆集 / 标点 / 文风 / 一致性 / 的地得 组合起来。见 doc.md §6.8。
//!
//! 每段独立跑，段内偏移最后统一加上段落基址还原成整章坐标。`[MUST]` 可中断：
//! 段与段之间查 cancel token。
#![allow(clippy::string_slice)] // 名字区间来自 find，构造即落在字符边界

use super::confusion::ConfusionSet;
use super::style::StyleParams;
use super::{
    CancelToken, Category, Issue, Paragraph, ProofContext, ProofError, Proofreader, Result,
    Severity, Source, consistency, punct, style,
};

/// 规则引擎开关与阈值。默认对应 §6.8：混淆集/标点/文风(前两条)/一致性开，
/// 的地得开但折叠（低置信）。
#[derive(Debug, Clone)]
pub struct ProofOptions {
    pub confusion_on: bool,
    pub punct_on: bool,
    pub consistency_on: bool,
    /// 的/地/得。默认开，但产出的都是低置信 Hint，UI 默认折叠（§12.3 [MUST]）。
    pub de_di_de_on: bool,
    pub style: StyleParams,
}

impl Default for ProofOptions {
    fn default() -> Self {
        Self {
            confusion_on: true,
            punct_on: true,
            consistency_on: true,
            de_di_de_on: true,
            style: StyleParams::default(),
        }
    }
}

/// 本地规则校对器。
pub struct RuleProofreader {
    confusion: ConfusionSet,
    opts: ProofOptions,
}

impl RuleProofreader {
    pub fn new(confusion: ConfusionSet, opts: ProofOptions) -> Self {
        Self { confusion, opts }
    }

    /// 内置混淆集 + 默认选项。
    pub fn builtin() -> Self {
        Self::new(ConfusionSet::builtin(), ProofOptions::default())
    }

    pub fn options(&self) -> &ProofOptions {
        &self.opts
    }

    /// 单段的全部问题（段内偏移）。
    fn check_paragraph(&self, para: &str, ctx: &ProofContext) -> Vec<Issue> {
        let mut issues = Vec::new();
        if self.opts.confusion_on {
            issues.extend(self.confusion.scan(para));
        }
        if self.opts.punct_on {
            issues.extend(punct::check(para));
        }
        issues.extend(style::check(para, &self.opts.style));
        if self.opts.consistency_on {
            issues.extend(consistency::check(para, ctx));
        }
        if self.opts.de_di_de_on {
            issues.extend(de_di_de(para));
        }
        // 专名放行：落在已知角色名内部的错别字/一致性问题一律撤掉（§2.2、§6.7）。
        suppress_on_names(&mut issues, para, &ctx.names);
        issues
    }
}

impl Proofreader for RuleProofreader {
    fn id(&self) -> &'static str {
        "rule"
    }

    fn check(
        &self,
        paragraphs: &[Paragraph<'_>],
        ctx: &ProofContext,
        cancel: &CancelToken,
    ) -> Result<Vec<Issue>> {
        let mut all = Vec::new();
        for p in paragraphs {
            if cancel.is_cancelled() {
                return Err(ProofError::Cancelled);
            }
            for mut issue in self.check_paragraph(p.text, ctx) {
                // 段内偏移 → 整章偏移。
                issue.range = (issue.range.start + p.offset)..(issue.range.end + p.offset);
                all.push(issue);
            }
        }
        // 稳定排序：UI 按位置列出，proptest 也好断言。
        all.sort_by(|a, b| {
            a.range
                .start
                .cmp(&b.range.start)
                .then(a.range.end.cmp(&b.range.end))
        });
        Ok(all)
    }
}

/// 撤掉落在已知角色名范围内的 Typo / Consistency 问题。
fn suppress_on_names(issues: &mut Vec<Issue>, para: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    // 已知名在本段的所有字节区间。
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for name in names {
        if name.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(rel) = para[from..].find(name.as_str()) {
            let s = from + rel;
            spans.push((s, s + name.len()));
            from = s + name.len();
        }
    }
    if spans.is_empty() {
        return;
    }
    issues.retain(|i| {
        if !matches!(i.category, Category::Typo | Category::Consistency) {
            return true;
        }
        // 命中完全落在某个名字里 → 撤掉。
        !spans
            .iter()
            .any(|&(s, e)| i.range.start >= s && i.range.end <= e)
    });
}

/// 的/地/得：**刻意做窄**（§12.3 [MUST]：这条最易惹恼用户，宁保守勿激进）。
///
/// 只认一个高精度型：形容词/动词 + 「的」或「地」+ 程度副词（很/太/极/挺），
/// 这里正字是「得」（「跑得很快」）。排除「的很多/的很少」这类正确的领属结构。
/// 全部低置信 Hint（<0.6），UI 默认折叠。
fn de_di_de(para: &str) -> Vec<Issue> {
    const DEGREE: &[char] = &['很', '太', '极', '挺'];
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    let mut out = Vec::new();
    for i in 0..chars.len() {
        let (bpos, c) = chars[i];
        if c != '的' && c != '地' {
            continue;
        }
        // 前一个字须是 CJK（像个谓词），否则不像「V/A + 的/地 + 程度」。
        let Some(&(_, prev)) = i.checked_sub(1).map(|k| &chars[k]) else {
            continue;
        };
        if !super::is_cjk(prev) {
            continue;
        }
        let Some(&(_, next)) = chars.get(i + 1) else {
            continue;
        };
        if !DEGREE.contains(&next) {
            continue;
        }
        // 排除领属：「的很多 / 的很少」。
        if next == '很'
            && let Some(&(_, nn)) = chars.get(i + 2)
            && (nn == '多' || nn == '少')
        {
            continue;
        }
        out.push(Issue {
            range: bpos..bpos + c.len_utf8(),
            severity: Severity::Hint,
            category: Category::Typo,
            rule_id: "typo.de_di_de".into(),
            message: format!("程度补语前一般用「得」，此处的「{c}」是否应为「得」？"),
            suggestions: vec!["得".into()],
            source: Source::Rule,
            confidence: 0.5,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::split_paragraphs;
    use super::*;

    fn run(text: &str, ctx: &ProofContext) -> Vec<Issue> {
        let paras = split_paragraphs(text);
        RuleProofreader::builtin()
            .check(&paras, ctx, &CancelToken::new())
            .unwrap()
    }

    fn ids(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.rule_id.as_str()).collect()
    }

    #[test]
    fn issues_carry_whole_chapter_offsets() {
        let text = "第一段没问题。\n\n他气得如火如茶。";
        let issues = run(text, &ProofContext::default());
        let confusion = issues
            .iter()
            .find(|i| i.rule_id == "typo.confusion")
            .unwrap();
        // 命中必须能用整章坐标切回原文。
        assert_eq!(text.get(confusion.range.clone()), Some("如火如茶"));
    }

    #[test]
    fn combines_multiple_categories() {
        let text = "他说：「如火如茶,他跑的很快";
        let issues = run(text, &ProofContext::default());
        let got = ids(&issues);
        assert!(got.contains(&"typo.confusion"), "{got:?}");
        assert!(got.contains(&"punct.halfwidth"), "{got:?}");
        assert!(got.contains(&"punct.unpaired"), "{got:?}");
    }

    #[test]
    fn de_di_de_fires_on_complement() {
        let issues = run("他跑的很快。", &ProofContext::default());
        let de = issues
            .iter()
            .find(|i| i.rule_id == "typo.de_di_de")
            .unwrap();
        assert_eq!(de.suggestions, vec!["得".to_string()]);
        assert!(de.confidence < 0.6, "的地得默认要折叠");
    }

    #[test]
    fn de_di_de_excludes_possessive() {
        let issues = run("他的很多朋友都来了。", &ProofContext::default());
        assert!(
            !ids(&issues).contains(&"typo.de_di_de"),
            "的很多 是领属，不该报"
        );
    }

    #[test]
    fn known_name_suppresses_typo_inside_it() {
        // 假设有个角色叫「如火如茶」（离谱但用于测试专名放行）。
        let ctx = ProofContext::new(vec!["如火如茶".to_string()]);
        let issues = run("主角如火如茶登场了。", &ctx);
        assert!(
            !ids(&issues).contains(&"typo.confusion"),
            "落在角色名里的错别字应放行"
        );
    }

    #[test]
    fn cancel_before_work_returns_cancelled() {
        let paras = split_paragraphs("如火如茶。");
        let cancel = CancelToken::new();
        cancel.cancel();
        let r = RuleProofreader::builtin().check(&paras, &ProofContext::default(), &cancel);
        assert_eq!(r, Err(ProofError::Cancelled));
    }

    #[test]
    fn clean_text_yields_nothing() {
        let text = "他推开门，风雪扑面而来，冷得他打了个寒战。";
        let issues = run(text, &ProofContext::default());
        assert!(issues.is_empty(), "干净文本不该报：{issues:?}");
    }

    #[test]
    fn name_suspect_reaches_through() {
        let ctx = ProofContext::new(vec!["沈砚".to_string()]);
        let issues = run("那天沈研来了。", &ctx);
        assert!(ids(&issues).contains(&"name.suspect"), "{:?}", ids(&issues));
    }
}
