//! 校对的落盘接线：把 mj-text 的纯规则引擎接到磁盘上的角色卡、用户词典、
//! 忽略表。见 doc.md §6.8、§6.7。
//!
//! 分层（§4）：判断「哪里有问题」是 mj-text 的纯函数；「从哪读专名、忽略了什么、
//! 忽略键怎么算」是 IO 与领域逻辑，归这里。blake3 也只有 mj-core 有。
#![allow(clippy::string_slice)] // issue.range 来自校对器，落在字符边界

use std::collections::HashSet;
use std::path::Path;

use mj_text::proof::{
    CancelToken, ConfusionSet, Issue, ProofContext, ProofOptions, Proofreader, RuleProofreader,
    split_paragraphs,
};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::id::BookId;
use crate::model::Character;
use crate::store::Store;
use crate::workspace::Workspace;

/// 已忽略的校对问题（§6.8）。
///
/// key = `blake3(rule_id + 命中文本 + 前后各 10 字)` 的十六进制前缀。用**内容**而非
/// 字节位置作键：在别处加删文字使该问题整体位移，键不变，忽略依然生效；而前后各
/// 10 字又能把同一段里两个相同命中区分开（忽略这个「的」不等于忽略所有「的」）。
#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    keys: HashSet<String>,
}

impl IgnoreSet {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    pub fn insert(&mut self, key: String) -> bool {
        self.keys.insert(key)
    }

    /// 从 `dict/ignore.json` 读。文件不存在 = 空表（正常情况）。
    /// 损坏时**不**报错清空——那会把用户攒下的忽略一次抹掉；返回空表并记日志，
    /// 保住原文件等人工看。
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "读忽略表失败，视作空表");
                return Self::default();
            }
        };
        match serde_json::from_str::<Vec<String>>(&text) {
            Ok(keys) => Self {
                keys: keys.into_iter().collect(),
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "忽略表 JSON 损坏，视作空表（原文件保留）");
                Self::default()
            }
        }
    }

    /// 原子写回 `dict/ignore.json`。排序后写，diff 友好、可入 git。
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut keys: Vec<&String> = self.keys.iter().collect();
        keys.sort();
        let json = serde_json::to_string_pretty(&keys).map_err(|e| Error::ChapterParse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        crate::atomic::write(path, json.as_bytes())
    }
}

/// 某条问题在整章文本里的忽略键。`text` 是整章正文，`issue.range` 是整章坐标。
pub fn ignore_key(text: &str, issue: &Issue) -> String {
    const CTX: usize = 10;
    let matched = text.get(issue.range.clone()).unwrap_or("");
    // 前 10 字：range.start 之前的最后 10 个字符。
    let before: String = text[..issue.range.start]
        .chars()
        .rev()
        .take(CTX)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[issue.range.end..].chars().take(CTX).collect();

    // 用 NUL 分隔各部分，避免「rule_id 尾 + 命中头」这类拼接歧义。
    let material = format!("{}\0{before}\0{matched}\0{after}", issue.rule_id);
    let h = blake3::hash(material.as_bytes());
    let mut s = String::with_capacity(32);
    for b in &h.as_bytes()[..16] {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 一章的校对器：跑本地规则，滤掉已忽略项。
pub struct Proofer {
    reader: RuleProofreader,
}

impl Proofer {
    pub fn new(confusion: ConfusionSet, opts: ProofOptions) -> Self {
        Self {
            reader: RuleProofreader::new(confusion, opts),
        }
    }

    /// 按 workspace 里的配置与用户混淆集建。
    ///
    /// 内置混淆集 + `dict/confusion.tsv` 的用户增补；规则开关来自 `config.proof`。
    pub fn from_workspace(ws: &Workspace, config: &Config) -> Self {
        let mut confusion = ConfusionSet::builtin();
        let path = ws.confusion_file();
        match std::fs::read_to_string(&path) {
            Ok(tsv) => confusion.extend_from(&tsv),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "读用户混淆集失败，仅用内置集")
            }
        }
        Self::new(confusion, config.proof.to_options())
    }

    /// 校对整章正文。已忽略的问题被滤掉（§6.8）。
    pub fn check_chapter(
        &self,
        text: &str,
        ctx: &ProofContext,
        ignore: &IgnoreSet,
        cancel: &CancelToken,
    ) -> mj_text::proof::Result<Vec<Issue>> {
        let paras = split_paragraphs(text);
        let issues = self.reader.check(&paras, ctx, cancel)?;
        Ok(issues
            .into_iter()
            .filter(|i| !ignore.contains(&ignore_key(text, i)))
            .collect())
    }
}

