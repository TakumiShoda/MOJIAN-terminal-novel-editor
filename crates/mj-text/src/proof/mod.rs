//! 校对契约与后端。见 doc.md §6.8、§2.2。
//!
//! 分三个后端，但**只有 `RuleProofreader` 属 M5**（本地、默认开、同步、快）；
//! `ExternalProofreader` / `LlmProofreader` 是 M7，这里只留骨架。
//!
//! # 为什么契约放在 mj-text
//!
//! `Issue` 是纯数据，`RuleProofreader` 是纯函数：喂进段落 + 上下文，吐出问题列表，
//! 不碰磁盘也不碰全局状态（§4）。这样错报/漏报能被 proptest 大规模验证，
//! 也能保证「校对绝不阻塞输入」——它就是一次普通的函数调用，随时可弃。
//!
//! 内置混淆集、句式表这类**数据**用 `include_str!` 编进二进制（编译期常量，
//! 不算运行时 IO）；用户在 `dict/*.tsv` 里的增补由 mj-core 读盘后并进来。

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub mod confusion;
pub mod consistency;
pub mod external;
pub mod llm;
pub mod punct;
pub mod rules;
pub mod style;

pub use confusion::ConfusionSet;
pub use rules::{ProofOptions, RuleProofreader};

/// 严重度。UI 按此分组、着色（Error 红 / Warning 黄 / Hint 暗）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "错误",
            Self::Warning => "警告",
            Self::Hint => "提示",
        }
    }
}

/// 问题类别（§6.8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Typo,
    Grammar,
    Punct,
    Style,
    Consistency,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Typo => "错别字",
            Self::Grammar => "病句",
            Self::Punct => "标点",
            Self::Style => "文风",
            Self::Consistency => "一致性",
        }
    }
}

/// 问题来源。`[MUST]` UI 必须区分展示：本地规则 vs 模型建议是两回事，
/// 用户对它们的信任度不同（§2.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// 本地规则引擎。
    Rule,
    /// 外部命令。
    External,
    /// 大模型。
    Llm,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rule => "规则",
            Self::External => "外部",
            Self::Llm => "模型",
        }
    }
}

/// 一条校对问题。`range` 是**相对整章正文**的字节区间（§6.8）。
///
/// 各后端只看得到自己那批段落，产出的是段内偏移；拼装成整章偏移由调用方
/// （`RuleProofreader::check`）加上段落基址完成——见 `rules.rs`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub range: Range<usize>,
    pub severity: Severity,
    pub category: Category,
    /// 规则标识，如 `typo.de_di_de` / `style.comma_chain` / `punct.unpaired`。
    pub rule_id: String,
    pub message: String,
    /// 可一键应用的替换建议（可能为空——如「缺句末标点」只提示不自动补）。
    pub suggestions: Vec<String>,
    pub source: Source,
    /// 0..1。低于阈值默认折叠（§6.8：的/地/得 默认 <0.6 折叠）。
    pub confidence: f32,
}

impl Issue {
    /// 命中的原文（供忽略键、UI 展示）。调用方保证 `range` 落在 `text` 内。
    pub fn matched<'a>(&self, text: &'a str) -> &'a str {
        text.get(self.range.clone()).unwrap_or("")
    }
}

/// 校对上下文：角色名等专名。
///
/// 这些名字有两重作用（§2.2、§6.7）：
/// 1. **压误报**——专名不在通用词典里，分词会切碎，混淆集/一致性检查会把
///    「沈砚」错报成问题。已知专名一律放行。
/// 2. **喂一致性检查**——正文里与已知名字编辑距离为 1 的 token 才是可疑的
///    （「沈研」vs「沈砚」）。
#[derive(Debug, Clone, Default)]
pub struct ProofContext {
    /// 角色名 + 别名 + 用户词典里的专名。
    pub names: Vec<String>,
}

impl ProofContext {
    pub fn new(names: Vec<String>) -> Self {
        Self { names }
    }
}

