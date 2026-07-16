//! SQLite 索引。见 doc.md §5.4。
//!
//! **索引是可重建的缓存，不是真相**（§0 禁令 3）。正文的真相永远是磁盘上的
//! 纯文本文件。故：
//! - schema 版本不符或文件损坏 → **直接删掉重建，不得报错阻塞用户**（§5.4 明言）；
//! - 任何索引操作失败都不应中断写作——最坏情况是搜索慢一点、字数要重算。
//!
//! 用途：全书搜索、字数汇总、码字量统计、校对结果缓存。
//! M2 只实装字数汇总与码字量；搜索是 M3、校对缓存是 M5。

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::id::{BookId, ChapterId};

/// schema 版本。改动表结构时 +1——旧库会被自动删掉重建。
const SCHEMA_VERSION: i32 = 1;

pub struct Index {
    db: Connection,
}

impl Index {
    /// 打开索引。损坏或版本不符时删掉重建。
    ///
    /// 返回 `Result` 只为「连重建都失败」这种极端情况（磁盘满/只读）。
    /// 即便如此，调用方也应降级运行而非退出——见 `open_or_disabled`。
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        match Self::try_open_existing(path) {
            Ok(Some(idx)) => return Ok(idx),
            Ok(None) => {
                tracing::info!(path = %path.display(), "索引 schema 版本不符，重建");
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "索引损坏，删除重建");
            }
        }
        // 删掉重来。索引没有不可再生的信息，删除是安全的。
        let _ = std::fs::remove_file(path);
        Self::create(path)
    }

    /// 尝试打开既有索引。版本不符返回 Ok(None)，损坏返回 Err。
    fn try_open_existing(path: &Path) -> rusqlite::Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let db = Connection::open(path)?;
        // 完整性自检：损坏的库在这里就会报错，而不是等到用户搜索时。
        let ok: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let v: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v != SCHEMA_VERSION {
            return Ok(None);
        }
        Ok(Some(Self { db }))
    }

    fn create(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;

             CREATE TABLE chapter_index (
               chapter_id TEXT PRIMARY KEY, book_id TEXT, volume_id TEXT,
               title TEXT, order_key INTEGER, path TEXT,
               content_hash TEXT,
               words_with_punct INTEGER, words_no_punct INTEGER, han_chars INTEGER,
               updated INTEGER
             );
             CREATE INDEX idx_chapter_book ON chapter_index(book_id);

             CREATE TABLE daily_words (
               book_id TEXT, day TEXT, delta INTEGER,
               PRIMARY KEY(book_id, day)
             );

             PRAGMA user_version = {SCHEMA_VERSION};"
        ))?;
        Ok(Self { db })
    }

    /// 内存索引，供测试。
    pub fn in_memory() -> rusqlite::Result<Self> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(&format!(
            "CREATE TABLE chapter_index (
               chapter_id TEXT PRIMARY KEY, book_id TEXT, volume_id TEXT,
               title TEXT, order_key INTEGER, path TEXT,
               content_hash TEXT,
               words_with_punct INTEGER, words_no_punct INTEGER, han_chars INTEGER,
               updated INTEGER
             );
             CREATE TABLE daily_words (
               book_id TEXT, day TEXT, delta INTEGER,
               PRIMARY KEY(book_id, day)
             );
             PRAGMA user_version = {SCHEMA_VERSION};"
        ))?;
        Ok(Self { db })
    }

    /// 记录一章的字数与内容哈希。
    ///
    /// `content_hash` 用于判断是否需要重新索引：哈希没变就不必重算字数
    /// （§5.4）。这是 100 万字全书统计能在 1s 内完成的关键。
    pub fn upsert_chapter(&self, e: &ChapterEntry) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO chapter_index
               (chapter_id, book_id, volume_id, title, order_key, path,
                content_hash, words_with_punct, words_no_punct, han_chars, updated)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(chapter_id) DO UPDATE SET
               book_id=?2, volume_id=?3, title=?4, order_key=?5, path=?6,
               content_hash=?7, words_with_punct=?8, words_no_punct=?9,
               han_chars=?10, updated=?11",
            rusqlite::params![
                e.chapter.to_string(),
                e.book.to_string(),
                e.volume.clone(),
                e.title.clone(),
                e.order,
                e.path.to_string_lossy(),
                e.content_hash.clone(),
                e.words_with_punct as i64,
                e.words_no_punct as i64,
                e.han_chars as i64,
                e.updated,
            ],
        )?;
        Ok(())
    }

    /// 取某章已索引的内容哈希。与当前文件哈希一致则无需重新统计。
    pub fn chapter_hash(&self, ch: ChapterId) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT content_hash FROM chapter_index WHERE chapter_id = ?1",
                [ch.to_string()],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 全书字数汇总。
    pub fn book_totals(&self, book: BookId) -> rusqlite::Result<BookTotals> {
        self.db.query_row(
            "SELECT COALESCE(SUM(words_with_punct),0), COALESCE(SUM(words_no_punct),0),
                    COALESCE(SUM(han_chars),0), COUNT(*)
             FROM chapter_index WHERE book_id = ?1",
            [book.to_string()],
            |r| {
                Ok(BookTotals {
                    with_punct: r.get::<_, i64>(0)? as u64,
                    no_punct: r.get::<_, i64>(1)? as u64,
                    han: r.get::<_, i64>(2)? as u64,
                    chapters: r.get::<_, i64>(3)? as usize,
                })
            },
        )
    }

    /// 删除某书的所有索引条目（书被删除或重建索引时）。
    pub fn clear_book(&self, book: BookId) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM chapter_index WHERE book_id = ?1",
            [book.to_string()],
        )?;
        Ok(())
    }

    /// 删除某章的索引条目。
    pub fn remove_chapter(&self, ch: ChapterId) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM chapter_index WHERE chapter_id = ?1",
            [ch.to_string()],
        )?;
        Ok(())
    }

    // ---- 今日码字量（§6.4）----

    /// 累加某日的净增字数。删改为负。
    pub fn add_daily_delta(&self, book: BookId, day: &str, delta: i64) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO daily_words (book_id, day, delta) VALUES (?1, ?2, ?3)
             ON CONFLICT(book_id, day) DO UPDATE SET delta = delta + ?3",
            rusqlite::params![book.to_string(), day, delta],
        )?;
        Ok(())
    }

    /// 某日的净增字数。
    pub fn daily_delta(&self, book: BookId, day: &str) -> rusqlite::Result<i64> {
        self.db
            .query_row(
                "SELECT delta FROM daily_words WHERE book_id = ?1 AND day = ?2",
                rusqlite::params![book.to_string(), day],
                |r| r.get(0),
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(0),
                other => Err(other),
            })
    }
}

