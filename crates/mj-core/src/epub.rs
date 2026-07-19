//! epub 导出。见 doc.md §12.2、§11 M7。
//!
//! epub 就是个规定了目录结构的 zip：
//!
//! ```text
//! mimetype                     ← 必须是第一个条目，且不压缩
//! META-INF/container.xml       ← 指向 content.opf
//! OEBPS/content.opf            ← 元数据 + 清单 + 阅读顺序
//! OEBPS/nav.xhtml              ← EPUB 3 的目录
//! OEBPS/toc.ncx                ← EPUB 2 的目录，给老阅读器
//! OEBPS/style.css
//! OEBPS/text/ch0001.xhtml …
//! ```
//!
//! # 三处不照做就出事的地方
//!
//! 1. **`mimetype` 必须是 zip 的第一个条目，且用 Stored（不压缩）**，内容正好是
//!    `application/epub+zip`、不带换行。OCF 规范这么定，是为了让阅读器不解压就能
//!    从固定字节偏移认出这是 epub。压了或挪了位置，一部分阅读器直接说文件损坏。
//!    有一条测试盯着字节偏移。
//!
//! 2. **所有用户文本都要转义**。书名里一个 `&`、正文里一个 `<`，就能让整本 XHTML
//!    不合法而打不开。作者写的是小说不是 XML，这种字符迟早会出现。
//!
//! 3. **段首的全角空格原样留着，不要加 `text-indent`**。U+3000 不属于 HTML 的
//!    可折叠空白（那只包括 ASCII 的空格/制表/换行），所以它在浏览器和阅读器里
//!    会照样占两格。再叠一层 CSS 缩进就成了缩四格。§12.2 的原则是「段首的全角
//!    空格是作者排的，不该在导出时被动过」，这里同样适用。

use std::io::{Cursor, Write as _};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::{Error, Result};
use crate::export::Chapter;

/// 一本书导出成 epub 的字节。
///
/// `identifier` 是 `dc:identifier`，要在一本书的生命周期里稳定且唯一（用 BookId）；
/// `modified` 是 `dcterms:modified`，EPUB 3 必填，格式 `YYYY-MM-DDTHH:MM:SSZ`。
/// 两个都从参数进而不在内部现取，是为了让同样的输入产出同样的字节——
/// 内部调 `Utc::now()` 的话，导出两次得到两个不同的文件，没法比对也没法测。
pub fn build(
    title: &str,
    author: &str,
    identifier: &str,
    modified: &str,
    chapters: &[Chapter],
) -> Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::<u8>::new()));
    // 时间戳固定在 zip 的纪元（1980-01-01），否则同样的书导出两次字节不同。
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let io = |e: std::io::Error| Error::Io {
        path: std::path::PathBuf::from("<epub>"),
        source: e,
    };
    let zip_err = |e: zip::result::ZipError| Error::ChapterParse {
        path: std::path::PathBuf::from("<epub>"),
        message: format!("打包 epub 失败：{e}"),
    };

    // 1. mimetype——必须第一个、必须 Stored。见模块注释。
    zip.start_file("mimetype", stored).map_err(zip_err)?;
    zip.write_all(b"application/epub+zip").map_err(io)?;

    let mut put = |name: &str, body: &str| -> Result<()> {
        zip.start_file(name, deflated).map_err(zip_err)?;
        zip.write_all(body.as_bytes()).map_err(io)
    };

    put("META-INF/container.xml", CONTAINER_XML)?;
    put("OEBPS/style.css", STYLE_CSS)?;
    put(
        "OEBPS/content.opf",
        &content_opf(title, author, identifier, modified, chapters),
    )?;
    put("OEBPS/nav.xhtml", &nav_xhtml(title, chapters))?;
    put("OEBPS/toc.ncx", &toc_ncx(title, identifier, chapters))?;
    for (i, ch) in chapters.iter().enumerate() {
        put(&chapter_path(i), &chapter_xhtml(&ch.title, &ch.body))?;
    }

    Ok(zip.finish().map_err(zip_err)?.into_inner())
}

fn chapter_id(i: usize) -> String {
    format!("ch{:04}", i + 1)
}

