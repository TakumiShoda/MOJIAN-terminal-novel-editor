//! 章节文件的解析与序列化。见 doc.md §5.2。
//!
//! 格式：`+++` 包裹的 TOML front matter + 纯文本正文。
//! （文档写的是 YAML，改用 TOML 的理由见 ADR 0004。）
//!
//! 三条 `[MUST]`（§5.2）：
//! - 无 front matter → 视为纯正文，首次保存时补写；
//! - 字段缺失 → 用默认值；
//! - 字段多余 → **原样回写，不得丢弃**。
//!
//! 最后一条是本模块的重心。用户可能手动往 front matter 里加东西，
//! 新版本可能加字段——老版本读了再存，绝不能把它们吃掉。
//!
//! 正文部分绝不含任何私有标记：用户把 .md 拿去别处必须能直接用。

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::id::ChapterId;
use crate::model::ChapterStatus;

/// front matter 的分隔线。
const FENCE: &str = "+++";

/// 已解析的章节文件。
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterFile {
    pub meta: FrontMatter,
    /// 正文。已归一化为 LF（doc.md §9）。
    pub body: String,
}

/// front matter。未知字段进 `extra`，回写时原样带回。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontMatter {
    pub id: ChapterId,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: ChapterStatus,
    /// 创建/修改时间。存 RFC3339 字符串（如 `2026-07-16T10:00:00+09:00`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// 字数缓存。§5.2 明言「以实际正文为准，不一致时重算」——
    /// 故读取时不信任它，只作为免于全量重算的提示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// 未知字段透传（§5.2 `[MUST]`）。必须放在最后：
    /// TOML 的表（table）一旦开始就会吞掉后续所有键，flatten 的散键必须先于它们序列化。
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl FrontMatter {
    pub fn new(id: ChapterId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            status: ChapterStatus::default(),
            created: None,
            updated: None,
            words: None,
            tags: Vec::new(),
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("front matter 未闭合（缺少结尾的 `{FENCE}`）")]
    UnterminatedFrontMatter,
    #[error("front matter 不是合法 TOML")]
    BadToml(#[from] toml::de::Error),
}

impl ChapterFile {
    /// 解析章节文件。
    ///
    /// `fallback_id` 用于无 front matter 的情况——那时文件是用户手动丢进来的
    /// 纯文本，需要分配一个新 id（§6.1 验收：手动丢进 books/ 的目录能被识别）。
    pub fn parse(raw: &str, fallback_id: ChapterId) -> Result<Self, ParseError> {
        // 读入即归一化（doc.md §9，ADR 0003）。
        let text = mj_text::eol::normalize(raw);

        let Some(rest) = strip_opening_fence(&text) else {
            // 无 front matter：整个文件都是正文。首次保存时会补写。
            return Ok(Self {
                meta: FrontMatter::new(fallback_id, ""),
                body: text,
            });
        };

        // 找结尾的 fence（必须独占一行）。
        let (fm_text, body) =
            split_at_closing_fence(rest).ok_or(ParseError::UnterminatedFrontMatter)?;

        let meta: FrontMatter = toml::from_str(fm_text)?;
        Ok(Self {
            meta,
            body: body.to_owned(),
        })
    }

    /// 序列化为文件内容（行尾仍是 LF；写盘前由 Store 按配置转换）。
    pub fn to_text(&self) -> Result<String, toml::ser::Error> {
        let fm = toml::to_string(&self.meta)?;
        // fm 已以换行结尾；正文与 fence 之间不额外插入空行——
        // 那会在每次读写往返时多出一行。
        Ok(format!("{FENCE}\n{fm}{FENCE}\n{}", self.body))
    }
}

/// 剥掉开头的 fence 行，返回其后的内容。非 fence 开头则返回 None。
fn strip_opening_fence(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(FENCE)?;
    // fence 后必须紧跟换行（`+++x` 不算）。
    match rest.strip_prefix('\n') {
        Some(r) => Some(r),
        // 文件只有一行 `+++`：视为未闭合，交给上层报错。
        None if rest.is_empty() => Some(rest),
        None => None,
    }
}

/// 在结尾 fence 处切开，返回 (front matter, 正文)。
///
/// 用 `split_at` 而非 `&rest[..n]`：前者在非字符边界会 panic 得明明白白，
/// 而这里的偏移由 `split_inclusive` 的整行长度累加而来，必落在边界上。
/// 更重要的是不必让读者自己去验证这一点（§0 禁令 5 的精神）。
fn split_at_closing_fence(rest: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n') == FENCE {
            let (fm, after) = rest.split_at(offset);
            // 跳过 fence 行自身。
            let body = after.get(line.len()..)?;
            return Some((fm, body));
        }
        offset += line.len();
    }
    None
}

