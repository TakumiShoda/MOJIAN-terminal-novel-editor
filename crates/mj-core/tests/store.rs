//! Store 集成测试。见 doc.md §11 M1 验收、§6.1、§6.2。
//!
//! M1 验收原文：「能新建书→建卷→建章→写 3000 字→重启内容完好」。
//! 「重启」在测试里体现为：丢弃 Store 实例，从磁盘重新扫描——
//! 这正是真实重启时发生的事（内存全丢，磁盘是唯一真相）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::model::ChapterBody;
use mj_core::store::Store;
use mj_core::workspace::Workspace;

/// 建一个临时 workspace 与 Store。返回 tempdir 以保持其存活。
fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let store = Store::new(ws, Config::default());
    (dir, store)
}

/// 模拟重启：丢弃旧 Store，从同一个 workspace 目录重新建一个。
fn restart(dir: &tempfile::TempDir) -> Store {
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    Store::new(ws, Config::default())
}

/// M1 验收主线：新建书 → 建卷 → 建章 → 写 3000 字 → 重启内容完好。
#[test]
fn m1_acceptance_write_3000_chars_and_restart() {
    let (dir, mut store) = setup();

    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch = store
        .create_chapter(book.id, vol, "第一章 雪夜", None)
        .unwrap();

    // 3000 字中文正文，带段首缩进——贴近真实手稿。
    let paragraph = "　　雪落了一夜。他推开门，风裹着雪灌进来，冷得刺骨。\n";
    let mut text = String::new();
    while mj_text::count::count_with_punct(&text) < 3000 {
        text.push_str(paragraph);
    }
    let written = mj_text::count::count_with_punct(&text);
    assert!(written >= 3000, "测试数据不足 3000 字");

    let body = ChapterBody::new(ch, &text);
    store.save_body(book.id, &body).unwrap();

    // ---- 重启 ----
    let store = restart(&dir);

    let books = store.list_books().unwrap();
    assert_eq!(books.len(), 1, "重启后应能扫到那本书");
    assert_eq!(books[0].title, "雪夜行");
    assert_eq!(books[0].author, "沈砚");
    assert_eq!(books[0].volumes.len(), 1);
    assert_eq!(books[0].volumes[0].title, "第一卷");
    assert_eq!(books[0].volumes[0].chapters.len(), 1);

    let meta = &books[0].volumes[0].chapters[0];
    assert_eq!(meta.id, ch, "章 id 必须稳定");
    assert_eq!(meta.title, "第一章 雪夜");
    assert_eq!(meta.word_count, Some(written as u64), "字数缓存应已写入");

    // 正文必须逐字完好——这是整条验收的核心。
    let loaded = store.load_body(book.id, ch).unwrap();
    assert_eq!(loaded.text.to_string(), text, "正文在重启后发生了变化");
}

/// §6.1 验收：手动往 books/ 里丢一个符合布局的目录，重启后书架能识别（自愈扫描）。
#[test]
fn discovers_manually_created_book_dir() {
    let (dir, store) = setup();

    let book_dir = dir.path().join("books").join("bk_MANW0123");
    let ch_dir = book_dir.join("volumes/010-shou-juan/chapters");
    std::fs::create_dir_all(&ch_dir).unwrap();

    std::fs::write(
        book_dir.join("book.toml"),
        "id = \"bk_MANW0123\"\ntitle = \"手工书\"\nauthor = \"某人\"\n",
    )
    .unwrap();
    std::fs::write(
        book_dir.join("volumes/010-shou-juan/volume.toml"),
        "id = \"vo_MANW0123\"\ntitle = \"首卷\"\norder = 10\n",
    )
    .unwrap();
    std::fs::write(
        ch_dir.join("0010-kaipian.md"),
        "+++\nid = \"ch_MANW0123\"\ntitle = \"开篇\"\n+++\n　　手写的正文。\n",
    )
    .unwrap();

    let books = store.list_books().unwrap();
    assert_eq!(books.len(), 1, "应识别手工创建的书");
    assert_eq!(books[0].title, "手工书");
    assert_eq!(books[0].volumes[0].chapters[0].title, "开篇");

    let body = store
        .load_body(books[0].id, books[0].volumes[0].chapters[0].id)
        .unwrap();
    assert_eq!(body.text.to_string(), "　　手写的正文。\n");
}

