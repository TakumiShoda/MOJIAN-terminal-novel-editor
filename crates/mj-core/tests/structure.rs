//! 结构管理：重命名 / 删除 / 移动 章·卷·书。见 doc.md §6.1、§6.2。
//!
//! 「重启」= 丢弃 Store 从磁盘重扫，验证磁盘是唯一真相（§1）。
//! 每个改名/移动的用例都盯着 §6.2 line 319 那条 [MUST]：**只动元数据与文件名，
//! 绝不碰正文、绝不换 ID**。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId, VolumeId};
use mj_core::model::ChapterBody;
use mj_core::store::Store;
use mj_core::workspace::Workspace;

struct Fx {
    dir: tempfile::TempDir,
    book: BookId,
    vol: VolumeId,
    ch1: ChapterId,
    ch2: ChapterId,
}

fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch1 = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    let ch2 = store
        .create_chapter(book.id, vol, "第二章", Some(ch1))
        .unwrap();
    store
        .save_body(book.id, &ChapterBody::new(ch1, "　　第一章的正文。\n"))
        .unwrap();
    store
        .save_body(book.id, &ChapterBody::new(ch2, "　　第二章的正文。\n"))
        .unwrap();
    Fx {
        dir,
        book: book.id,
        vol,
        ch1,
        ch2,
    }
}

impl Fx {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }
    fn ws(&self) -> Workspace {
        Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap()
    }
}

// ---- 重命名 ----

/// 章改名：标题变了，但 id、正文、order 都不动（§6.2 [MUST]）。
#[test]
fn rename_chapter_changes_title_only() {
    let f = setup();
    let body_before = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    let order_before = {
        let b = f.store().load_book(f.book).unwrap();
        b.volumes[0]
            .chapters
            .iter()
            .find(|c| c.id == f.ch1)
            .unwrap()
            .order
    };

    f.store().rename_chapter(f.book, f.ch1, "楔子").unwrap();

    // 重启，从磁盘看。
    let b = f.store().load_book(f.book).unwrap();
    let ch = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch1)
        .unwrap();
    assert_eq!(ch.title, "楔子", "标题应已改");
    assert_eq!(ch.id, f.ch1, "id 绝不能变");
    assert_eq!(ch.order, order_before, "order 不该动");
    let body_after = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    assert_eq!(body_after, body_before, "正文一字不能动");
    // 另一章不受牵连。
    let ch2 = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch2)
        .unwrap();
    assert_eq!(ch2.title, "第二章");
}

/// 卷改名：卷目录跟着搬，里面的章 id / 正文 / 顺序都不变。
#[test]
fn rename_volume_keeps_its_chapters() {
    let f = setup();
    f.store().rename_volume(f.book, f.vol, "序卷 风起").unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let v = b.volumes.iter().find(|v| v.id == f.vol).unwrap();
    assert_eq!(v.title, "序卷 风起");
    assert_eq!(v.chapters.len(), 2, "两章都要还在");
    // 章还能正常读正文，id 没变。
    assert!(v.chapters.iter().any(|c| c.id == f.ch1));
    let body = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    assert!(body.contains("第一章的正文"), "正文还在：{body}");
}

/// 书改名：只改 book.toml，不搬目录（书目录名是 id）。
#[test]
fn rename_book_changes_title() {
    let f = setup();
    f.store().rename_book(f.book, "归途").unwrap();
    let b = f.store().load_book(f.book).unwrap();
    assert_eq!(b.title, "归途");
    assert_eq!(b.author, "沈砚", "作者不该被顺手清掉");
    assert_eq!(b.volumes.len(), 1, "卷章结构不受影响");
}

// ---- 删除（软删到 trash，§0 可撤销）----

#[test]
fn delete_chapter_moves_to_trash() {
    let f = setup();
    f.store().delete_chapter(f.book, f.ch1).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    assert!(
        !b.volumes[0].chapters.iter().any(|c| c.id == f.ch1),
        "删掉的章不该再出现在树里"
    );
    assert!(
        b.volumes[0].chapters.iter().any(|c| c.id == f.ch2),
        "别的章还在"
    );
    // §0：进 trash，不是真删。
    let trashed = f
        .dir
        .path()
        .join("books")
        .join(f.book.to_string())
        .join("trash")
        .join("chapters")
        .join(format!("{}.md", f.ch1));
    assert!(trashed.exists(), "删掉的章应进 trash");
}

#[test]
fn delete_volume_takes_its_chapters_to_trash() {
    let f = setup();
    f.store().delete_volume(f.book, f.vol).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    assert!(b.volumes.is_empty(), "卷删了，树上不该还有它");
    let trashed = f
        .dir
        .path()
        .join("books")
        .join(f.book.to_string())
        .join("trash")
        .join("volumes")
        .join(f.vol.to_string());
    assert!(trashed.exists(), "整卷应进 trash");
}

#[test]
fn delete_book_moves_whole_book_to_workspace_trash() {
    let f = setup();
    f.store().delete_book(f.book).unwrap();

    // 书架上不该再有它。
    assert!(
        !f.store()
            .list_books()
            .unwrap()
            .iter()
            .any(|b| b.id == f.book),
        "删掉的书不该出现在书架"
    );
    // §0：进工作区级 trash。
    let trashed = f.ws().trash_dir().join("books").join(f.book.to_string());
    assert!(trashed.exists(), "整本书应进工作区 trash");
    assert!(
        trashed.join("book.toml").exists(),
        "书的内容应完整搬进 trash（能人工恢复）"
    );
}

// ---- 移动章 ----

