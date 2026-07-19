//! 导出与导入。见 doc.md §12.2、§11 M6。
//!
//! `mj export <book> --format txt|md -o <out>`。epub 属 M7（要 zip + 模板 + 目录，
//! 是另一件事），这里只做纯文本两种。
//!
//! # 为什么导出的 Markdown 要能被自己读回来
//!
//! 导出格式定成「卷用 `##`、章用 `###`」不是随手挑的：`import` 就按这套规则解析。
//! 于是「导出 → 改 → 导入」构成闭环，用户可以把稿子拿去别的编辑器改完再收回来。
//! 有一条往返测试盯着这件事，格式一改就会红。
//!
//! 渲染是纯函数（吃已载入的书与正文，吐字符串），读盘/写盘各在外面套一层。

use crate::error::{Error, Result};
use crate::id::BookId;
use crate::model::Book;
use crate::store::Store;

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Txt,
    Md,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "txt" | "text" => Some(Self::Txt),
            "md" | "markdown" => Some(Self::Md),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Md => "md",
        }
    }
}

/// 一章的最小信息，供渲染。
pub struct Chapter {
    pub volume: String,
    pub title: String,
    pub body: String,
}

/// 纯渲染：吃书名/作者与按顺序排好的章节，吐出整篇文本。
pub fn render(title: &str, author: &str, chapters: &[Chapter], fmt: Format) -> String {
    let mut out = String::new();
    match fmt {
        Format::Md => {
            out.push_str(&format!("# {title}\n\n"));
            if !author.is_empty() {
                out.push_str(&format!("> 作者：{author}\n\n"));
            }
        }
        Format::Txt => {
            out.push_str(&format!("{title}\n"));
            if !author.is_empty() {
                out.push_str(&format!("作者：{author}\n"));
            }
            out.push('\n');
        }
    }

    let mut current_volume: Option<&str> = None;
    for ch in chapters {
        if current_volume != Some(ch.volume.as_str()) {
            match fmt {
                Format::Md => out.push_str(&format!("## {}\n\n", ch.volume)),
                Format::Txt => out.push_str(&format!("{}\n\n", ch.volume)),
            }
            current_volume = Some(&ch.volume);
        }
        match fmt {
            Format::Md => out.push_str(&format!("### {}\n\n", ch.title)),
            Format::Txt => out.push_str(&format!("{}\n\n", ch.title)),
        }
        // 正文原样落地：段首的全角空格是作者排的，不该在导出时被动过。
        out.push_str(ch.body.trim_end_matches('\n'));
        out.push_str("\n\n");
    }
    // 章与章之间空一行，但文件末尾只留一个换行——文本文件的通例，
    // 也让「导出的文件」与「原样的文件」能逐字节相等。
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// 读盘 + 渲染。章节按阅读顺序（卷序 → 章序），受损章跳过。
pub fn export(store: &Store, book: BookId, fmt: Format) -> Result<String> {
    let b = store.load_book(book)?;
    let mut chapters = Vec::new();
    for vol in &b.volumes {
        for ch in &vol.chapters {
            if ch.damaged.is_some() {
                tracing::warn!(chapter = %ch.id, "导出：跳过受损章");
                continue;
            }
            match store.load_body(book, ch.id) {
                Ok(body) => chapters.push(Chapter {
                    volume: vol.title.clone(),
                    title: ch.title.clone(),
                    body: body.text.to_string(),
                }),
                Err(e) => {
                    tracing::warn!(chapter = %ch.id, error = %e, "导出：跳过读不出的章");
                }
            }
        }
    }
    Ok(render(&b.title, &b.author, &chapters, fmt))
}

/// 导出到文件。
pub fn export_to_file(
    store: &Store,
    book: BookId,
    fmt: Format,
    path: &std::path::Path,
) -> Result<()> {
    let text = export(store, book, fmt)?;
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    }
    crate::atomic::write(path, text.as_bytes())
}

/// 从 Markdown 解析出的书稿骨架。
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub author: String,
    /// `(卷名, 章名, 正文)`，按出现顺序。
    pub chapters: Vec<(String, String, String)>,
}