/// front matter 损坏的章**不得从树上消失**——正文就在磁盘上，
/// 消失会让用户以为稿子没了（§0 禁令 1 的精神）。
#[test]
fn damaged_chapter_stays_visible_and_unwritable() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "好章", None).unwrap();

    // 再加一章，然后把它的 front matter 弄坏。
    let broken = store
        .create_chapter(book.id, vol, "坏章", Some(ch))
        .unwrap();
    let broken_path = {
        let b = store.list_books().unwrap();
        let m = b[0].volumes[0]
            .chapters
            .iter()
            .find(|c| c.id == broken)
            .unwrap();
        dir.path().join(&m.path)
    };
    std::fs::write(
        &broken_path,
        "+++\nthis is not toml {{{\n+++\n　　这段正文必须救得回来。\n",
    )
    .unwrap();

    let store = restart(&dir);
    let books = store.list_books().unwrap();
    let chapters = &books[0].volumes[0].chapters;

    assert_eq!(chapters.len(), 2, "受损章不得从树上消失");

    let dmg = chapters.iter().find(|c| c.damaged.is_some()).unwrap();
    assert!(dmg.title.contains("损坏"), "标题应提示损坏: {}", dmg.title);

    // 正文仍在磁盘上，一字未动。
    let raw = std::fs::read_to_string(&broken_path).unwrap();
    assert!(raw.contains("这段正文必须救得回来"), "正文被动过了");
}

/// 受损章拒绝写入——写回会覆盖掉用户还能救回的内容。
#[test]
fn save_body_refuses_damaged_chapter() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let path = {
        let b = store.list_books().unwrap();
        dir.path().join(&b[0].volumes[0].chapters[0].path)
    };
    let original = "+++\nbad toml {{{\n+++\n　　原始正文。\n";
    std::fs::write(&path, original).unwrap();

    let mut store = restart(&dir);
    let damaged_id = {
        let b = store.list_books().unwrap();
        b[0].volumes[0].chapters[0].id
    };
    let _ = ch;

    let err = store.save_body(book.id, &ChapterBody::new(damaged_id, "新内容"));
    assert!(err.is_err(), "受损章不应允许写入");

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "拒绝写入后文件必须一字未动"
    );
}

/// 损坏的书不应让整个书架打不开——只跳过它。
#[test]
fn corrupt_book_does_not_break_shelf() {
    let (dir, mut store) = setup();
    store.create_book("好书", "作者").unwrap();

    // 目录名不参与解析（真相在 book.toml 的 id），故这里叫什么都行；
    // 让 book.toml 本身是坏的，才是这个测试要验的东西。
    let bad = dir.path().join("books").join("bk_BR0KEN01");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("book.toml"), "this is not toml {{{").unwrap();

    let books = store.list_books().unwrap();
    assert_eq!(books.len(), 1, "损坏的书应被跳过，好书仍在");
    assert_eq!(books[0].title, "好书");
}

/// §6.2 [MUST]：保存正文不得丢弃 front matter 里的未知字段。
#[test]
fn save_body_preserves_unknown_front_matter_fields() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    // 模拟用户手动往 front matter 加字段。
    let path = {
        let b = store.list_books().unwrap();
        dir.path().join(&b[0].volumes[0].chapters[0].path)
    };
    let raw = std::fs::read_to_string(&path).unwrap();
    let patched = raw.replacen("+++\n", "+++\n\"情绪\" = \"阴郁\"\n", 1);
    std::fs::write(&path, patched).unwrap();

    // 保存正文。
    let mut store = restart(&dir);
    let body = ChapterBody::new(ch, "　　新正文。");
    store.save_body(book.id, &body).unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("情绪"), "未知字段被保存吃掉了:\n{after}");
    assert!(after.contains("阴郁"), "未知字段的值丢失:\n{after}");
    assert!(after.contains("新正文"), "正文未写入");
}

