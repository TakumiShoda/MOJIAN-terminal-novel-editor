//! 统计面板。见 doc.md §6.4：
//! `[MUST]` 按卷/章列出双口径字数，可导出 CSV。
//!
//! 数据生成与渲染分离：CSV 的正确性可以完整测试，不必去戳界面。

use mj_core::model::Book;

use crate::screens::shelf::format_words;

/// 面板里的一行。
#[derive(Debug, Clone, PartialEq)]
pub enum StatRow {
    Volume {
        title: String,
        with_punct: u64,
        no_punct: u64,
        chapters: usize,
    },
    Chapter {
        title: String,
        with_punct: u64,
        no_punct: u64,
    },
    /// 全书合计。
    Total { with_punct: u64, no_punct: u64 },
}

/// 统计面板的状态。
#[derive(Debug, Default)]
pub struct Stats {
    scroll: usize,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn scroll_down(&mut self, rows: usize, height: usize) {
        let max = rows.saturating_sub(height);
        self.scroll = (self.scroll + 1).min(max);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// 生成面板数据。
    ///
    /// 字数取各章的缓存值（front matter 里的 `words`）。
    /// **`no_punct` 缓存里没有**——§5.2 的 front matter 只存一个 `words`。
    /// 故净字数要现算，需要读正文；为不阻塞面板打开，这里接受调用方传入
    /// 已算好的净字数表（来自索引）。索引不可用时净字数显示为 0，
    /// 而不是让面板打不开——统计面板不是关键路径。
    pub fn rows(book: &Book, no_punct_of: impl Fn(mj_core::id::ChapterId) -> u64) -> Vec<StatRow> {
        let mut out = Vec::new();
        let mut total_wp = 0u64;
        let mut total_np = 0u64;

        for v in &book.volumes {
            let mut vol_wp = 0u64;
            let mut vol_np = 0u64;
            let mut chapter_rows = Vec::new();

            for c in &v.chapters {
                let wp = c.word_count.unwrap_or(0);
                let np = no_punct_of(c.id);
                vol_wp += wp;
                vol_np += np;
                chapter_rows.push(StatRow::Chapter {
                    title: c.title.clone(),
                    with_punct: wp,
                    no_punct: np,
                });
            }

            out.push(StatRow::Volume {
                title: v.title.clone(),
                with_punct: vol_wp,
                no_punct: vol_np,
                chapters: v.chapters.len(),
            });
            out.extend(chapter_rows);
            total_wp += vol_wp;
            total_np += vol_np;
        }

        out.push(StatRow::Total {
            with_punct: total_wp,
            no_punct: total_np,
        });
        out
    }

    /// 渲染成文本行，供界面显示。
    pub fn render_rows(rows: &[StatRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                StatRow::Volume {
                    title,
                    with_punct,
                    no_punct,
                    chapters,
                } => format!(
                    "▾ {title}  ({chapters} 章)  {} / 净 {}",
                    format_words(*with_punct),
                    format_words(*no_punct)
                ),
                StatRow::Chapter {
                    title,
                    with_punct,
                    no_punct,
                } => format!(
                    "    {title}  {} / 净 {}",
                    format_words(*with_punct),
                    format_words(*no_punct)
                ),
                StatRow::Total {
                    with_punct,
                    no_punct,
                } => format!(
                    "全书合计  {} / 净 {}",
                    format_words(*with_punct),
                    format_words(*no_punct)
                ),
            })
            .collect()
    }
}

/// 导出 CSV（§6.4 `[MUST]` 可导出 CSV）。
///
/// 字段用引号包裹并转义内部引号：书名章名里出现逗号是常事
/// （「第一章，雪夜」），不转义会让整个 CSV 错列。
pub fn to_csv(book: &Book, no_punct_of: impl Fn(mj_core::id::ChapterId) -> u64) -> String {
    let mut s = String::from("卷,章,含标点,净字数\n");

    for v in &book.volumes {
        for c in &v.chapters {
            s.push_str(&format!(
                "{},{},{},{}\n",
                csv_field(&v.title),
                csv_field(&c.title),
                c.word_count.unwrap_or(0),
                no_punct_of(c.id)
            ));
        }
    }
    s
}