/// 中断令牌。校对跑在工作线程，用户接着打字/切章时要能立刻停（§6.8 [MUST] 可中断）。
///
/// 是共享句柄不是全局状态：`clone` 出来给工作线程，主线程 `cancel()`。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProofError {
    /// 被 cancel token 打断。调用方通常直接丢弃本次结果。
    #[error("校对已取消")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, ProofError>;

/// 校对后端统一接口。
pub trait Proofreader: Send {
    fn id(&self) -> &'static str;

    /// 按段落切分后调用，便于缓存与增量。`paragraphs[i]` 的问题偏移**相对整章**。
    /// `[MUST]` 必须可中断：跑之前、以及每段之间检查 `cancel`。
    fn check(
        &self,
        paragraphs: &[Paragraph<'_>],
        ctx: &ProofContext,
        cancel: &CancelToken,
    ) -> Result<Vec<Issue>>;
}

/// 一个段落及其在整章正文里的起始字节偏移。
///
/// 校对是按段做的（便于缓存），但 `Issue.range` 要的是整章坐标，
/// 故每段都得带着自己的基址走。
#[derive(Debug, Clone, Copy)]
pub struct Paragraph<'a> {
    pub text: &'a str,
    /// 本段首字节在整章正文中的偏移。
    pub offset: usize,
}

impl<'a> Paragraph<'a> {
    pub fn new(text: &'a str, offset: usize) -> Self {
        Self { text, offset }
    }
}

/// 是否 CJK 表意文字。标点/假名/谚文不算——校对规则用它判定「这是中文正文」。
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'      // CJK 统一表意
        | '\u{3400}'..='\u{4DBF}'    // 扩展 A
        | '\u{F900}'..='\u{FAFF}'    // 兼容表意
        | '\u{20000}'..='\u{2A6DF}') // 扩展 B
}

/// 把整章正文切成带偏移的段落。
///
/// 段以**空行**分隔（与排版、统计口径一致）。保留每段的原始起点，
/// 这样段内命中能还原成整章坐标。空段（连续空行）跳过，但偏移照常推进。
pub fn split_paragraphs(text: &str) -> Vec<Paragraph<'_>> {
    let mut out = Vec::new();
    let mut offset = 0;
    for part in text.split_inclusive('\n') {
        let trimmed = part.trim_end_matches('\n');
        // 只对非空段建 Paragraph；空行只推进偏移。
        if !trimmed.trim().is_empty() {
            out.push(Paragraph::new(trimmed, offset));
        }
        offset += part.len();
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    #![allow(clippy::string_slice)] // 断言里用段落偏移切原文，正是要验证它落在边界上

    use super::*;

    #[test]
    fn paragraphs_carry_absolute_offsets() {
        let text = "第一段。\n\n第二段。\n";
        let ps = split_paragraphs(text);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].text, "第一段。");
        assert_eq!(ps[0].offset, 0);
        assert_eq!(
            &text[ps[1].offset..ps[1].offset + ps[1].text.len()],
            "第二段。"
        );
    }

    #[test]
    fn blank_lines_are_skipped_but_offsets_stay_true() {
        let text = "甲\n\n\n乙\n";
        let ps = split_paragraphs(text);
        assert_eq!(ps.len(), 2);
        for p in &ps {
            assert_eq!(
                &text[p.offset..p.offset + p.text.len()],
                p.text,
                "偏移必须对得上原文"
            );
        }
    }

    #[test]
    fn cancel_token_flips() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        let t2 = t.clone();
        t2.cancel();
        assert!(t.is_cancelled(), "clone 出去的句柄取消，原句柄也该看到");
    }

    #[test]
    fn severity_orders_error_first() {
        let mut v = [Severity::Hint, Severity::Error, Severity::Warning];
        v.sort();
        assert_eq!(v, [Severity::Error, Severity::Warning, Severity::Hint]);
    }
}