/// 正文里不得混入私有标记：文件拿去别处必须能直接用（§5.2）。
#[test]
fn chapter_file_body_is_plain_text() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let text = "　　雪落了一夜。\n\n　　他推开门。\n";
    store
        .save_body(book.id, &ChapterBody::new(ch, text))
        .unwrap();

    let path = {
        let b = store.list_books().unwrap();
        dir.path().join(&b[0].volumes[0].chapters[0].path)
    };
    let raw = std::fs::read_to_string(&path).unwrap();
    // front matter 之后的部分应逐字等于正文。
    let body_part = raw.split("+++\n").nth(2).unwrap();
    assert_eq!(body_part, text, "正文被加料了");
}

/// 稀疏排序：连续插入不应重写既有章节的文件名（§5.3）。
#[test]
fn sparse_order_inserts_between() {
    let (_dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();

    let a = store.create_chapter(book.id, vol, "A", None).unwrap();
    let c = store.create_chapter(book.id, vol, "C", Some(a)).unwrap();
    // 在 A 与 C 之间插 B。
    let b = store.create_chapter(book.id, vol, "B", Some(a)).unwrap();

    let books = store.list_books().unwrap();
    let titles: Vec<_> = books[0].volumes[0]
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, ["A", "B", "C"], "插入顺序错误");

    // id 必须稳定。
    let ids: Vec<_> = books[0].volumes[0].chapters.iter().map(|c| c.id).collect();
    assert_eq!(ids, [a, b, c]);
}

/// 保存 → 加载往返，CRLF 配置下正文在内存里仍只有 LF（ADR 0003）。
#[test]
fn crlf_config_roundtrips_to_lf_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();

    let mut config = Config::default();
    config.general.line_ending = mj_text::eol::LineEnding::Native;
    let mut store = Store::new(ws, config);

    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let text = "第一行\n第二行\n";
    store
        .save_body(book.id, &ChapterBody::new(ch, text))
        .unwrap();

    let loaded = store.load_body(book.id, ch).unwrap();
    assert!(
        !loaded.text.to_string().contains('\r'),
        "内存里的正文不得含 CR"
    );
    assert_eq!(loaded.text.to_string(), text);
}

/// 章节标题含 Windows 非法字符时，文件仍应能建成（ADR 0003）。
#[test]
fn chapter_title_with_reserved_chars_is_safe_on_disk() {
    let (_dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store
        .create_chapter(book.id, vol, "第一章: 雪夜/风雪", None)
        .unwrap();

    let books = store.list_books().unwrap();
    let meta = &books[0].volumes[0].chapters[0];
    assert_eq!(meta.id, ch);
    assert_eq!(meta.title, "第一章: 雪夜/风雪", "标题本身应原样保留");

    let name = meta.path.file_name().unwrap().to_str().unwrap();
    assert!(
        !name.contains([':', '/', '\\', '?', '*']),
        "文件名含非法字符: {name}"
    );
}

/// 空章节能存能读——新建后立刻重启是常见路径。
#[test]
fn empty_chapter_roundtrips() {
    let (dir, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "空章", None).unwrap();

    let store = restart(&dir);
    let body = store.load_body(book.id, ch).unwrap();
    assert_eq!(body.text.to_string(), "");
}

/// 多卷多章的完整结构在重启后顺序正确。
#[test]
fn multi_volume_structure_survives_restart() {
    let (dir, mut store) = setup();
    let book = store.create_book("长篇", "作者").unwrap();

    let v1 = store.create_volume(book.id, "第一卷", None).unwrap();
    let v2 = store.create_volume(book.id, "第二卷", Some(v1)).unwrap();

    store.create_chapter(book.id, v1, "一之一", None).unwrap();
    let c12 = store.create_chapter(book.id, v1, "一之二", None).unwrap();
    store.create_chapter(book.id, v2, "二之一", None).unwrap();

    // 注意 create_chapter 的 after=None 是插到卷首，故「一之二」在前。
    let _ = c12;

    let store = restart(&dir);
    let books = store.list_books().unwrap();
    let b = &books[0];

    assert_eq!(b.volumes.len(), 2);
    assert_eq!(b.volumes[0].title, "第一卷", "卷顺序错误");
    assert_eq!(b.volumes[1].title, "第二卷");
    assert_eq!(b.volumes[0].chapters.len(), 2);
    assert_eq!(b.volumes[1].chapters.len(), 1);
    assert_eq!(b.chapter_count(), 3);
}
