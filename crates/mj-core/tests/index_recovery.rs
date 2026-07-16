//! 索引的自愈。见 doc.md §5.4：
//! 「启动时如果 schema 版本不符或文件损坏，**直接删掉重建，不得报错阻塞用户**」。
//!
//! 索引是缓存不是真相（§0 禁令 3）——它坏了，用户的稿子一个字都没少，
//! 所以绝不该因此打不开程序。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::index::Index;

#[test]
fn opens_fresh_index() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".index.sqlite");
    assert!(Index::open(&p).is_ok());
    assert!(p.exists(), "应创建索引文件");
}

/// 完全损坏的文件（不是 SQLite 库）必须被删掉重建，而不是报错。
#[test]
fn corrupt_file_is_rebuilt_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".index.sqlite");
    std::fs::write(&p, "这根本不是数据库，是用户手滑写进去的垃圾").unwrap();

    let idx = Index::open(&p);
    assert!(idx.is_ok(), "损坏的索引必须能自愈，不得阻塞用户");

    // 重建后应可正常使用。
    let idx = idx.unwrap();
    let book = mj_core::id::BookId::generate();
    idx.add_daily_delta(book, "2026-07-17", 100).unwrap();
    assert_eq!(idx.daily_delta(book, "2026-07-17").unwrap(), 100);
}

/// 截断的 SQLite 文件（写到一半断电）同样要能自愈。
#[test]
fn truncated_db_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".index.sqlite");

    // 先建一个正常的库并写入数据。
    {
        let idx = Index::open(&p).unwrap();
        idx.add_daily_delta(mj_core::id::BookId::generate(), "2026-07-17", 1)
            .unwrap();
    }
    // 拦腰截断。
    let data = std::fs::read(&p).unwrap();
    std::fs::write(&p, &data[..data.len() / 2]).unwrap();

    assert!(Index::open(&p).is_ok(), "截断的索引应被重建");
}

/// schema 版本不符 → 重建（未来改表结构时靠这条自动迁移）。
#[test]
fn wrong_schema_version_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".index.sqlite");

    // 造一个版本号不对的库。
    {
        let db = rusqlite::Connection::open(&p).unwrap();
        db.execute_batch("CREATE TABLE old_stuff (x INTEGER); PRAGMA user_version = 999;")
            .unwrap();
    }

    let idx = Index::open(&p).unwrap();
    // 新 schema 可用即证明已重建。
    let book = mj_core::id::BookId::generate();
    idx.add_daily_delta(book, "2026-07-17", 42).unwrap();
    assert_eq!(idx.daily_delta(book, "2026-07-17").unwrap(), 42);
}

/// 重建会丢掉索引数据——这是可接受的，因为它可从正文重算。
/// 但绝不能丢正文：索引与正文是两个文件，删索引碰不到 .md。
#[test]
fn rebuilding_index_does_not_touch_manuscripts() {
    let dir = tempfile::tempdir().unwrap();
    let manuscript = dir.path().join("0010-开篇.md");
    let body = "+++\nid = \"ch_7Q2M4KZA\"\n+++\n　　雪落了一夜。";
    std::fs::write(&manuscript, body).unwrap();

    let p = dir.path().join(".index.sqlite");
    std::fs::write(&p, "垃圾").unwrap();
    let _ = Index::open(&p).unwrap();

    assert_eq!(
        std::fs::read_to_string(&manuscript).unwrap(),
        body,
        "重建索引不得碰正文"
    );
}

/// 正常关闭后重开，数据应还在——自愈不能变成「每次都重建」。
#[test]
fn healthy_index_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".index.sqlite");
    let book = mj_core::id::BookId::generate();

    {
        let idx = Index::open(&p).unwrap();
        idx.add_daily_delta(book, "2026-07-17", 1240).unwrap();
    }
    let idx = Index::open(&p).unwrap();
    assert_eq!(
        idx.daily_delta(book, "2026-07-17").unwrap(),
        1240,
        "健康的索引不该被无谓重建"
    );
}