/// 一章的索引条目。
#[derive(Debug, Clone)]
pub struct ChapterEntry {
    pub chapter: ChapterId,
    pub book: BookId,
    pub volume: String,
    pub title: String,
    pub order: u32,
    pub path: PathBuf,
    /// blake3(正文)，判断是否需要重新索引（§5.4）。
    pub content_hash: String,
    pub words_with_punct: u64,
    pub words_no_punct: u64,
    pub han_chars: u64,
    pub updated: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BookTotals {
    pub with_punct: u64,
    pub no_punct: u64,
    pub han: u64,
    pub chapters: usize,
}

/// 正文的内容哈希。
pub fn content_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

/// 按「一天从凌晨 N 点开始」切分出日期（§6.4）。
///
/// 写作者常见作息：凌晨 3 点写的字算前一天的——他自己也是这么认的，
/// 那是「昨晚」的工作，不是「今天」的。
pub fn writing_day(now: chrono::DateTime<chrono::Local>, day_starts_at: u8) -> String {
    use chrono::{Duration, Timelike};
    let shifted = if now.hour() < day_starts_at as u32 {
        now - Duration::hours(day_starts_at as i64)
    } else {
        now
    };
    shifted.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::id::VolumeId;

    fn entry(book: BookId, ch: ChapterId, words: u64) -> ChapterEntry {
        ChapterEntry {
            chapter: ch,
            book,
            volume: VolumeId::generate().to_string(),
            title: "章".into(),
            order: 10,
            path: "x.md".into(),
            content_hash: content_hash("正文"),
            words_with_punct: words,
            words_no_punct: words,
            han_chars: words,
            updated: 0,
        }
    }

    #[test]
    fn upserts_and_totals() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        idx.upsert_chapter(&entry(book, ChapterId::generate(), 3128))
            .unwrap();
        idx.upsert_chapter(&entry(book, ChapterId::generate(), 2000))
            .unwrap();

        let t = idx.book_totals(book).unwrap();
        assert_eq!(t.with_punct, 5128);
        assert_eq!(t.chapters, 2);
    }

    /// 同一章重复索引应更新而非累加——否则字数会随保存次数暴涨。
    #[test]
    fn upsert_updates_instead_of_duplicating() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        let ch = ChapterId::generate();

        idx.upsert_chapter(&entry(book, ch, 100)).unwrap();
        idx.upsert_chapter(&entry(book, ch, 200)).unwrap();