impl FromStr for ChapterStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft" => Ok(Self::Draft),
            "revised" => Ok(Self::Revised),
            "done" => Ok(Self::Done),
            other => Err(format!("未知状态 `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn id() -> ChapterId {
        "ch_7Q2M4KZA".parse().unwrap()
    }

    #[test]
    fn parses_full_front_matter() {
        let raw = "\
+++
id = \"ch_7Q2M4KZA\"
title = \"第一章 雪夜\"
status = \"draft\"
words = 3128
tags = [\"伏笔\"]
+++
　　雪落了一夜。
";
        let f = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        assert_eq!(f.meta.id, id());
        assert_eq!(f.meta.title, "第一章 雪夜");
        assert_eq!(f.meta.status, ChapterStatus::Draft);
        assert_eq!(f.meta.words, Some(3128));
        assert_eq!(f.meta.tags, vec!["伏笔"]);
        assert_eq!(f.body, "　　雪落了一夜。\n");
    }

    /// §5.2 [MUST]：无 front matter 视为纯正文。
    #[test]
    fn tolerates_missing_front_matter() {
        let fallback = ChapterId::generate();
        let f = ChapterFile::parse("　　雪落了一夜。\n没有任何头部。", fallback).unwrap();
        assert_eq!(f.meta.id, fallback, "应分配 fallback id");
        assert_eq!(f.body, "　　雪落了一夜。\n没有任何头部。", "全文都是正文");
    }

    /// §5.2 [MUST]：字段缺失用默认值。
    #[test]
    fn tolerates_missing_fields() {
        let raw = "+++\nid = \"ch_7Q2M4KZA\"\n+++\n正文\n";
        let f = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        assert_eq!(f.meta.title, "");
        assert_eq!(f.meta.status, ChapterStatus::Draft, "status 默认 draft");
        assert_eq!(f.meta.words, None);
        assert!(f.meta.tags.is_empty());
    }

    /// §5.2 [MUST]：字段多余必须原样回写，不得丢弃。这是本模块最重要的一条。
    #[test]
    fn preserves_unknown_fields_on_roundtrip() {
        let raw = "\
+++
id = \"ch_7Q2M4KZA\"
title = \"第一章\"
mood = \"阴郁\"
custom_number = 42
+++
正文
";
        let f = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        assert_eq!(
            f.meta.extra.get("mood").and_then(|v| v.as_str()),
            Some("阴郁")
        );

        let out = f.to_text().unwrap();
        assert!(out.contains("mood"), "未知字段被吃掉:\n{out}");
        assert!(out.contains("阴郁"), "未知字段的值被吃掉:\n{out}");
        assert!(out.contains("custom_number"), "未知字段被吃掉:\n{out}");
    }

    /// 读 → 写 → 读，必须完全一致。这是「不损坏用户文件」的核心保证。
    #[test]
    fn roundtrip_is_stable() {
        let raw = "\
+++
id = \"ch_7Q2M4KZA\"
title = \"第一章 雪夜\"
status = \"revised\"
tags = [\"伏笔\", \"重要\"]
mood = \"阴郁\"
+++
　　雪落了一夜。

　　他推开门。
";
        let first = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        let text = first.to_text().unwrap();
        let second = ChapterFile::parse(&text, ChapterId::generate()).unwrap();
        assert_eq!(first, second, "往返后元数据或正文发生漂移");
        assert_eq!(second.to_text().unwrap(), text, "二次序列化不稳定");
    }

    /// CRLF 文件读入后正文里不得残留 \r（ADR 0003）。
    #[test]
    fn normalizes_crlf_on_parse() {
        let raw = "+++\r\nid = \"ch_7Q2M4KZA\"\r\n+++\r\n　　雪落了一夜。\r\n";
        let f = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        assert!(!f.body.contains('\r'), "正文残留 CR: {:?}", f.body);
        assert_eq!(f.body, "　　雪落了一夜。\n");
    }

    #[test]
    fn rejects_unterminated_front_matter() {
        let raw = "+++\nid = \"ch_7Q2M4KZA\"\n没有结尾栅栏\n";
        assert!(matches!(
            ChapterFile::parse(raw, ChapterId::generate()),
            Err(ParseError::UnterminatedFrontMatter)
        ));
    }

    #[test]
    fn rejects_malformed_toml() {
        let raw = "+++\nthis is not toml {{{\n+++\n正文\n";
        assert!(matches!(
            ChapterFile::parse(raw, ChapterId::generate()),
            Err(ParseError::BadToml(_))
        ));
    }

    /// 正文里出现 `+++` 不应被误认为 front matter 结尾——
    /// 只有开头那段才是 front matter。
    #[test]
    fn plus_fence_inside_body_is_untouched() {
        let raw = "+++\nid = \"ch_7Q2M4KZA\"\n+++\n正文第一行\n+++\n正文第三行\n";
        let f = ChapterFile::parse(raw, ChapterId::generate()).unwrap();
        assert_eq!(f.body, "正文第一行\n+++\n正文第三行\n");
    }

    /// 不以 fence 开头的文件，即使内部含 `+++` 也是纯正文。
    #[test]
    fn body_starting_with_text_is_not_front_matter() {
        let f = ChapterFile::parse("正文\n+++\n更多正文\n", id()).unwrap();
        assert_eq!(f.body, "正文\n+++\n更多正文\n");
    }

    #[test]
    fn empty_body_roundtrips() {
        let f = ChapterFile {
            meta: FrontMatter::new(id(), "空章"),
            body: String::new(),
        };
        let text = f.to_text().unwrap();
        let back = ChapterFile::parse(&text, ChapterId::generate()).unwrap();
        assert_eq!(back.body, "");
        assert_eq!(back.meta.title, "空章");
    }

    #[test]
    fn body_is_free_of_private_markers() {
        // §5.2：正文绝不含私有标记。序列化后 fence 之后的内容必须逐字等于 body。
        let f = ChapterFile {
            meta: FrontMatter::new(id(), "t"),
            body: "　　雪落了一夜。\n".into(),
        };
        let text = f.to_text().unwrap();
        let body_part = text
            .split_once("+++\n")
            .unwrap()
            .1
            .split_once("+++\n")
            .unwrap()
            .1;
        assert_eq!(body_part, "　　雪落了一夜。\n");
    }
}