/// 从一本书的角色卡 + 用户词典 `dict/user.txt` 建校对上下文（§6.7 [MUST]：
/// 角色名/别名注入词典，是校对不误报的前提）。
///
/// user.txt 用 jieba 用户词典格式（`词 [词频] [词性]`，空格分隔）；这里只取每行首个
/// 词作专名。名字长的排前面——长名先匹配，避免「沈砚」被「沈」抢先（一致性检查用）。
pub fn build_context(store: &Store, ws: &Workspace, book: BookId) -> Result<ProofContext> {
    let mut names: Vec<String> = Vec::new();
    for c in store.list_characters(book)? {
        names.extend(character_names(&c));
    }
    // 用户词典。
    let path = ws.user_dict_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(word) = line.split_whitespace().next() {
                    names.push(word.to_string());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), error = %e, "读用户词典失败"),
    }

    dedup_keep_longest_first(&mut names);
    Ok(ProofContext::new(names))
}

fn character_names(c: &Character) -> Vec<String> {
    c.all_names().map(|s| s.to_string()).collect()
}

/// 去重并按长度降序（长名优先），供一致性检查与专名放行用。
fn dedup_keep_longest_first(names: &mut Vec<String>) {
    let mut seen = HashSet::new();
    names.retain(|n| !n.trim().is_empty() && seen.insert(n.clone()));
    names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_text::proof::{Category, Severity, Source};

    fn issue(rule: &str, range: std::ops::Range<usize>) -> Issue {
        Issue {
            range,
            severity: Severity::Warning,
            category: Category::Typo,
            rule_id: rule.into(),
            message: String::new(),
            suggestions: Vec::new(),
            source: Source::Rule,
            confidence: 0.9,
        }
    }

    #[test]
    fn ignore_key_is_stable_under_repositioning() {
        // 命中前后各 ≥10 字的邻域不变，只是整体被推到后面：键应一致。
        // （前缀 >10 字，故 b 里新加的「序章」段落够不到那 10 字窗口。）
        let a = "他一路奔波风尘仆仆终于赶到城门时气得如火如茶转身就走。";
        let b = format!("序章交代一些背景。\n\n{a}");
        let ra = a.find("如火如茶").unwrap();
        let rb = b.find("如火如茶").unwrap();
        let ka = ignore_key(a, &issue("typo.confusion", ra..ra + "如火如茶".len()));
        let kb = ignore_key(&b, &issue("typo.confusion", rb..rb + "如火如茶".len()));
        assert_eq!(ka, kb, "邻域不变时，位移不该改变忽略键");
    }

    #[test]
    fn ignore_key_distinguishes_same_text_different_context() {
        // 两个「的」，前后文不同，键应不同（忽略一个不等于忽略所有）。
        let text = "红的花，蓝的天。";
        let p1 = text.find("红的").unwrap() + "红".len();
        let p2 = text.find("蓝的").unwrap() + "蓝".len();
        let k1 = ignore_key(text, &issue("typo.de_di_de", p1..p1 + "的".len()));
        let k2 = ignore_key(text, &issue("typo.de_di_de", p2..p2 + "的".len()));
        assert_ne!(k1, k2, "前后文不同应产生不同键");
    }

    #[test]
    fn ignore_set_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ignore.json");
        let mut set = IgnoreSet::default();
        set.insert("abc123".into());
        set.insert("def456".into());
        set.save(&path).unwrap();

        let loaded = IgnoreSet::load(&path);
        assert!(loaded.contains("abc123"));
        assert!(loaded.contains("def456"));
        assert!(!loaded.contains("nope"));
    }

    #[test]
    fn missing_ignore_file_is_empty_not_error() {
        let set = IgnoreSet::load(Path::new("/nonexistent/ignore.json"));
        assert!(set.is_empty());
    }

    #[test]
    fn corrupt_ignore_file_is_empty_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ignore.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let set = IgnoreSet::load(&path);
        assert!(set.is_empty(), "坏文件视作空表，不 panic");
    }

    #[test]
    fn dedup_orders_longest_first() {
        let mut names = vec![
            "沈".to_string(),
            "沈砚".to_string(),
            "沈砚".to_string(),
            "小砚".to_string(),
        ];
        dedup_keep_longest_first(&mut names);
        assert_eq!(names, vec!["沈砚", "小砚", "沈"]);
    }
}