        let t = idx.book_totals(book).unwrap();
        assert_eq!(t.chapters, 1, "同一章不应出现两条");
        assert_eq!(t.with_punct, 200, "应是新值而非累加");
    }

    #[test]
    fn totals_of_unknown_book_are_zero() {
        let idx = Index::in_memory().unwrap();
        let t = idx.book_totals(BookId::generate()).unwrap();
        assert_eq!(t, BookTotals::default(), "无数据应返回 0 而非报错");
    }

    #[test]
    fn tracks_content_hash() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        let ch = ChapterId::generate();
        assert_eq!(idx.chapter_hash(ch).unwrap(), None, "未索引应返回 None");

        idx.upsert_chapter(&entry(book, ch, 10)).unwrap();
        assert_eq!(idx.chapter_hash(ch).unwrap(), Some(content_hash("正文")));
    }

    #[test]
    fn content_hash_is_stable_and_distinct() {
        assert_eq!(content_hash("雪落了一夜"), content_hash("雪落了一夜"));
        assert_ne!(content_hash("雪落了一夜"), content_hash("雪落了两夜"));
    }

    #[test]
    fn clear_book_removes_entries() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        idx.upsert_chapter(&entry(book, ChapterId::generate(), 100))
            .unwrap();
        idx.clear_book(book).unwrap();
        assert_eq!(idx.book_totals(book).unwrap().chapters, 0);
    }

    #[test]
    fn remove_chapter_removes_one() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        let a = ChapterId::generate();
        idx.upsert_chapter(&entry(book, a, 100)).unwrap();
        idx.upsert_chapter(&entry(book, ChapterId::generate(), 50))
            .unwrap();
        idx.remove_chapter(a).unwrap();

        let t = idx.book_totals(book).unwrap();
        assert_eq!(t.chapters, 1);
        assert_eq!(t.with_punct, 50);
    }

    // ---- 今日码字量 ----

    #[test]
    fn daily_delta_accumulates() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        idx.add_daily_delta(book, "2026-07-17", 500).unwrap();
        idx.add_daily_delta(book, "2026-07-17", 740).unwrap();
        assert_eq!(idx.daily_delta(book, "2026-07-17").unwrap(), 1240);
    }

    /// 删改为负（§6.4）。
    #[test]
    fn daily_delta_can_go_negative() {
        let idx = Index::in_memory().unwrap();
        let book = BookId::generate();
        idx.add_daily_delta(book, "2026-07-17", 100).unwrap();
        idx.add_daily_delta(book, "2026-07-17", -300).unwrap();
        assert_eq!(
            idx.daily_delta(book, "2026-07-17").unwrap(),
            -200,
            "删得比写得多，今日为负"
        );
    }

    #[test]
    fn daily_delta_of_unknown_day_is_zero() {
        let idx = Index::in_memory().unwrap();
        assert_eq!(
            idx.daily_delta(BookId::generate(), "2026-01-01").unwrap(),
            0
        );
    }

    #[test]
    fn daily_delta_is_per_book() {
        let idx = Index::in_memory().unwrap();
        let a = BookId::generate();
        let b = BookId::generate();
        idx.add_daily_delta(a, "2026-07-17", 100).unwrap();
        assert_eq!(idx.daily_delta(b, "2026-07-17").unwrap(), 0, "不应串书");
    }

    // ---- 写作日切分 ----

    #[test]
    fn writing_day_shifts_early_morning_to_previous_day() {
        use chrono::TimeZone;
        // 凌晨 3 点写的字，按「4 点切日」应算前一天。
        let t = chrono::Local
            .with_ymd_and_hms(2026, 7, 17, 3, 0, 0)
            .unwrap();
        assert_eq!(writing_day(t, 4), "2026-07-16", "凌晨 3 点算昨天");
    }

    #[test]
    fn writing_day_after_cutoff_is_today() {
        use chrono::TimeZone;
        let t = chrono::Local
            .with_ymd_and_hms(2026, 7, 17, 5, 0, 0)
            .unwrap();
        assert_eq!(writing_day(t, 4), "2026-07-17", "5 点算今天");
        let t = chrono::Local
            .with_ymd_and_hms(2026, 7, 17, 23, 0, 0)
            .unwrap();
        assert_eq!(writing_day(t, 4), "2026-07-17");
    }

    #[test]
    fn writing_day_with_zero_cutoff_is_natural_day() {
        use chrono::TimeZone;
        let t = chrono::Local
            .with_ymd_and_hms(2026, 7, 17, 0, 30, 0)
            .unwrap();
        assert_eq!(writing_day(t, 0), "2026-07-17", "0 点切日 = 自然日");
    }
}