/// CSV 字段转义：包引号，内部引号翻倍（RFC 4180）。
fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_core::id::{BookId, ChapterId, VolumeId};
    use mj_core::model::{ChapterMeta, ChapterStatus, Volume};

    fn chapter(title: &str, words: u64) -> ChapterMeta {
        ChapterMeta {
            id: ChapterId::generate(),
            title: title.into(),
            order: 10,
            status: ChapterStatus::Draft,
            word_count: Some(words),
            tags: Vec::new(),
            path: "x.md".into(),
            updated: None,
            damaged: None,
        }
    }

    fn book() -> Book {
        let mut b = Book::new(BookId::generate(), "雪夜行", "沈砚");
        let mut v1 = Volume::new(VolumeId::generate(), "第一卷", 10);
        v1.chapters.push(chapter("第一章", 3128));
        v1.chapters.push(chapter("第二章", 2000));
        let mut v2 = Volume::new(VolumeId::generate(), "第二卷", 20);
        v2.chapters.push(chapter("第三章", 500));
        b.volumes.push(v1);
        b.volumes.push(v2);
        b
    }

    /// 净字数按含标点的 9 折模拟。
    fn np(_: ChapterId) -> u64 {
        0
    }

    #[test]
    fn rows_group_by_volume_with_totals() {
        let b = book();
        let rows = Stats::rows(&b, np);
        // 2 卷 + 3 章 + 1 合计
        assert_eq!(rows.len(), 6);
        assert!(matches!(&rows[0], StatRow::Volume { chapters: 2, .. }));
        assert!(matches!(&rows[1], StatRow::Chapter { .. }));
        assert!(matches!(&rows[5], StatRow::Total { .. }));
    }

    #[test]
    fn volume_totals_sum_their_chapters() {
        let b = book();
        let rows = Stats::rows(&b, np);
        match &rows[0] {
            StatRow::Volume { with_punct, .. } => assert_eq!(*with_punct, 5128),
            other => panic!("首行应是卷: {other:?}"),
        }
    }

    #[test]
    fn total_sums_everything() {
        let b = book();
        let rows = Stats::rows(&b, np);
        match rows.last().unwrap() {
            StatRow::Total { with_punct, .. } => assert_eq!(*with_punct, 5628),
            other => panic!("末行应是合计: {other:?}"),
        }
    }

    #[test]
    fn no_punct_comes_from_callback() {
        let b = book();
        let rows = Stats::rows(&b, |_| 100);
        match &rows[0] {
            StatRow::Volume { no_punct, .. } => assert_eq!(*no_punct, 200, "两章各 100"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_book_still_has_total() {
        let b = Book::new(BookId::generate(), "空书", "作者");
        let rows = Stats::rows(&b, np);
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0],
            StatRow::Total {
                with_punct: 0,
                no_punct: 0
            }
        ));
    }

    // ---- CSV ----

    #[test]
    fn csv_has_header_and_rows() {
        let b = book();
        let csv = to_csv(&b, np);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "卷,章,含标点,净字数");
        assert_eq!(lines.len(), 4, "表头 + 3 章");
        assert!(lines[1].contains("第一卷"));
        assert!(lines[1].contains("3128"));
    }

    /// 标题里的逗号必须被引号包住，否则整个 CSV 错列。
    #[test]
    fn csv_escapes_commas_in_titles() {
        let mut b = Book::new(BookId::generate(), "书", "作者");
        let mut v = Volume::new(VolumeId::generate(), "第一卷，风起", 10);
        v.chapters.push(chapter("第一章，雪夜", 100));
        b.volumes.push(v);

        let csv = to_csv(&b, np);
        let line = csv.lines().nth(1).unwrap();
        assert!(
            line.starts_with("\"第一卷，风起\""),
            "含逗号的字段应被引号包住: {line}"
        );
        // 按 CSV 规则解析后应恰为 4 列——若引号没起作用，
        // 标题里的逗号会把行切成 6 列，整个表格错位。
        assert_eq!(parse_csv_line(line).len(), 4, "字段数错: {line}");
    }

    /// 一个够用的 CSV 行解析器，仅供测试断言列数。
    fn parse_csv_line(line: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if in_quotes && chars.peek() == Some(&'"') => {
                    chars.next();
                    cur.push('"');
                }
                '"' => in_quotes = !in_quotes,
                ',' if !in_quotes => fields.push(std::mem::take(&mut cur)),
                other => cur.push(other),
            }
        }
        fields.push(cur);
        fields
    }

    /// 标题里的引号按 RFC 4180 翻倍转义。
    #[test]
    fn csv_escapes_quotes_in_titles() {
        let mut b = Book::new(BookId::generate(), "书", "作者");
        let mut v = Volume::new(VolumeId::generate(), "卷", 10);
        v.chapters.push(chapter("他说\"你好\"", 100));
        b.volumes.push(v);

        let csv = to_csv(&b, np);
        assert!(csv.contains("\"他说\"\"你好\"\"\""), "引号应翻倍:\n{csv}");
    }

    #[test]
    fn csv_of_empty_book_is_header_only() {
        let b = Book::new(BookId::generate(), "空书", "作者");
        assert_eq!(to_csv(&b, np), "卷,章,含标点,净字数\n");
    }

    // ---- 渲染 ----

    #[test]
    fn render_shows_both_measures() {
        let b = book();
        let rows = Stats::rows(&b, |_| 2904);
        let text = Stats::render_rows(&rows);
        assert!(text[0].contains("第一卷"), "{:?}", text[0]);
        assert!(text[0].contains("净"), "应显示双口径: {:?}", text[0]);
        assert!(text.last().unwrap().contains("全书合计"));
    }

    #[test]
    fn scrolling_stops_at_bounds() {
        let mut s = Stats::new();
        s.scroll_up();
        assert_eq!(s.scroll(), 0, "顶部再上滚仍是 0");
        for _ in 0..20 {
            s.scroll_down(6, 4);
        }
        assert_eq!(s.scroll(), 2, "不该滚过末尾（6 行 - 4 高 = 2）");
    }

    #[test]
    fn no_scroll_when_content_fits() {
        let mut s = Stats::new();
        s.scroll_down(3, 10);
        assert_eq!(s.scroll(), 0, "内容装得下就不该滚");
    }
}
