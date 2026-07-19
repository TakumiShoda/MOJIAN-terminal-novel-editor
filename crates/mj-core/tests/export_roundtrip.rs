//! 导入导出经 Store 的往返。见 doc.md §12.2、§11 M6。
//!
//! 纯函数的往返测试（render → parse → render）碰不到 Store，
//! 而卷/章的**顺序**恰恰是 Store 那一层决定的——第一版就是在这里翻了车：
//! `create_volume(after: None)` 是「插到最前」，逐卷建下来把顺序整个倒了过来，
//! 而纯函数测试全绿。故这份测试从磁盘往返。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::export::{self, Format};
use mj_core::store::Store;
use mj_core::workspace::Workspace;

fn store(dir: &tempfile::TempDir) -> Store {
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    Store::new(ws, Config::default())
}

const SRC: &str = "\
# 雪夜行

> 作者：沈砚

## 第一卷

### 第一章 雪夜

　　雪落了一夜。他推开门，风裹着雪灌进来。

　　院里那株梅树开了。

### 第二章 门外

　　他站了很久。

## 第二卷

### 第三章 远行

　　路很长。
";

/// 导入 → 导出，应当一字不差地回到原样。
#[test]
fn markdown_roundtrips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);

    let id = export::import_markdown(&mut s, SRC, "备用书名").unwrap();
    let out = export::export(&s, id, Format::Md).unwrap();

    assert_eq!(out, SRC, "经磁盘往返后应与原文一致");
}

/// 卷与章的顺序必须保住——这正是纯函数测试盖不到、又最容易错的地方。
#[test]
fn volume_and_chapter_order_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();

    let b = s.load_book(id).unwrap();
    let vols: Vec<&str> = b.volumes.iter().map(|v| v.title.as_str()).collect();
    assert_eq!(vols, vec!["第一卷", "第二卷"], "卷序不能倒");

    let chapters: Vec<&str> = b
        .volumes
        .iter()
        .flat_map(|v| &v.chapters)
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        chapters,
        vec!["第一章 雪夜", "第二章 门外", "第三章 远行"],
        "章序不能倒"
    );
}

/// 段首的全角空格是作者排的版，往返后必须还在。
#[test]
fn paragraph_indentation_survives() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();
    let out = export::export(&s, id, Format::Md).unwrap();
    assert!(
        out.contains("　　雪落了一夜。"),
        "段首全角空格被吃了：{out}"
    );
    assert!(out.contains("　　院里那株梅树开了。"), "{out}");
}

/// 章内的空行（分段）也要保住。
#[test]
fn blank_lines_inside_a_chapter_survive() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();
    let b = s.load_book(id).unwrap();
    let first = b.volumes[0].chapters[0].id;
    let body = s.load_body(id, first).unwrap().text.to_string();
    assert!(body.contains("\n\n"), "章内分段的空行应保留：{body:?}");
}

#[test]
fn txt_export_has_no_markdown_marks() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();
    let out = export::export(&s, id, Format::Txt).unwrap();
    assert!(!out.contains('#'), "{out}");
    assert!(out.contains("第一章 雪夜"), "{out}");
    assert!(out.contains("　　路很长。"), "{out}");
}

/// 没有 `# 书名` 时用兜底名，别让书叫空字符串。
#[test]
fn falls_back_to_given_title() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, "### 只有一章\n\n正文。\n", "无名稿").unwrap();
    let b = s.load_book(id).unwrap();
    assert_eq!(b.title, "无名稿");
    assert_eq!(b.volumes.len(), 1, "没有卷标题时归到一卷");
}

/// 按书名找书（用户记得住的是书名，不是 8 位 base32）。
#[test]
fn resolve_book_by_title_or_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();

    let by_title = export::resolve_book(&s, "雪夜行").unwrap();
    assert_eq!(by_title.id, id);

    let by_id = export::resolve_book(&s, &id.to_string()).unwrap();
    assert_eq!(by_id.id, id);

    assert!(export::resolve_book(&s, "查无此书").is_err());
}

#[test]
fn export_to_file_writes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = store(&dir);
    let id = export::import_markdown(&mut s, SRC, "x").unwrap();

    let out = dir.path().join("sub/dir/book.md");
    export::export_to_file(&s, id, Format::Md, &out).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert_eq!(text, SRC, "落到文件里的内容应与导出一致");
}
