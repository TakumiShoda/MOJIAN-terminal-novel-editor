//! 目录树：书 → 卷 → 章。见 doc.md §6.2、§7.2。
//!
//! 本模块只管**树的状态与导航**（展开/折叠/选中/多选），不含渲染，故可完整测试。

use std::collections::HashSet;

use mj_core::id::{ChapterId, VolumeId};
use mj_core::model::{Book, ChapterStatus};

/// 树上的一行。
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Volume {
        id: VolumeId,
        title: String,
        expanded: bool,
        chapter_count: usize,
    },
    Chapter {
        id: ChapterId,
        volume: VolumeId,
        title: String,
        status: ChapterStatus,
        words: u64,
        /// front matter 损坏（见 ADR 0004）：树上要看得见，但不可编辑。
        damaged: bool,
    },
}

impl Row {
    pub fn is_chapter(&self) -> bool {
        matches!(self, Row::Chapter { .. })
    }
}

/// 树的状态。
#[derive(Debug, Default)]
pub struct Tree {
    /// 折叠的卷。默认全展开，故记录「折叠的」而非「展开的」。
    collapsed: HashSet<VolumeId>,
    /// 当前高亮行。
    cursor: usize,
    /// 勾选的章（§6.2 [MUST] 多选批量操作）。
    checked: HashSet<ChapterId>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把书压平成可见行。折叠的卷不展开其章节。
    pub fn rows(&self, book: &Book) -> Vec<Row> {
        let mut out = Vec::new();
        for v in &book.volumes {
            let expanded = !self.collapsed.contains(&v.id);
            out.push(Row::Volume {
                id: v.id,
                title: v.title.clone(),
                expanded,
                chapter_count: v.chapters.len(),
            });
            if expanded {
                for c in &v.chapters {
                    out.push(Row::Chapter {
                        id: c.id,
                        volume: v.id,
                        title: c.title.clone(),
                        status: c.status,
                        words: c.word_count.unwrap_or(0),
                        damaged: c.damaged.is_some(),
                    });
                }
            }
        }
        out
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 移动高亮。越界时停在两端而非回绕——回绕会让用户在长目录里迷失。
    pub fn move_down(&mut self, book: &Book) {
        let n = self.rows(book).len();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
    }

    /// 直接把选中挪到第 `i` 行（鼠标点击用）。越界贴到最后一行。
    pub fn set_cursor(&mut self, i: usize, book: &Book) {
        let n = self.rows(book).len();
        self.cursor = i.min(n.saturating_sub(1));
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// 折叠后高亮可能落在不存在的行上，需夹回范围内。
    fn clamp(&mut self, book: &Book) {
        let n = self.rows(book).len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    pub fn selected(&self, book: &Book) -> Option<Row> {
        self.rows(book).get(self.cursor).cloned()
    }

    /// 当前高亮的章（高亮在卷上时返回 None）。
    pub fn selected_chapter(&self, book: &Book) -> Option<ChapterId> {
        match self.selected(book) {
            Some(Row::Chapter { id, .. }) => Some(id),
            _ => None,
        }
    }

    /// 展开/折叠当前卷。高亮在章上时折叠其所属卷并跳到卷首——
    /// 否则折叠后高亮会指向一个已隐藏的行。
    pub fn toggle(&mut self, book: &Book) {
        match self.selected(book) {
            Some(Row::Volume { id, .. }) => {
                if !self.collapsed.remove(&id) {
                    self.collapsed.insert(id);
                }
            }
            Some(Row::Chapter { volume, .. }) => {
                self.collapsed.insert(volume);
                // 跳到该卷的行上。
                if let Some(i) = self
                    .rows(book)
                    .iter()
                    .position(|r| matches!(r, Row::Volume { id, .. } if *id == volume))
                {
                    self.cursor = i;
                }
            }
            None => {}
        }
        self.clamp(book);
    }

    /// 勾选/取消当前章（§6.2 Space 多选）。
    pub fn toggle_check(&mut self, book: &Book) {
        if let Some(Row::Chapter { id, .. }) = self.selected(book)
            && !self.checked.remove(&id)
        {
            self.checked.insert(id);
        }
    }

    pub fn is_checked(&self, id: ChapterId) -> bool {
        self.checked.contains(&id)
    }

    pub fn checked(&self) -> &HashSet<ChapterId> {
        &self.checked
    }

    pub fn clear_checks(&mut self) {
        self.checked.clear();
    }

    /// 勾选章节的总字数（§6.2 [MUST] 统计选中字数）。
    pub fn checked_words(&self, book: &Book) -> u64 {
        book.volumes
            .iter()
            .flat_map(|v| &v.chapters)
            .filter(|c| self.checked.contains(&c.id))
            .map(|c| c.word_count.unwrap_or(0))
            .sum()
    }

    /// 把高亮移到指定章上（打开章节后同步树的位置）。
    pub fn focus_chapter(&mut self, book: &Book, ch: ChapterId) {
        // 章可能在折叠的卷里——先展开。
        if let Some((v, _)) = book.find_chapter(ch) {
            self.collapsed.remove(&v.id);
        }
        if let Some(i) = self
            .rows(book)
            .iter()
            .position(|r| matches!(r, Row::Chapter { id, .. } if *id == ch))
        {
            self.cursor = i;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_core::id::BookId;
    use mj_core::model::{ChapterMeta, Volume};

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

    #[test]
    fn flattens_book_into_rows() {
        let b = book();
        let t = Tree::new();
        let rows = t.rows(&b);
        // 2 卷 + 3 章
        assert_eq!(rows.len(), 5);
        assert!(matches!(rows[0], Row::Volume { .. }));
        assert!(matches!(rows[1], Row::Chapter { .. }));
        assert!(matches!(rows[3], Row::Volume { .. }));
    }

    #[test]
    fn collapse_hides_chapters() {
        let b = book();
        let mut t = Tree::new();
        t.toggle(&b); // 折叠第一卷
        let rows = t.rows(&b);
        assert_eq!(rows.len(), 3, "第一卷的两章应被隐藏");
        assert!(matches!(
            &rows[0],
            Row::Volume {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn collapse_then_expand_restores() {
        let b = book();
        let mut t = Tree::new();
        t.toggle(&b);
        t.toggle(&b);
        assert_eq!(t.rows(&b).len(), 5);
    }

    /// 折叠时高亮在章上：应折叠其所属卷并把高亮移到卷行，
    /// 否则高亮会指向一个已经隐藏的行。
    #[test]
    fn collapsing_from_chapter_moves_cursor_to_volume() {
        let b = book();
        let mut t = Tree::new();
        t.move_down(&b); // 第一章
        assert!(t.selected(&b).unwrap().is_chapter());
        t.toggle(&b);
        assert!(
            matches!(t.selected(&b), Some(Row::Volume { .. })),
            "高亮应落在卷上"
        );
    }

    #[test]
    fn cursor_stops_at_ends() {
        let b = book();
        let mut t = Tree::new();
        t.move_up();
        assert_eq!(t.cursor(), 0, "顶部再上移仍是 0");
        for _ in 0..20 {
            t.move_down(&b);
        }
        assert_eq!(t.cursor(), 4, "底部再下移仍是末行");
    }

    #[test]
    fn selected_chapter_is_none_on_volume_row() {
        let b = book();
        let t = Tree::new();
        assert!(t.selected_chapter(&b).is_none(), "高亮在卷上应返回 None");
    }

    #[test]
    fn checks_accumulate_and_sum_words() {
        let b = book();
        let mut t = Tree::new();
        t.move_down(&b); // 第一章 3128
        t.toggle_check(&b);
        t.move_down(&b); // 第二章 2000
        t.toggle_check(&b);
        assert_eq!(t.checked().len(), 2);
        assert_eq!(t.checked_words(&b), 5128);
    }

    #[test]
    fn toggle_check_unchecks() {
        let b = book();
        let mut t = Tree::new();
        t.move_down(&b);
        t.toggle_check(&b);
        t.toggle_check(&b);
        assert!(t.checked().is_empty());
    }

    #[test]
    fn checking_a_volume_row_is_noop() {
        let b = book();
        let mut t = Tree::new();
        t.toggle_check(&b); // 高亮在卷上
        assert!(t.checked().is_empty(), "卷不可勾选");
    }

    #[test]
    fn focus_chapter_expands_collapsed_volume() {
        let b = book();
        let target = b.volumes[0].chapters[1].id;
        let mut t = Tree::new();
        t.toggle(&b); // 折叠第一卷
        t.focus_chapter(&b, target);
        assert_eq!(t.selected_chapter(&b), Some(target), "应展开并定位到该章");
    }

    #[test]
    fn damaged_chapter_is_visible_and_flagged() {
        let mut b = book();
        b.volumes[0].chapters[0].damaged = Some("front matter 坏了".into());
        let t = Tree::new();
        let rows = t.rows(&b);
        assert!(
            matches!(&rows[1], Row::Chapter { damaged: true, .. }),
            "受损章必须在树上可见并标记（ADR 0004）"
        );
    }

    #[test]
    fn empty_book_has_no_rows() {
        let b = Book::new(BookId::generate(), "空书", "作者");
        let t = Tree::new();
        assert!(t.rows(&b).is_empty());
        assert!(t.selected(&b).is_none(), "空树选中应返回 None 而非 panic");
    }
}
