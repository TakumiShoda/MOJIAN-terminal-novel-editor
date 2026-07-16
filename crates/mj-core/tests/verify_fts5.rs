//! [VERIFY] doc.md §5.4：确认 rusqlite bundled 是否启用 FTS5 与 trigram 分词器。
//! 结论若为否，全书搜索需回退到「遍历 + 内存匹配」。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rusqlite::Connection;

#[test]
fn fts5_trigram_available_and_matches_cjk() {
    let db = Connection::open_in_memory().unwrap();

    db.execute_batch(
        "CREATE VIRTUAL TABLE chapter_fts USING fts5(
             chapter_id UNINDEXED, title, body, tokenize = 'trigram'
         );",
    )
    .expect("FTS5 + trigram 应可用");

    db.execute(
        "INSERT INTO chapter_fts(chapter_id, title, body) VALUES (?1, ?2, ?3)",
        (
            "ch_1",
            "第一章 雪夜",
            "　　雪落了一夜。他推开门，风裹着雪灌进来。",
        ),
    )
    .unwrap();

    // 中文子串检索：trigram 下无需分词器即可命中。
    let n: i64 = db
        .query_row(
            "SELECT count(*) FROM chapter_fts WHERE chapter_fts MATCH ?1",
            ["风裹着雪"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "trigram 应能命中中文子串");
}
