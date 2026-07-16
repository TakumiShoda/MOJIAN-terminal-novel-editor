//! 崩溃恢复的集成测试。见 doc.md §6.3、§11 M1 验收「拔电测试不损坏」。
//!
//! 模拟真实的崩溃时序：写了 swp，但正文还没保存，进程就没了。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::model::ChapterBody;
use mj_core::store::Store;
use mj_core::workspace::Workspace;

fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    (dir, Store::new(ws, Config::default()))
}

/// 崩溃场景：swp 里有未保存的字，正文文件还是旧的。重启后必须救得回来。
#[test]
fn swap_recovers_unsaved_text_after_crash() {
    let (_d, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    // 已保存的版本。
    store
        .save_body(book.id, &ChapterBody::new(ch, "　　雪落了一夜。"))
        .unwrap();

    let path = store.chapter_file_path(book.id, ch).unwrap();

    // 用户又写了一段，自动保存还没触发，swp 已经写下——然后断电。
    let unsaved = "　　雪落了一夜。他推开门，风裹着雪灌进来。";
    mj_core::swap::write(&path, unsaved).unwrap();

    // ---- 重启 ----
    let saved = store.load_body(book.id, ch).unwrap().text.to_string();
    let recovery = mj_core::swap::detect(&path, &saved).unwrap().unwrap();

    assert!(recovery.differs(), "应检测到未保存的改动");
    assert_eq!(recovery.swap_body, unsaved, "断电前的字必须一字不差救回");
    assert_eq!(recovery.saved_body, "　　雪落了一夜。", "磁盘版本仍是旧的");
}

/// 正常保存后 swp 必须被清掉——否则下次启动会误报「有未保存的改动」。
#[test]
fn save_clears_swap() {
    let (_d, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let path = store.chapter_file_path(book.id, ch).unwrap();
    mj_core::swap::write(&path, "临时内容").unwrap();
    assert!(mj_core::swap::swap_path(&path).exists());

    store
        .save_body(book.id, &ChapterBody::new(ch, "正式内容"))
        .unwrap();

    assert!(
        !mj_core::swap::swap_path(&path).exists(),
        "保存后 swp 应被清理，否则下次启动会狼来了"
    );
}

/// swp 与磁盘内容一致 = 上次正常退出的残留，不构成真正的恢复。
#[test]
fn identical_swap_is_not_a_recovery() {
    let (_d, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let text = "　　雪落了一夜。";
    store
        .save_body(book.id, &ChapterBody::new(ch, text))
        .unwrap();
    let path = store.chapter_file_path(book.id, ch).unwrap();

    // 手动造一个与磁盘一致的 swp。
    mj_core::swap::write(&path, text).unwrap();

    let saved = store.load_body(book.id, ch).unwrap().text.to_string();
    let r = mj_core::swap::detect(&path, &saved).unwrap().unwrap();
    assert!(!r.differs(), "内容一致不该提示恢复");
}

/// swp 落在章节文件旁边，且是隐藏文件——不该污染用户的目录。
#[test]
fn swap_lives_next_to_chapter_and_is_hidden() {
    let (_d, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();

    let path = store.chapter_file_path(book.id, ch).unwrap();
    mj_core::swap::write(&path, "x").unwrap();

    let sp = mj_core::swap::swap_path(&path);
    assert_eq!(sp.parent(), path.parent(), "应与章节文件同目录");
    let name = sp.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with('.'), "应是隐藏文件: {name}");
    assert!(name.ends_with(".swp"), "应以 .swp 结尾: {name}");
}

/// swp 恢复的内容含中文与 emoji 时不得损坏。
#[test]
fn swap_preserves_cjk_and_emoji() {
    let (_d, mut store) = setup();
    let book = store.create_book("书", "作者").unwrap();
    let vol = store.create_volume(book.id, "卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "章", None).unwrap();
    let path = store.chapter_file_path(book.id, ch).unwrap();

    let body = "　　「你来了。」👨‍👩‍👧\n　　沈砚点头。";
    mj_core::swap::write(&path, body).unwrap();

    let r = mj_core::swap::detect(&path, "").unwrap().unwrap();
    assert_eq!(r.swap_body, body);
}