/// 解析 `export` 产出的 Markdown（`#` 书名 / `##` 卷 / `###` 章）。
///
/// 宽容一些：没有卷标题时归到「第一卷」，没有书名时留空由调用方兜底——
/// 用户很可能是从别的编辑器拿一份大致合规的 md 过来，不该动辄拒收。
pub fn parse_markdown(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut volume = String::new();
    let mut chapter: Option<String> = None;
    let mut body = String::new();

    let flush = |vol: &str, ch: &mut Option<String>, body: &mut String, out: &mut Parsed| {
        if let Some(title) = ch.take() {
            let vol = if vol.is_empty() { "第一卷" } else { vol };
            out.chapters
                .push((vol.to_string(), title, trim_blank_lines(body)));
        }
        body.clear();
    };

    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("### ") {
            flush(&volume, &mut chapter, &mut body, &mut out);
            chapter = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("## ") {
            flush(&volume, &mut chapter, &mut body, &mut out);
            volume = rest.trim().to_string();
        } else if let Some(rest) = t.strip_prefix("# ") {
            flush(&volume, &mut chapter, &mut body, &mut out);
            out.title = rest.trim().to_string();
        } else if let Some(rest) = t.strip_prefix("> 作者：") {
            out.author = rest.trim().to_string();
        } else {
            // 正文原样保留（含段首全角空格），故用 line 而非 trim 后的 t。
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&volume, &mut chapter, &mut body, &mut out);
    out
}

/// 去掉首尾的**空行**，但保留正文行自身的前导空白。
///
/// 不能用 `trim()`：全角空格 U+3000 也算 Unicode 空白，`trim()` 会把段首缩进
/// 一并吃掉——那是作者排的版，被静默删掉就是改稿（§0）。往返测试逮到过这个。
fn trim_blank_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let Some(start) = start else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

/// 把一份 Markdown 导入成新书。
pub fn import_markdown(store: &mut Store, text: &str, fallback_title: &str) -> Result<BookId> {
    let parsed = parse_markdown(text);
    let title = if parsed.title.is_empty() {
        fallback_title
    } else {
        &parsed.title
    };
    let author = if parsed.author.is_empty() {
        "佚名"
    } else {
        &parsed.author
    };

    let book = store.create_book(title, author)?;
    let mut last_volume: Option<(String, crate::id::VolumeId)> = None;
    let mut last_chapter = None;

    for (vol_title, ch_title, body) in &parsed.chapters {
        let vol_id = match &last_volume {
            Some((name, id)) if name == vol_title => *id,
            _ => {
                // `after` 传 None 是**插到最前**，逐卷建下来会把顺序整个倒过来。
                // 必须挂在上一卷之后。（这个是跑真实导入导出往返时发现的：
                // 纯函数的往返测试碰不到 Store，看不出来。）
                let after = last_volume.as_ref().map(|(_, id)| *id);
                let id = store.create_volume(book.id, vol_title, after)?;
                last_volume = Some((vol_title.clone(), id));
                last_chapter = None;
                id
            }
        };
        let ch_id = store.create_chapter(book.id, vol_id, ch_title, last_chapter)?;
        last_chapter = Some(ch_id);
        store.save_body(book.id, &crate::model::ChapterBody::new(ch_id, body))?;
    }
    Ok(book.id)
}

/// 找书：先按 id，再按标题（用户记得住的是书名，不是 8 位 base32）。
pub fn resolve_book(store: &Store, needle: &str) -> Result<Book> {
    if let Ok(id) = needle.parse::<BookId>()
        && let Ok(b) = store.load_book(id)
    {
        return Ok(b);
    }
    let books = store.list_books()?;
    books
        .into_iter()
        .find(|b| b.title == needle)
        .ok_or_else(|| Error::ChapterParse {
            path: std::path::PathBuf::from(needle),
            message: format!("找不到书「{needle}」（可用 id 或书名）"),
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn chapters() -> Vec<Chapter> {
        vec![
            Chapter {
                volume: "第一卷".into(),
                title: "第一章 雪夜".into(),
                body: "　　雪落了一夜。\n".into(),
            },
            Chapter {
                volume: "第一卷".into(),
                title: "第二章 门外".into(),
                body: "　　他推开门。\n".into(),
            },
            Chapter {
                volume: "第二卷".into(),
                title: "第三章 远行".into(),
                body: "　　路很长。\n".into(),
            },
        ]
    }

    #[test]
    fn markdown_uses_heading_levels() {
        let s = render("雪夜行", "沈砚", &chapters(), Format::Md);
        assert!(s.contains("# 雪夜行"), "{s}");
        assert!(s.contains("## 第一卷"), "{s}");
        assert!(s.contains("### 第一章 雪夜"), "{s}");
        assert!(s.contains("> 作者：沈砚"), "{s}");
    }

    /// 卷标题只在换卷时出现一次，不是每章都重复一遍。
    #[test]
    fn volume_heading_appears_once_per_volume() {
        let s = render("书", "作者", &chapters(), Format::Md);
        assert_eq!(s.matches("## 第一卷").count(), 1, "{s}");
        assert_eq!(s.matches("## 第二卷").count(), 1, "{s}");
    }

    #[test]
    fn txt_has_no_markdown_marks() {
        let s = render("雪夜行", "沈砚", &chapters(), Format::Txt);
        assert!(!s.contains('#'), "纯文本不该有井号：{s}");
        assert!(s.contains("第一章 雪夜"), "{s}");
    }

    /// 段首的全角空格是作者排的版，导出时不许动。
    #[test]
    fn body_indentation_is_preserved() {
        let s = render("书", "", &chapters(), Format::Md);
        assert!(
            s.contains("　　雪落了一夜。"),
            "段首全角空格应原样保留：{s}"
        );
    }

    #[test]
    fn empty_author_is_omitted() {
        let s = render("书", "", &chapters(), Format::Md);
        assert!(!s.contains("作者"), "{s}");
    }

    // ---- 解析与往返 ----

    #[test]
    fn parses_what_we_render() {
        let s = render("雪夜行", "沈砚", &chapters(), Format::Md);
        let p = parse_markdown(&s);
        assert_eq!(p.title, "雪夜行");
        assert_eq!(p.author, "沈砚");
        assert_eq!(p.chapters.len(), 3);
        assert_eq!(p.chapters[0].0, "第一卷");
        assert_eq!(p.chapters[0].1, "第一章 雪夜");
        assert_eq!(p.chapters[2].0, "第二卷");
    }

    /// 导出 → 导入 → 再导出，应当一字不差。
    ///
    /// 这条盯着「格式与解析器不许各自跑偏」：改了导出格式却忘了改解析器，
    /// 用户把稿子拿出去改完就再也收不回来了。
    #[test]
    fn markdown_roundtrips() {
        let first = render("雪夜行", "沈砚", &chapters(), Format::Md);
        let parsed = parse_markdown(&first);
        let again: Vec<Chapter> = parsed
            .chapters
            .iter()
            .map(|(v, t, b)| Chapter {
                volume: v.clone(),
                title: t.clone(),
                body: b.clone(),
            })
            .collect();
        let second = render(&parsed.title, &parsed.author, &again, Format::Md);
        assert_eq!(first, second, "导出→导入→导出 应当稳定");
    }

    /// 没有卷标题的 md 也收：归到「第一卷」，别动辄拒收外来稿子。
    #[test]
    fn accepts_markdown_without_volumes() {
        let p = parse_markdown("# 书\n\n### 第一章\n\n正文。\n");
        assert_eq!(p.chapters.len(), 1);
        assert_eq!(p.chapters[0].0, "第一卷");
    }

    #[test]
    fn format_parse_accepts_aliases() {
        assert_eq!(Format::parse("md"), Some(Format::Md));
        assert_eq!(Format::parse("Markdown"), Some(Format::Md));
        assert_eq!(Format::parse("TXT"), Some(Format::Txt));
        assert_eq!(Format::parse("epub"), None, "epub 属 M7，这里不该假装支持");
    }
}