fn chapter_path(i: usize) -> String {
    format!("OEBPS/text/{}.xhtml", chapter_id(i))
}

/// XML 文本转义。
///
/// 五个都要转：`&` 和 `<` 是硬伤；`>` 在 `]]>` 里会出事；引号是因为同一个函数
/// 也用在属性值上（`dc:title` 之类），分成两个函数迟早会用错那一个。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

/// 排版只做最克制的一点：行高、两端留白。
///
/// **不设 `text-indent`**：段首的全角空格已经在正文里了，再缩一层就是四格。
const STYLE_CSS: &str = r#"html { font-size: 100%; }
body { margin: 0 5%; line-height: 1.7; }
h1, h2 { line-height: 1.4; }
p { margin: 0 0 0.6em 0; text-indent: 0; }
"#;

fn content_opf(
    title: &str,
    author: &str,
    identifier: &str,
    modified: &str,
    chapters: &[Chapter],
) -> String {
    let mut manifest = String::new();
    let mut spine = String::new();
    for (i, _) in chapters.iter().enumerate() {
        let id = chapter_id(i);
        manifest.push_str(&format!(
            "    <item id=\"{id}\" href=\"text/{id}.xhtml\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("    <itemref idref=\"{id}\"/>\n"));
    }
    let creator = if author.is_empty() {
        String::new()
    } else {
        format!("    <dc:creator>{}</dc:creator>\n", esc(author))
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id" xml:lang="zh">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">{id}</dc:identifier>
    <dc:title>{title}</dc:title>
    <dc:language>zh</dc:language>
{creator}    <meta property="dcterms:modified">{modified}</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
{manifest}  </manifest>
  <spine toc="ncx">
{spine}  </spine>
</package>
"#,
        id = esc(identifier),
        title = esc(title),
        modified = esc(modified),
    )
}

/// EPUB 3 的目录。按卷分层：卷名一层，章挂在下面。
fn nav_xhtml(title: &str, chapters: &[Chapter]) -> String {
    let mut body = String::new();
    let mut current: Option<&str> = None;
    let mut open_sub = false;
    for (i, ch) in chapters.iter().enumerate() {
        if current != Some(ch.volume.as_str()) {
            if open_sub {
                body.push_str("      </ol>\n    </li>\n");
            }
            // 卷本身没有对应页面，但 nav 的 <li> 必须有内容——用 <span>。
            body.push_str(&format!(
                "    <li><span>{}</span>\n      <ol>\n",
                esc(&ch.volume)
            ));
            current = Some(&ch.volume);
            open_sub = true;
        }
        body.push_str(&format!(
            "        <li><a href=\"text/{}.xhtml\">{}</a></li>\n",
            chapter_id(i),
            esc(&ch.title)
        ));
    }
    if open_sub {
        body.push_str("      </ol>\n    </li>\n");
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="zh" lang="zh">
<head><meta charset="utf-8"/><title>{title}</title></head>
<body>
  <nav epub:type="toc" id="toc">
    <h1>目录</h1>
    <ol>
{body}    </ol>
  </nav>
</body>
</html>
"#,
        title = esc(title),
    )
}

