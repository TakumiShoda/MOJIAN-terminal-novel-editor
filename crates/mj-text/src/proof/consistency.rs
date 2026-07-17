//! 专名一致性：正文里与已知角色名「差一个字」的 token（§6.7 [MUST]）。
//!
//! 长篇最实用的检查之一——作者记错角色名的某个字（「沈砚」写成「沈研」）是
//! 高频错误。判定用**等长、替换距离为 1**（而非完整编辑距离）：这是这类错误的
//! 主要形态，且精度远高于插入/删除，误报少。
//!
//! 再叠两道收窄，压误报（§2.2 警告规则引擎在模糊任务上误报会让人直接关掉功能）：
//! - 候选与目标名**首字相同**——典型错误是姓对、名错（「苏妲己」→「苏妲已」）；
//! - 候选本身不是任何已知名/别名（那是正确用法，不是笔误）。

use super::{Category, Issue, ProofContext, Severity, Source};

/// 等长字符串的替换距离；> 1 时提前退出返回 2。
fn subst_distance(a: &[char], b: &[char]) -> usize {
    debug_assert_eq!(a.len(), b.len());
    let mut d = 0;
    for (x, y) in a.iter().zip(b) {
        if x != y {
            d += 1;
            if d > 1 {
                return 2;
            }
        }
    }
    d
}

/// 扫一个段落。`ctx.names` 是已知角色名 + 别名。
pub fn check(para: &str, ctx: &ProofContext) -> Vec<Issue> {
    // 只保留长度 ≥2 的名字；单字名滑窗会命中海量常用字，没法用。
    let names: Vec<Vec<char>> = ctx
        .names
        .iter()
        .map(|n| n.chars().collect::<Vec<_>>())
        .filter(|cs| cs.len() >= 2)
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    // 已知名/别名集合（按字符序列），用于「候选本身是正确名字就放行」。
    let known: Vec<&Vec<char>> = names.iter().collect();

    let chars: Vec<(usize, char)> = para.char_indices().collect();
    let mut out = Vec::new();
    let mut reported: Vec<String> = Vec::new();

    for name in &names {
        let len = name.len();
        if chars.len() < len {
            continue;
        }
        for start in 0..=chars.len() - len {
            let window: Vec<char> = chars[start..start + len].iter().map(|&(_, c)| c).collect();
            // 全 CJK 才作为候选：夹标点/空白的窗口不是一个名字。
            if !window.iter().all(|c| super::is_cjk(*c)) {
                continue;
            }
            // 首字须相同（姓对名错）。
            if window[0] != name[0] {
                continue;
            }
            if subst_distance(&window, name) != 1 {
                continue;
            }
            // 候选本身是已知名/别名 → 正确用法，放行。
            if known.iter().any(|k| ***k == window[..]) {
                continue;
            }
            let cand: String = window.iter().collect();
            if reported.contains(&cand) {
                continue;
            }
            reported.push(cand.clone());

            let name_str: String = name.iter().collect();
            let byte_start = chars[start].0;
            let byte_end = chars[start + len - 1].0 + chars[start + len - 1].1.len_utf8();
            out.push(Issue {
                range: byte_start..byte_end,
                severity: Severity::Warning,
                category: Category::Consistency,
                rule_id: "name.suspect".into(),
                message: format!("「{cand}」与角色名「{name_str}」仅一字之差，是否笔误？"),
                suggestions: vec![name_str],
                source: Source::Rule,
                confidence: 0.55,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn ctx(names: &[&str]) -> ProofContext {
        ProofContext::new(names.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn one_char_off_name_is_flagged() {
        let issues = check("那天沈研走进门。", &ctx(&["沈砚"]));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].matched("那天沈研走进门。"), "沈研");
        assert_eq!(issues[0].suggestions, vec!["沈砚".to_string()]);
    }

    #[test]
    fn correct_name_is_not_flagged() {
        assert!(check("那天沈砚走进门。", &ctx(&["沈砚"])).is_empty());
    }

    #[test]
    fn different_surname_is_not_flagged() {
        // 首字不同：不是「姓对名错」，不报。
        assert!(check("李砚走了。", &ctx(&["沈砚"])).is_empty());
    }

    #[test]
    fn two_chars_off_is_not_flagged() {
        // 首字同但其余两字全不同（差两个字），不该报。
        assert!(check("苏轻语。", &ctx(&["苏妲己"])).is_empty());
    }

    #[test]
    fn alias_is_treated_as_correct() {
        // 「小砚」是别名，正确，不该被当成「沈砚」的错写。
        assert!(check("小砚笑了。", &ctx(&["沈砚", "小砚"])).is_empty());
    }

    #[test]
    fn three_char_name_off_by_one() {
        let issues = check("苏妲已入宫。", &ctx(&["苏妲己"]));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].suggestions, vec!["苏妲己".to_string()]);
    }

    #[test]
    fn same_suspect_reported_once_per_paragraph() {
        let issues = check("沈研走了，沈研又回来，沈研站住。", &ctx(&["沈砚"]));
        assert_eq!(issues.len(), 1, "同一可疑词一段只报一次");
    }

    #[test]
    fn empty_names_no_work() {
        assert!(check("随便什么字。", &ctx(&[])).is_empty());
    }
}