/// 卷内重排：把第一章移到第二章之后，顺序应对调。
#[test]
fn move_chapter_reorders_within_volume() {
    let f = setup();
    // 初始顺序 ch1, ch2。
    f.store()
        .move_chapter(f.book, f.ch1, f.vol, Some(f.ch2))
        .unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let ids: Vec<ChapterId> = b.volumes[0].chapters.iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![f.ch2, f.ch1], "第一章应排到第二章之后");
    // 正文与标题不动。
    let body = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    assert!(body.contains("第一章的正文"), "移动不该碰正文：{body}");
}

/// 跨卷移动：把一章挪到另一卷，源卷少一章、目标卷多一章，id 不变。
#[test]
fn move_chapter_across_volumes() {
    let f = setup();
    let vol2 = f
        .store()
        .create_volume(f.book, "第二卷", Some(f.vol))
        .unwrap();

    f.store().move_chapter(f.book, f.ch1, vol2, None).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let v1 = b.volumes.iter().find(|v| v.id == f.vol).unwrap();
    let v2 = b.volumes.iter().find(|v| v.id == vol2).unwrap();
    assert!(!v1.chapters.iter().any(|c| c.id == f.ch1), "源卷不该再有它");
    assert!(v2.chapters.iter().any(|c| c.id == f.ch1), "目标卷应有它");
    // 跨卷后正文照样读得出。
    let body = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    assert!(body.contains("第一章的正文"), "跨卷移动不该丢正文：{body}");
}

// ---- 置顶 / 归档（§6.1）----

/// 置顶落盘、跨重启还在，且书架排序把它顶到最前。
#[test]
fn pin_persists_and_sorts_to_top() {
    let f = setup();
    // 再建两本，书名让默认排序把 setup 的「雪夜行」夹在中间。
    let mut s = f.store();
    let early = s.create_book("阿书", "作者").unwrap(); // 排最前
    s.create_book("子书", "作者").unwrap(); // 排最后

    // 置顶「子书」——它本该垫底，置顶后要窜到最前。
    let zi = s
        .list_books()
        .unwrap()
        .into_iter()
        .find(|b| b.title == "子书")
        .unwrap();
    s.set_book_pinned(zi.id, true).unwrap();

    let store = f.store();
    let names: Vec<String> = store
        .list_books()
        .unwrap()
        .into_iter()
        .map(|b| b.title)
        .collect();
    assert_eq!(
        names.first().map(String::as_str),
        Some("子书"),
        "置顶的应最前：{names:?}"
    );
    // 没置顶的仍按书名。
    assert!(
        names.iter().position(|n| n == "阿书") < names.iter().position(|n| n == "雪夜行"),
        "未置顶的仍按书名：{names:?}"
    );
    let _ = early;
}

/// 归档落盘、跨重启还在，且沉到书架最底。
#[test]
fn archive_persists_and_sorts_to_bottom() {
    let f = setup();
    let mut s = f.store();
    s.create_book("阿书", "作者").unwrap();

    // 归档「阿书」——它本该排最前，归档后沉到最底。
    let a = s
        .list_books()
        .unwrap()
        .into_iter()
        .find(|b| b.title == "阿书")
        .unwrap();
    s.set_book_archived(a.id, true).unwrap();

    let store = f.store();
    let books = store.list_books().unwrap();
    let names: Vec<&str> = books.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(names.last(), Some(&"阿书"), "归档的应沉最底：{names:?}");
    // 归档不删——书还在，archived 标着。
    assert!(books.iter().find(|b| b.title == "阿书").unwrap().archived);
}

/// 取消置顶/归档也要生效。
#[test]
fn unpin_and_unarchive() {
    let f = setup();
    let mut s = f.store();
    s.set_book_pinned(f.book, true).unwrap();
    s.set_book_archived(f.book, true).unwrap();
    s.set_book_pinned(f.book, false).unwrap();
    s.set_book_archived(f.book, false).unwrap();

    let b = f.store().load_book(f.book).unwrap();
    assert!(!b.pinned && !b.archived, "取消后两个标志都该清掉");
}

/// 设状态：改 front matter、不动正文与文件名，跨重启还在（§6.2）。
#[test]
fn set_chapter_status_persists_without_touching_body() {
    use mj_core::model::ChapterStatus;
    let f = setup();
    let body_before = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();

    f.store()
        .set_chapter_status(f.book, f.ch1, ChapterStatus::Done)
        .unwrap();

    let b = f.store().load_book(f.book).unwrap();
    let ch = b.volumes[0]
        .chapters
        .iter()
        .find(|c| c.id == f.ch1)
        .unwrap();
    assert_eq!(ch.status, ChapterStatus::Done, "状态应已改到盘上");
    let body_after = f.store().load_body(f.book, f.ch1).unwrap().text.to_string();
    assert_eq!(body_after, body_before, "改状态不该碰正文");
}

/// 改名用带特殊字符/纯中文的标题也不能崩（slug 会退化，但文件名总要合法）。
#[test]
fn rename_with_tricky_titles_stays_loadable() {
    let f = setup();
    for title in ["第一章：风雪/夜", "🌟 星", "   ", "A & B"] {
        f.store().rename_chapter(f.book, f.ch1, title).unwrap();
        let b = f.store().load_book(f.book).unwrap();
        let ch = b.volumes[0]
            .chapters
            .iter()
            .find(|c| c.id == f.ch1)
            .unwrap();
        assert_eq!(ch.title, title, "标题应原样存住：{title:?}");
        // 正文始终读得出。
        assert!(
            f.store().load_body(f.book, f.ch1).is_ok(),
            "{title:?} 后读不出正文"
        );
    }
}
