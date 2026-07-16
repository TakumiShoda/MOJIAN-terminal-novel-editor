//! 书架。见 doc.md §6.1、§7.1。
//!
//! 启动进入书架页。此处只管状态与导航，渲染在 app.rs。

use mj_core::id::BookId;
use mj_core::model::Book;

/// 书架状态。
#[derive(Debug, Default)]
pub struct Shelf {
    books: Vec<Book>,
    cursor: usize,
}

impl Shelf {
    pub fn new(books: Vec<Book>) -> Self {
        Self { books, cursor: 0 }
    }

    pub fn books(&self) -> &[Book] {
        &self.books
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selected(&self) -> Option<&Book> {
        self.books.get(self.cursor)
    }

    pub fn selected_id(&self) -> Option<BookId> {
        self.selected().map(|b| b.id)
    }

    /// 移动高亮。停在两端而非回绕。
    pub fn move_down(&mut self) {
        if !self.books.is_empty() {
            self.cursor = (self.cursor + 1).min(self.books.len() - 1);
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// 重新载入书目（新建书之后调用）。尽量保持高亮在同一本书上——
    /// 列表按标题排序，新建一本可能让原来的书换位置。
    pub fn reload(&mut self, books: Vec<Book>, keep: Option<BookId>) {
        self.books = books;
        self.cursor = keep
            .and_then(|id| self.books.iter().position(|b| b.id == id))
            .unwrap_or(0);
        self.cursor = self.cursor.min(self.books.len().saturating_sub(1));
    }

    /// 一本书的统计摘要（§6.1：卷数章数、总字数、进度）。
    ///
    /// 字数取各章的缓存值，不读正文——书架必须 < 100ms 打开（§6.1）。
    pub fn summary(book: &Book) -> BookSummary {
        let words: u64 = book
            .volumes
            .iter()
            .flat_map(|v| &v.chapters)
            .filter_map(|c| c.word_count)
            .sum();
        BookSummary {
            volumes: book.volumes.len(),
            chapters: book.chapter_count(),
            words,
            progress: book
                .target_words
                .and_then(|t| (t > 0).then(|| (words as f64 / t as f64).min(1.0))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookSummary {
    pub volumes: usize,
    pub chapters: usize,
    pub words: u64,
    /// 进度（0..1）。仅当设了目标字数时有值（§6.1）。
    pub progress: Option<f64>,
}

/// 把字数写成中文习惯的形式：过万用「万」。
///
/// 状态栏与书架都要显示字数（§7.2 的「21.7万字」）。`217000` 远不如
/// `21.7万` 好读——后者才是中文写作者描述篇幅时的实际说法。
pub fn format_words(n: u64) -> String {
    if n < 10_000 {
        // 千位分隔：3128 -> 3,128
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    } else {
        let wan = n as f64 / 10_000.0;
        if wan >= 100.0 {
            format!("{wan:.0}万")
        } else {
            format!("{wan:.1}万")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_core::id::{ChapterId, VolumeId};
    use mj_core::model::{ChapterMeta, ChapterStatus, Volume};

    fn chapter(words: u64) -> ChapterMeta {
        ChapterMeta {
            id: ChapterId::generate(),
            title: "章".into(),
            order: 10,
            status: ChapterStatus::Draft,
            word_count: Some(words),
            tags: Vec::new(),
            path: "x.md".into(),
            updated: None,
            damaged: None,
        }
    }

    fn book_with(title: &str, words: &[u64]) -> Book {
        let mut b = Book::new(BookId::generate(), title, "作者");
        let mut v = Volume::new(VolumeId::generate(), "卷", 10);
        for w in words {
            v.chapters.push(chapter(*w));
        }
        b.volumes.push(v);
        b
    }

    #[test]
    fn navigates_and_stops_at_ends() {
        let mut s = Shelf::new(vec![book_with("甲", &[100]), book_with("乙", &[200])]);
        assert_eq!(s.cursor(), 0);
        s.move_up();
        assert_eq!(s.cursor(), 0, "顶部再上移仍是 0");
        s.move_down();
        s.move_down();
        assert_eq!(s.cursor(), 1, "底部再下移仍是末项");
    }

    #[test]
    fn empty_shelf_is_safe() {
        let mut s = Shelf::new(Vec::new());
        s.move_down();
        s.move_up();
        assert!(s.selected().is_none(), "空书架不应 panic");
        assert!(s.is_empty());
    }

    #[test]
    fn summary_sums_cached_words() {
        let b = book_with("书", &[3128, 2000, 500]);
        let sum = Shelf::summary(&b);
        assert_eq!(sum.chapters, 3);
        assert_eq!(sum.volumes, 1);
        assert_eq!(sum.words, 5628);
        assert!(sum.progress.is_none(), "未设目标字数时无进度");
    }

    #[test]
    fn summary_computes_progress_when_target_set() {
        let mut b = book_with("书", &[5000]);
        b.target_words = Some(10_000);
        assert_eq!(Shelf::summary(&b).progress, Some(0.5));
    }

    /// 超额完成时进度封顶 1.0——进度条不该冲出格子。
    #[test]
    fn progress_is_capped_at_one() {
        let mut b = book_with("书", &[30_000]);
        b.target_words = Some(10_000);
        assert_eq!(Shelf::summary(&b).progress, Some(1.0));
    }

    #[test]
    fn zero_target_does_not_divide_by_zero() {
        let mut b = book_with("书", &[100]);
        b.target_words = Some(0);
        assert_eq!(Shelf::summary(&b).progress, None);
    }

    #[test]
    fn reload_keeps_selection_on_same_book() {
        let a = book_with("甲", &[1]);
        let b = book_with("乙", &[1]);
        let b_id = b.id;
        let mut s = Shelf::new(vec![a, b]);
        s.move_down();
        assert_eq!(s.selected_id(), Some(b_id));

        // 新增一本书排在最前，原选中的书位置变了。
        let new = book_with("阿", &[1]);
        let a2 = book_with("甲", &[1]);
        let b2 = Book {
            id: b_id,
            ..book_with("乙", &[1])
        };
        s.reload(vec![new, a2, b2], Some(b_id));
        assert_eq!(s.selected_id(), Some(b_id), "高亮应跟着书走");
    }

    #[test]
    fn reload_falls_back_when_book_gone() {
        let mut s = Shelf::new(vec![book_with("甲", &[1])]);
        s.reload(vec![book_with("乙", &[1])], Some(BookId::generate()));
        assert_eq!(s.cursor(), 0, "原书不在了应回到首项");
    }

    #[test]
    fn formats_words_in_chinese_convention() {
        assert_eq!(format_words(0), "0");
        assert_eq!(format_words(999), "999");
        assert_eq!(format_words(3128), "3,128");
        assert_eq!(format_words(9999), "9,999");
        assert_eq!(format_words(10_000), "1.0万");
        assert_eq!(format_words(217_000), "21.7万");
        assert_eq!(format_words(1_000_000), "100万");
    }
}
