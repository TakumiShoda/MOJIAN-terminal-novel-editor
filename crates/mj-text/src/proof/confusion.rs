//! 混淆集：错别字词表命中。见 doc.md §6.8、§12.3。
//!
//! 内置集编译进二进制（`include_str!`，编译期常量，非运行时 IO）；用户在
//! `dict/confusion.tsv` 里的条目由 mj-core 读盘后 `extend_from` 并进来，可覆盖内置项。
//!
//! 匹配是**字面串**查找（错误形是确定的字），可选一条**段落级上下文正则**：
//! 正则非空时，须在该段任意位置匹配上，条目才触发。这样用户能写
//! `帐\t账\t号|户\t帐号/帐户` 这种「仅在特定语境下才算错」的条目，压误报。

#![allow(clippy::string_slice)] // 切片下标均来自 find/char_indices，构造即落在字符边界

use std::collections::HashMap;
use std::ops::Range;

use regex::Regex;

use super::{Category, Issue, Severity, Source};

/// 一条混淆条目。
#[derive(Debug, Clone)]
pub struct ConfusionEntry {
    pub wrong: String,
    pub right: String,
    /// 段落级上下文正则。`None` = 无条件触发。
    pub context: Option<Regex>,
    pub note: String,
}

/// 混淆集。内置 + 用户增补。
#[derive(Debug, Clone, Default)]
pub struct ConfusionSet {
    entries: Vec<ConfusionEntry>,
}

impl ConfusionSet {
    /// 内置起始集（§12.3）。
    pub fn builtin() -> Self {
        let mut set = Self::default();
        // 内置集是可信的；真出了非法正则，编译期改不了但运行不该崩——
        // parse 内部会跳过并记日志。
        set.extend_from(include_str!("confusion_builtin.tsv"));
        set
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 追加/覆盖条目。同一个「错误形」后来者覆盖前者——让用户能改内置项的建议或语境。
    pub fn extend_from(&mut self, tsv: &str) {
        for (lineno, raw) in tsv.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 2 || cols[0].is_empty() {
                tracing::warn!(line = lineno + 1, "混淆集行列数不足，跳过：{line:?}");
                continue;
            }
            let wrong = cols[0].to_string();
            let right = cols[1].to_string();
            let context = match cols.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => None,
                Some(pat) => match Regex::new(pat) {
                    Ok(re) => Some(re),
                    Err(e) => {
                        // 非法正则不该拖垮整张表：跳过这条，其余照常。
                        tracing::warn!(line = lineno + 1, pattern = pat, error = %e, "混淆集上下文正则非法，跳过该条");
                        continue;
                    }
                },
            };
            let note = cols.get(3).map(|s| s.to_string()).unwrap_or_default();
            self.upsert(ConfusionEntry {
                wrong,
                right,
                context,
                note,
            });
        }
    }

    fn upsert(&mut self, entry: ConfusionEntry) {
        if let Some(slot) = self.entries.iter_mut().find(|e| e.wrong == entry.wrong) {
            *slot = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// 扫一个段落，产出段内偏移的问题。上下文正则按整段判定。
    ///
    /// 同一段里多个条目可能命中重叠区间——按「起点升序、长者优先」贪心取非重叠，
    /// 避免同一处报两遍。
    pub fn scan(&self, para: &str) -> Vec<Issue> {
        let mut hits: Vec<(Range<usize>, &ConfusionEntry)> = Vec::new();
        for e in &self.entries {
            if let Some(re) = &e.context
                && !re.is_match(para)
            {
                continue;
            }
            let mut from = 0;
            while let Some(rel) = para[from..].find(&e.wrong) {
                let start = from + rel;
                let end = start + e.wrong.len();
                hits.push((start..end, e));
                from = end;
            }
        }
        // 起点升序；同起点时长者优先（成语整条盖过其中的短片段）。
        hits.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));

        let mut out = Vec::new();
        let mut cursor = 0;
        for (range, e) in hits {
            if range.start < cursor {
                continue; // 与已选区间重叠，跳过
            }
            cursor = range.end;
            let msg = if e.note.is_empty() {
                format!("疑似「{}」误作「{}」", e.right, e.wrong)
            } else {
                format!("疑似别字：{}", e.note)
            };
            out.push(Issue {
                range,
                severity: Severity::Warning,
                category: Category::Typo,
                rule_id: "typo.confusion".into(),
                message: msg,
                suggestions: vec![e.right.clone()],
                source: Source::Rule,
                // 混淆集是高精度条目（错误形基本总是错），给高置信。
                confidence: 0.9,
            });
        }
        out
    }
}

/// 供 mj-core 做「专名一致性」等用途：返回内置集覆盖的错误形集合。
pub fn builtin_wrong_forms() -> HashMap<String, String> {
    ConfusionSet::builtin()
        .entries
        .into_iter()
        .map(|e| (e.wrong, e.right))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn builtin_loads_and_parses() {
        let set = ConfusionSet::builtin();
        assert!(set.len() >= 15, "内置集条目太少：{}", set.len());
    }

    #[test]
    fn catches_idiom_typo() {
        let set = ConfusionSet::builtin();
        let issues = set.scan("现场气氛如火如茶。");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].matched("现场气氛如火如茶。"), "如火如茶");
        assert_eq!(issues[0].suggestions, vec!["如火如荼".to_string()]);
        assert_eq!(issues[0].category, Category::Typo);
    }

    #[test]
    fn correct_idiom_is_not_flagged() {
        let set = ConfusionSet::builtin();
        assert!(set.scan("现场气氛如火如荼。").is_empty(), "正确写法不该报");
    }

    #[test]
    fn context_regex_gates_the_hit() {
        let mut set = ConfusionSet::default();
        // 「帐」只在紧挨「号/户」时才算错。
        set.extend_from("帐\t账\t帐[号户]\t帐号/帐户");
        assert_eq!(set.scan("这是我的帐号").len(), 1, "帐号 该报");
        assert!(set.scan("营帐扎在山下").is_empty(), "营帐 不该报");
    }

    #[test]
    fn user_entry_overrides_builtin() {
        let mut set = ConfusionSet::builtin();
        let before = set.len();
        set.extend_from("如火如茶\t如火如荼\t\t自定义说明");
        assert_eq!(set.len(), before, "同错误形应覆盖而非新增");
        let issues = set.scan("如火如茶");
        assert!(
            issues[0].message.contains("自定义说明"),
            "{}",
            issues[0].message
        );
    }

    #[test]
    fn overlapping_hits_reported_once() {
        let mut set = ConfusionSet::default();
        set.extend_from("走头无路\t走投无路\n头无\t头误\t\t");
        let issues = set.scan("他走头无路。");
        // 「走头无路」与「头无」重叠，只报一处（长者优先）。
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].matched("他走头无路。"), "走头无路");
    }

    #[test]
    fn invalid_user_regex_is_skipped_not_fatal() {
        let mut set = ConfusionSet::default();
        set.extend_from("甲\t乙\t(unclosed\t坏正则\n丙\t丁");
        // 坏正则那条被跳过，好的那条仍在。
        assert_eq!(set.scan("丙").len(), 1);
        assert!(set.scan("甲").is_empty(), "坏正则条目不该生效");
    }

    #[test]
    fn ranges_are_valid_utf8_boundaries() {
        let set = ConfusionSet::builtin();
        let text = "开头迫不急待地走头无路，如火如茶。";
        for issue in set.scan(text) {
            assert!(
                text.get(issue.range.clone()).is_some(),
                "range 必须落在字符边界"
            );
        }
    }
}