/// EPUB 2 的目录。EPUB 3 不要求它，但不少国产阅读器只认这个。
fn toc_ncx(title: &str, identifier: &str, chapters: &[Chapter]) -> String {
    let mut points = String::new();
    for (i, ch) in chapters.iter().enumerate() {
        points.push_str(&format!(
            "    <navPoint id=\"nav{n}\" playOrder=\"{n}\">\n      \
             <navLabel><text>{label}</text></navLabel>\n      \
             <content src=\"text/{id}.xhtml\"/>\n    </navPoint>\n",
            n = i + 1,
            label = esc(&ch.title),
            id = chapter_id(i),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1" xml:lang="zh">
  <head><meta name="dtb:uid" content="{id}"/></head>
  <docTitle><text>{title}</text></docTitle>
  <navMap>
{points}  </navMap>
</ncx>
"#,
        id = esc(identifier),
        title = esc(title),
    )
}

/// 一章的 XHTML。正文按行成段，空行丢掉（空行在 epub 里没有意义，段距由 CSS 管）。
fn chapter_xhtml(title: &str, body: &str) -> String {
    let mut paras = String::new();
    for line in body.lines() {
        // 只去行尾的 \r（CRLF 正文）与 ASCII 空白；**不碰行首的全角空格**——
        // 那是作者排的缩进，U+3000 在 HTML 里不折叠，原样留着正好显示成两格。
        let line = line.trim_end_matches(['\r', ' ', '\t']);
        if line.is_empty() {
            continue;
        }
        paras.push_str(&format!("  <p>{}</p>\n", esc(line)));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="zh" lang="zh">
<head>
  <meta charset="utf-8"/>
  <title>{t}</title>
  <link rel="stylesheet" type="text/css" href="../style.css"/>
</head>
<body>
  <h2>{t}</h2>
{paras}</body>
</html>
"#,
        t = esc(title),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn ch(volume: &str, title: &str, body: &str) -> Chapter {
        Chapter {
            volume: volume.into(),
            title: title.into(),
            body: body.into(),
        }
    }

    fn sample() -> Vec<u8> {
        build(
            "雪夜行",
            "沈砚",
            "urn:mojian:book:0123",
            "2026-07-19T00:00:00Z",
            &[
                ch("第一卷", "第一章 风雪", "　　他推开门。\n　　风雪扑面。\n"),
                ch("第一卷", "第二章 城门", "　　城门已闭。\n"),
                ch("第二卷", "第三章 归途", "　　天亮了。\n"),
            ],
        )
        .unwrap()
    }

    /// 把 zip 里某个条目读出来。
    fn entry(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut f = z.by_name(name).unwrap_or_else(|_| panic!("没有 {name}"));
        let mut s = String::new();
        std::io::Read::read_to_string(&mut f, &mut s).unwrap();
        s
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        let z = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        z.file_names().map(|s| s.to_string()).collect()
    }

    /// **最要紧的一条**：mimetype 必须是第一个条目、不压缩、内容正好那 20 个字节。
    ///
    /// 直接按字节偏移验，而不是用 zip 库读——用库读等于「我写进去了我读得出来」，
    /// 而阅读器是照着固定偏移去认的。本地文件头 30 字节 + 文件名 8 字节，
    /// 之后就该是 mimetype 的内容；中间要是插了 extra field 或压缩了，这里就对不上。
    #[test]
    fn mimetype_is_first_stored_and_byte_exact() {
        let b = sample();
        assert_eq!(&b[0..4], b"PK\x03\x04", "开头该是本地文件头");
        assert_eq!(&b[8..10], &[0, 0], "压缩方法必须是 0（Stored）");
        assert_eq!(&b[26..28], &[8, 0], "文件名长度该是 8");
        assert_eq!(&b[28..30], &[0, 0], "mimetype 不能带 extra field");
        assert_eq!(&b[30..38], b"mimetype");
        assert_eq!(
            &b[38..58],
            b"application/epub+zip",
            "内容要正好是这 20 字节，不带换行"
        );
    }

    /// 结构齐全，且 container.xml 指得到 opf。
    #[test]
    fn has_the_required_structure() {
        let b = sample();
        let n = names(&b);
        for want in [
            "mimetype",
            "META-INF/container.xml",
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/toc.ncx",
            "OEBPS/text/ch0001.xhtml",
            "OEBPS/text/ch0003.xhtml",
        ] {
            assert!(n.iter().any(|x| x == want), "缺 {want}：{n:?}");
        }
        assert_eq!(n[0], "mimetype", "mimetype 必须排第一");
        assert!(entry(&b, "META-INF/container.xml").contains("OEBPS/content.opf"));
    }

    /// opf 里每一章都要既在 manifest 又在 spine——少一边就是打不开或读不到。
    #[test]
    fn every_chapter_is_in_manifest_and_spine() {
        let opf = entry(&sample(), "OEBPS/content.opf");
        for id in ["ch0001", "ch0002", "ch0003"] {
            assert!(
                opf.contains(&format!("<item id=\"{id}\"")),
                "{id} 不在 manifest：{opf}"
            );
            assert!(
                opf.contains(&format!("<itemref idref=\"{id}\"/>")),
                "{id} 不在 spine：{opf}"
            );
        }
        assert!(opf.contains("<dc:title>雪夜行</dc:title>"));
        assert!(opf.contains("<dc:creator>沈砚</dc:creator>"));
        assert!(opf.contains("dcterms:modified"), "EPUB 3 必填");
    }

    /// 段首的全角空格要原样留着（U+3000 在 HTML 里不折叠），
    /// 且**不能**再叠一层 text-indent——那就成缩四格了。
    #[test]
    fn preserves_full_width_indent_without_double_indenting() {
        let b = sample();
        let x = entry(&b, "OEBPS/text/ch0001.xhtml");
        assert!(x.contains("<p>　　他推开门。</p>"), "全角空格被吃了：{x}");
        let css = entry(&b, "OEBPS/style.css");
        assert!(
            css.contains("text-indent: 0"),
            "正文已自带全角缩进，CSS 不能再缩：{css}"
        );
    }

    /// 空行不成段——段距交给 CSS，空的 <p> 在阅读器里是一块莫名的留白。
    #[test]
    fn blank_lines_do_not_become_empty_paragraphs() {
        let b = build(
            "书",
            "",
            "id",
            "2026-07-19T00:00:00Z",
            &[ch("卷", "章", "第一段。\n\n\n第二段。\n")],
        )
        .unwrap();
        let x = entry(&b, "OEBPS/text/ch0001.xhtml");
        assert_eq!(x.matches("<p>").count(), 2, "该只有两段：{x}");
        assert!(!x.contains("<p></p>"));
    }

    /// 作者写的是小说不是 XML。书名里一个 `&` 就能让整本书打不开。
    #[test]
    fn escapes_xml_metacharacters_everywhere() {
        let b = build(
            "A & B <奇怪的书名>",
            "\"引号\"作者",
            "id&1",
            "2026-07-19T00:00:00Z",
            &[ch("卷 <一>", "第一章 & 序", "他说：<别去>，我不去。\n")],
        )
        .unwrap();
        for name in [
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/toc.ncx",
            "OEBPS/text/ch0001.xhtml",
        ] {
            let x = entry(&b, name);
            // 转义后不该再有裸露的 < > &：只允许出现在标签与实体里。
            let stripped = x
                .replace("&amp;", "")
                .replace("&lt;", "")
                .replace("&gt;", "")
                .replace("&quot;", "")
                .replace("&apos;", "");
            assert!(
                !stripped.contains(" & "),
                "{name} 里有没转义的 &：{stripped}"
            );
            assert!(
                !stripped.contains("<别去>"),
                "{name} 里有没转义的尖括号：{stripped}"
            );
        }
        let x = entry(&b, "OEBPS/text/ch0001.xhtml");
        assert!(x.contains("&lt;别去&gt;"), "正文该被转义：{x}");
    }

    /// 目录按卷分层，卷名与章名都在。
    #[test]
    fn nav_groups_chapters_by_volume() {
        let nav = entry(&sample(), "OEBPS/nav.xhtml");
        assert!(nav.contains("<span>第一卷</span>"));
        assert!(nav.contains("<span>第二卷</span>"));
        assert!(nav.contains("text/ch0003.xhtml\">第三章 归途</a>"));
        // 两个卷 → 两个顶层 <li>，各带一个子 <ol>。
        assert_eq!(nav.matches("<ol>").count(), 3, "1 个总表 + 2 个卷内表");
        assert_eq!(nav.matches("</ol>").count(), 3, "开合要配平：{nav}");
    }

    /// 同样的输入要产出同样的字节——否则导两次得到两个文件，没法比对。
    #[test]
    fn output_is_deterministic() {
        assert_eq!(sample(), sample());
    }

    /// 一章都没有也不能崩：空书导出仍是个合法的 epub。
    #[test]
    fn empty_book_still_produces_a_valid_container() {
        let b = build("空书", "", "id", "2026-07-19T00:00:00Z", &[]).unwrap();
        assert_eq!(&b[30..38], b"mimetype");
        let n = names(&b);
        assert!(n.iter().any(|x| x == "OEBPS/content.opf"));
        let nav = entry(&b, "OEBPS/nav.xhtml");
        assert!(nav.contains("<ol>") && nav.contains("</ol>"), "{nav}");
    }
}
