//! 编辑缓冲：ropey + grapheme 光标 + 撤销栈。见 doc.md §6.3。
//!
//! 这一层不含任何渲染，只管「文本怎么变」——故可被完整单元测试。
//! 视口与绘制在 `view.rs`。
//!
//! 三条 `[MUST]`（§6.3）：
//! - ropey 作缓冲，插入/删除不整段重建字符串；
//! - 光标按 grapheme 移动（§0 禁令 5）；
//! - 撤销栈按操作类型 + 时间间隔（默认 500ms）合并成组，栈深默认 500。

use std::time::{Duration, Instant};

use ropey::Rope;

/// 撤销组的合并间隔（§6.3 默认 500ms）。
const COALESCE_WINDOW: Duration = Duration::from_millis(500);

/// 一次可撤销的编辑。
#[derive(Debug, Clone, PartialEq)]
struct Change {
    /// 起始字节偏移。
    at: usize,
    /// 被删除的文本（撤销时要还原它）。
    removed: String,
    /// 插入的文本。
    inserted: String,
    /// 编辑前的光标位置，撤销时恢复到这里。
    cursor_before: usize,
}

/// 编辑的种类。相邻的同类编辑才可能合并成一个撤销组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Insert,
    Delete,
    /// 排版、批量替换等：**永不合并**，一次算一个撤销组（§6.3）。
    Bulk,
}

#[derive(Debug)]
struct UndoGroup {
    changes: Vec<Change>,
    kind: Kind,
    /// 组内最后一次编辑的时刻，用于判断是否还能继续合并。
    last_edit: Instant,
}

/// 文本缓冲。
pub struct Buffer {
    text: Rope,
    /// 光标的字节偏移。永远落在 grapheme 边界上。
    cursor: usize,
    /// 选区的另一端（锚点）。None = 无选区。
    ///
    /// 存锚点而非 (start, end)：用户可以从任一端往回选，
    /// 而「哪端是起点」正是 Shift+方向键要保留的信息。
    anchor: Option<usize>,
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    undo_depth: usize,
    /// 自上次保存以来是否有改动。
    dirty: bool,
    /// 自上次保存以来累计变更的字数，用于触发自动保存（§6.3）。
    changed_chars: usize,
}

impl Buffer {
    pub fn new(text: &str, undo_depth: usize) -> Self {
        Self {
            text: Rope::from_str(text),
            cursor: 0,
            anchor: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_depth: undo_depth.max(1),
            dirty: false,
            changed_chars: 0,
        }
    }

    // ---- 选区（§6.4：选中文本时状态栏切为「选中 N 字」）----

    /// 开始/延续选区：记下锚点（若尚无）。Shift+方向键调用。
    pub fn start_selection(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
    }

    /// 清除选区。普通方向键、插入等调用。
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// 当前选区的字节区间（有序）。无选区或空选区返回 None。
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let a = self.anchor?;
        let (lo, hi) = if a <= self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        };
        (lo < hi).then_some(lo..hi)
    }

    /// 选中的文本。
    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|r| self.slice_to_string(r))
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// 取出全文。
    ///
    /// 不叫 `to_string`：那会与 `ToString` trait 撞名，调用方无从分辨拿到的是哪个。
    /// 名字也该提醒代价——大章节下这是一次全文拷贝，别在热路径里调。
    pub fn contents(&self) -> String {
        self.text.to_string()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn changed_chars(&self) -> usize {
        self.changed_chars
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }

    /// 标记已保存。自动保存与手动保存后调用。
    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.changed_chars = 0;
    }

    /// 标记为「有未落盘的改动」。
    ///
    /// 专供崩溃恢复：从 swp 灌回来的内容与磁盘不一致，虽然用户没敲过键，
    /// 但它确实未保存——状态栏必须显示「未保存」，否则用户以为已经安全了。
    pub fn mark_dirty_for_recovery(&mut self) {
        self.dirty = true;
    }

    /// 取整个缓冲的字符串（用于保存/统计）。避免频繁调用——大章节开销不小。
    fn slice_to_string(&self, range: std::ops::Range<usize>) -> String {
        let start = self.text.byte_to_char(range.start);
        let end = self.text.byte_to_char(range.end);
        self.text.slice(start..end).to_string()
    }

    // ---- 编辑 ----

    /// 在光标处插入文本。
    pub fn insert(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.apply(
            Change {
                at: self.cursor,
                removed: String::new(),
                inserted: s.to_owned(),
                cursor_before: self.cursor,
            },
            Kind::Insert,
        );
    }

    /// 插入一对符号，光标停在两者之间（§6.3 中文输入辅助）。
    ///
    /// 与「插入两个字符再左移」不同：这是**一次**编辑，撤销时整对一起消失。
    /// 用户敲一次 `「`，撤销一次就该回到敲之前——而不是留下半个 `」`。
    pub fn insert_pair(&mut self, open: char, close: char) {
        let mut s = String::with_capacity(open.len_utf8() + close.len_utf8());
        s.push(open);
        s.push(close);
        self.insert(&s);
        // 光标退回到两者之间。
        self.cursor -= close.len_utf8();
    }

    /// 光标右侧的第一个字符（供成对符号判断上下文）。
    pub fn next_char(&self) -> Option<char> {
        let span = self.line_span(self.cursor);
        if self.cursor >= span.end {
            return None;
        }
        self.slice_to_string(self.cursor..span.end)
            .chars()
            .next()
            .filter(|c| *c != '\n')
    }

    /// 删除光标前的一个 grapheme（Backspace）。
    pub fn delete_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.prev_boundary(self.cursor);
        let removed = self.slice_to_string(prev..self.cursor);
        self.apply(
            Change {
                at: prev,
                removed,
                inserted: String::new(),
                cursor_before: self.cursor,
            },
            Kind::Delete,
        );
    }

    /// 删除光标后的一个 grapheme（Delete）。
    pub fn delete_forward(&mut self) {
        let next = self.next_boundary(self.cursor);
        if next == self.cursor {
            return;
        }
        let removed = self.slice_to_string(self.cursor..next);
        self.apply(
            Change {
                at: self.cursor,
                removed,
                inserted: String::new(),
                cursor_before: self.cursor,
            },
            Kind::Delete,
        );
    }

    /// 替换一个区间。用于排版/批量替换——**永不与相邻编辑合并**（§6.3）。
    pub fn replace_range(&mut self, range: std::ops::Range<usize>, with: &str) {
        let removed = self.slice_to_string(range.clone());
        self.apply(
            Change {
                at: range.start,
                removed,
                inserted: with.to_owned(),
                cursor_before: self.cursor,
            },
            Kind::Bulk,
        );
    }

    /// 一次性替换多个区间，**整体算一个撤销组**（§6.5：一次排版 = 一个撤销组）。
    ///
    /// `edits` 必须互不重叠（`format::plan` 已保证）。内部按起点倒序施加，
    /// 免得前面的改动让后面的偏移失效。
    ///
    /// 为什么不能循环调 `replace_range`：那会产生 N 个撤销组，用户排完版
    /// 得按 N 次 Ctrl+Z 才退得干净——而他心里那是**一次**操作。
    pub fn replace_ranges(&mut self, edits: &[(std::ops::Range<usize>, String)]) {
        if edits.is_empty() {
            return;
        }
        let cursor_before = self.cursor;

        // **按起点升序施加，并把后续偏移按已产生的长度差校正。**
        //
        // 这样每条 Change 的 `at` 都落在「它被施加时」的坐标系里——与打字产生的
        // Change 语义一致。撤销栈的回放依赖这条不变量：`undo` 倒着 revert
        // （高位先还原，低位的 at 才不会失效），`redo` 正着 edit。
        // 若改成倒序施加、`at` 记原文坐标，redo 就会正着重放一串倒序坐标，直接错乱。
        let mut sorted: Vec<_> = edits.iter().collect();
        sorted.sort_by_key(|(r, _)| r.start);

        let mut changes = Vec::with_capacity(sorted.len());
        let mut delta: isize = 0;

        for (range, with) in sorted {
            let at = (range.start as isize + delta).max(0) as usize;
            let end = (range.end as isize + delta).max(0) as usize;

            // 区间与当前文本对不上就跳过——排版失败顶多是没排上，
            // 不该把用户的正文搞坏（§0 禁令 1）。
            let ok = at <= end
                && end <= self.text.len_bytes()
                && self.text.try_byte_to_char(at).is_ok()
                && self.text.try_byte_to_char(end).is_ok();
            if !ok {
                tracing::warn!(?range, "排版编辑区间与正文对不上，跳过");
                continue;
            }

            let change = Change {
                at,
                removed: self.slice_to_string(at..end),
                inserted: with.clone(),
                cursor_before,
            };
            self.edit(&change);
            delta += with.len() as isize - (end - at) as isize;
            changes.push(change);
        }
        if changes.is_empty() {
            return;
        }

        // 一个组，装下全部改动。
        self.undo.push(UndoGroup {
            changes,
            kind: Kind::Bulk,
            last_edit: Instant::now(),
        });
        while self.undo.len() > self.undo_depth {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.anchor = None;
        // 光标可能落在被删掉的区间里，夹回合法位置。
        self.cursor = self.clamp_to_boundary(self.cursor.min(self.text.len_bytes()));
    }

    fn apply(&mut self, change: Change, kind: Kind) {
        self.edit(&change);
        self.push_undo(change, kind);
        self.redo.clear(); // 新编辑作废重做栈
        self.dirty = true;
        // 文本变了，选区的坐标随之失效——留着它只会高亮到错的地方。
        self.anchor = None;
    }

    /// 施加到 rope 上并移动光标。
    fn edit(&mut self, c: &Change) {
        let at_char = self.text.byte_to_char(c.at);
        if !c.removed.is_empty() {
            let end = self.text.byte_to_char(c.at + c.removed.len());
            self.text.remove(at_char..end);
        }
        if !c.inserted.is_empty() {
            self.text.insert(at_char, &c.inserted);
        }
        self.cursor = c.at + c.inserted.len();
        self.changed_chars += mj_text::width::grapheme_count(&c.inserted)
            .max(mj_text::width::grapheme_count(&c.removed));
    }

    fn push_undo(&mut self, change: Change, kind: Kind) {
        let now = Instant::now();

        // 能否并入上一组：同类、非 Bulk、且在合并窗口内。
        let can_coalesce = matches!(self.undo.last(), Some(g)
            if g.kind == kind
                && kind != Kind::Bulk
                && now.duration_since(g.last_edit) < COALESCE_WINDOW);

        if can_coalesce && let Some(g) = self.undo.last_mut() {
            g.changes.push(change);
            g.last_edit = now;
            return;
        }

        self.undo.push(UndoGroup {
            changes: vec![change],
            kind,
            last_edit: now,
        });

        // 栈深上限：淘汰最旧的。
        // 撤销栈与版本历史是两套机制（§6.3）——历史由快照负责，
        // 这里丢掉最旧的组不影响用户回退到昨天的稿子。
        while self.undo.len() > self.undo_depth {
            self.undo.remove(0);
        }
    }

    /// 撤销一组。
    pub fn undo(&mut self) -> bool {
        let Some(group) = self.undo.pop() else {
            return false;
        };
        // 逆序回放：后发生的先撤销。
        for c in group.changes.iter().rev() {
            self.revert(c);
        }
        if let Some(first) = group.changes.first() {
            self.cursor = first.cursor_before;
        }
        self.redo.push(group);
        self.dirty = true;
        true
    }

    /// 重做一组。
    pub fn redo(&mut self) -> bool {
        let Some(group) = self.redo.pop() else {
            return false;
        };
        for c in &group.changes {
            self.edit(c);
        }
        self.undo.push(group);
        self.dirty = true;
        true
    }

    /// 撤销单条：把 inserted 拿掉，把 removed 放回。
    fn revert(&mut self, c: &Change) {
        let at_char = self.text.byte_to_char(c.at);
        if !c.inserted.is_empty() {
            let end = self.text.byte_to_char(c.at + c.inserted.len());
            self.text.remove(at_char..end);
        }
        if !c.removed.is_empty() {
            self.text.insert(at_char, &c.removed);
        }
        self.cursor = c.at + c.removed.len();
    }

    // ---- 光标（一律按 grapheme，§0 禁令 5）----

    pub fn move_left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    pub fn move_to(&mut self, byte: usize) {
        self.cursor = self.clamp_to_boundary(byte);
    }

    pub fn move_home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// 当前光标所在的逻辑行号（0 起）。
    pub fn cursor_line(&self) -> usize {
        self.text.byte_to_line(self.cursor)
    }

    fn line_start(&self, byte: usize) -> usize {
        let line = self.text.byte_to_line(byte);
        self.text.line_to_byte(line)
    }

    fn line_end(&self, byte: usize) -> usize {
        let line = self.text.byte_to_line(byte);
        let next = self
            .text
            .line_to_byte((line + 1).min(self.text.len_lines()));
        // 去掉行尾换行符本身。
        let s = self.slice_to_string(self.line_start(byte)..next);
        next - (s.len() - s.trim_end_matches('\n').len())
    }

    /// 光标所在逻辑行的字节区间。
    ///
    /// grapheme 判定以**行**为窗口，而不是「光标附近 N 字节」：
    /// grapheme cluster 没有长度上限（ZWJ emoji 家族就有 18 字节，
    /// 旗帜、肤色修饰符还能更长），任何固定窗口都是在赌它够大——
    /// 赌输了就会切在 cluster 中间，offset 跑出边界并 panic。
    /// 而 grapheme 永远不跨行，所以行是天然安全的窗口。
    ///
    /// 代价：单行极长（如导入的未分段文本）时窗口也大。但那是 M3 `line_join`
    /// 要处理的问题，且换行本就是段落边界，不会退化成「整章一行」的常态。
    fn line_span(&self, byte: usize) -> std::ops::Range<usize> {
        let line = self.text.byte_to_line(byte);
        let start = self.text.line_to_byte(line);
        let end = if line + 1 < self.text.len_lines() {
            self.text.line_to_byte(line + 1)
        } else {
            self.text.len_bytes()
        };
        start..end
    }

    fn next_boundary(&self, byte: usize) -> usize {
        let span = self.line_span(byte);
        if byte >= span.end {
            return byte.min(self.text.len_bytes());
        }
        let line = self.slice_to_string(span.clone());
        let rel = byte - span.start;
        span.start + mj_text::width::next_grapheme_boundary(&line, rel)
    }

    fn prev_boundary(&self, byte: usize) -> usize {
        let span = self.line_span(byte);
        // 已在行首：退到上一行的行尾（跨过换行符）。
        if byte == span.start {
            return byte.saturating_sub(1).min(self.text.len_bytes());
        }
        let line = self.slice_to_string(span.clone());
        let rel = byte - span.start;
        span.start + mj_text::width::prev_grapheme_boundary(&line, rel)
    }

    /// 把任意字节位置向下取整到 **grapheme** 边界。
    ///
    /// 注意不能只用 `try_byte_to_char`——那只保证落在 *char* 边界上，
    /// 而组合字符 `e`+`U+0301` 的中间正是一个合法的 char 边界，
    /// 光标停在那里会把一个字看成两个（§0 禁令 5）。
    fn floor_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len_bytes());
        let span = self.line_span(byte);
        if byte <= span.start {
            return span.start;
        }
        let line = self.slice_to_string(span.clone());
        let rel = byte - span.start;

        // 从行首逐个 grapheme 前进，找到不超过 rel 的最大边界。
        let mut last = 0usize;
        for (off, g) in mj_text::width::grapheme_offsets(&line) {
            if off >= rel {
                break;
            }
            last = off;
            if off + g.len() > rel {
                break; // rel 落在这个 cluster 内部
            }
            last = off + g.len();
        }
        span.start + last
    }

    fn clamp_to_boundary(&self, byte: usize) -> usize {
        self.floor_boundary(byte.min(self.text.len_bytes()))
    }

    /// 该字节位置是否是 grapheme 边界。供测试断言不变量用。
    #[cfg(test)]
    fn is_char_boundary(&self, byte: usize) -> bool {
        byte == self.floor_boundary(byte)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::new(s, 500)
    }

    #[test]
    fn inserts_at_cursor() {
        let mut b = buf("");
        b.insert("雪");
        b.insert("落");
        assert_eq!(b.contents(), "雪落");
        assert_eq!(b.cursor(), 6, "光标应在两个中文字之后");
    }

    #[test]
    fn deletes_backward_by_grapheme() {
        let mut b = buf("");
        b.insert("雪落");
        b.delete_backward();
        assert_eq!(b.contents(), "雪", "应整字删除，不留半个字节");
    }

    /// 组合字符必须整体删除——否则会留下孤立的组合符，显示成乱码。
    #[test]
    fn deletes_combining_char_as_one() {
        let mut b = buf("");
        b.insert("e\u{301}");
        b.delete_backward();
        assert_eq!(b.contents(), "", "e+组合符应一次删净");
    }

    #[test]
    fn deletes_emoji_family_as_one() {
        let mut b = buf("");
        b.insert("👨‍👩‍👧");
        b.delete_backward();
        assert_eq!(b.contents(), "", "ZWJ 家族应一次删净");
    }

    #[test]
    fn delete_forward_works() {
        let mut b = buf("雪落");
        b.move_to(0);
        b.delete_forward();
        assert_eq!(b.contents(), "落");
    }

    #[test]
    fn deletes_at_edges_are_noops() {
        let mut b = buf("雪");
        b.move_to(0);
        b.delete_backward();
        assert_eq!(b.contents(), "雪", "开头 Backspace 应无操作");
        b.move_to(3);
        b.delete_forward();
        assert_eq!(b.contents(), "雪", "结尾 Delete 应无操作");
    }

    #[test]
    fn cursor_moves_by_grapheme() {
        let mut b = buf("雪a落");
        b.move_to(0);
        b.move_right();
        assert_eq!(b.cursor(), 3);
        b.move_right();
        assert_eq!(b.cursor(), 4);
        b.move_left();
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn cursor_saturates_at_ends() {
        let mut b = buf("雪");
        b.move_to(0);
        b.move_left();
        assert_eq!(b.cursor(), 0);
        b.move_to(3);
        b.move_right();
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn home_end_work_on_logical_line() {
        let mut b = buf("第一行\n第二行");
        b.move_to(12); // 第二行中间
        b.move_home();
        assert_eq!(b.cursor(), 10, "应到第二行行首");
        b.move_end();
        assert_eq!(b.cursor(), 19, "应到第二行行尾");
    }

    #[test]
    fn end_excludes_newline() {
        let mut b = buf("第一行\n第二行");
        b.move_to(0);
        b.move_end();
        assert_eq!(b.cursor(), 9, "行尾应在换行符之前");
    }

    // ---- 撤销 ----

    #[test]
    fn undo_restores_previous_text() {
        let mut b = buf("");
        b.insert("雪落了一夜");
        assert!(b.undo());
        assert_eq!(b.contents(), "");
    }

    #[test]
    fn redo_reapplies() {
        let mut b = buf("");
        b.insert("雪");
        b.undo();
        assert!(b.redo());
        assert_eq!(b.contents(), "雪");
    }

    #[test]
    fn undo_on_empty_stack_is_false() {
        let mut b = buf("雪");
        assert!(!b.undo(), "空栈撤销应返回 false 而非 panic");
        assert!(!b.redo());
    }

    /// 连续输入合并成一组：撤销一次应退掉整串，而不是一个字。
    #[test]
    fn consecutive_inserts_coalesce() {
        let mut b = buf("");
        b.insert("雪");
        b.insert("落");
        b.insert("了");
        assert!(b.undo());
        assert_eq!(b.contents(), "", "三次连续输入应合并为一组");
    }

    /// 插入与删除不同类，不得合并。
    #[test]
    fn insert_and_delete_do_not_coalesce() {
        let mut b = buf("");
        b.insert("雪落");
        b.delete_backward();
        assert_eq!(b.contents(), "雪");
        b.undo();
        assert_eq!(b.contents(), "雪落", "撤销应只退掉删除");
        b.undo();
        assert_eq!(b.contents(), "", "再撤销才退掉插入");
    }

    /// §6.3：排版/批量替换算**一个**撤销组，且永不与相邻编辑合并。
    #[test]
    fn bulk_edit_is_its_own_group() {
        let mut b = buf("雪落了");
        b.insert("x");
        b.replace_range(0..1, "Y");
        b.undo();
        assert!(b.contents().starts_with('x'), "批量编辑应独立成组");
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut b = buf("");
        b.insert("雪");
        b.undo();
        b.insert("落");
        assert!(!b.redo(), "新编辑后重做栈应清空");
    }

    /// 栈深上限：超出后丢最旧的，但不得 panic、不得丢当前文本。
    #[test]
    fn undo_depth_is_bounded() {
        let mut b = Buffer::new("", 3);
        for i in 0..10 {
            // 用 Bulk 保证每次都独立成组（Insert 会被合并）。
            let at = b.len_bytes();
            b.replace_range(at..at, &i.to_string());
        }
        assert_eq!(b.contents(), "0123456789");
        let mut count = 0;
        while b.undo() {
            count += 1;
        }
        assert_eq!(count, 3, "栈深应受限为 3");
    }

    #[test]
    fn dirty_flag_tracks_edits() {
        let mut b = buf("");
        assert!(!b.is_dirty());
        b.insert("雪");
        assert!(b.is_dirty());
        b.mark_saved();
        assert!(!b.is_dirty());
        assert_eq!(b.changed_chars(), 0);
    }

    /// 累计变更字数用于触发自动保存（§6.3：累计 200 字）。
    #[test]
    fn tracks_changed_chars_for_autosave() {
        let mut b = buf("");
        b.insert("雪落了一夜");
        assert_eq!(b.changed_chars(), 5);
        b.delete_backward();
        assert_eq!(b.changed_chars(), 6, "删除也算变更");
    }

    /// 撤销后文本必须逐字回到原样——含中文与组合字符。
    #[test]
    fn undo_roundtrip_preserves_cjk() {
        let original = "　　雪落了一夜。他推开门。";
        let mut b = buf(original);
        b.move_to(b.len_bytes());
        b.insert("风裹着雪灌进来。");
        b.undo();
        assert_eq!(b.contents(), original, "撤销后应逐字还原");
    }

    /// 光标永远落在 grapheme 边界——否则 rope 操作会 panic。
    #[test]
    fn cursor_always_on_boundary() {
        let mut b = buf("雪👨‍👩‍👧落");
        b.move_to(0);
        for _ in 0..10 {
            b.move_right();
            assert!(
                b.is_char_boundary(b.cursor()),
                "光标 {} 不在字符边界",
                b.cursor()
            );
        }
    }

    // ---- replace_ranges（排版/批量替换）----

    /// §6.5：一次排版 = 一个撤销组。
    #[test]
    fn replace_ranges_is_one_undo_group() {
        let mut b = buf("aXbXc");
        b.replace_ranges(&[(1..2, "1".into()), (3..4, "2".into())]);
        assert_eq!(b.contents(), "a1b2c");

        assert!(b.undo());
        assert_eq!(b.contents(), "aXbXc", "一次撤销应退掉全部改动");
        assert!(!b.undo(), "不该还有第二个组");
    }

    #[test]
    fn replace_ranges_redo_restores() {
        let mut b = buf("aXbXc");
        b.replace_ranges(&[(1..2, "1".into()), (3..4, "2".into())]);
        b.undo();
        assert!(b.redo());
        assert_eq!(b.contents(), "a1b2c", "重做应还原全部改动");
    }

    /// 改动长度不一时，后续区间要按已产生的长度差校正。
    #[test]
    fn replace_ranges_handles_length_changes() {
        let mut b = buf("aXbXc");
        // 第一处变长（1→3 字节），第二处的原文坐标 3..4 要相应右移。
        b.replace_ranges(&[(1..2, "111".into()), (3..4, "2".into())]);
        assert_eq!(b.contents(), "a111b2c");

        b.undo();
        assert_eq!(b.contents(), "aXbXc", "长度变化下撤销仍要逐字还原");
    }

    /// 删除（替换为空）同样要校正后续偏移。
    #[test]
    fn replace_ranges_handles_deletions() {
        let mut b = buf("a雪b雪c");
        let text = b.contents();
        let first = text.find('雪').unwrap();
        let second = text.rfind('雪').unwrap();
        b.replace_ranges(&[
            (first..first + 3, String::new()),
            (second..second + 3, "X".into()),
        ]);
        assert_eq!(b.contents(), "abXc");
        b.undo();
        assert_eq!(b.contents(), "a雪b雪c");
    }

    #[test]
    fn replace_ranges_with_cjk_roundtrips() {
        let original = "　　雪落了一夜。他推开门。";
        let mut b = buf(original);
        let pos = original.find('。').unwrap();
        b.replace_ranges(&[(pos..pos + 3, "……".into())]);
        assert!(b.contents().contains("……"));
        b.undo();
        assert_eq!(b.contents(), original, "中文正文必须逐字还原");
    }

    #[test]
    fn replace_ranges_empty_is_noop() {
        let mut b = buf("abc");
        b.replace_ranges(&[]);
        assert!(!b.is_dirty(), "空编辑不该弄脏缓冲");
        assert!(!b.undo(), "空编辑不该产生撤销组");
    }

    /// 区间对不上时跳过而非 panic——正文比排版重要。
    #[test]
    fn replace_ranges_skips_bad_ranges() {
        let mut b = buf("abc");
        b.replace_ranges(&[(100..200, "X".into())]);
        assert_eq!(b.contents(), "abc", "越界区间应被跳过");
    }

    /// 乱序传入也要正确——调用方不该被迫先排序。
    #[test]
    fn replace_ranges_accepts_unsorted_input() {
        let mut b = buf("aXbXc");
        b.replace_ranges(&[(3..4, "2".into()), (1..2, "1".into())]);
        assert_eq!(b.contents(), "a1b2c");
    }

    /// 与 format::plan 对接：plan 的输出喂进来，结果必须与 format 一致。
    #[test]
    fn replace_ranges_matches_format_output() {
        let text = "雪落了一夜...  \n\n\n\n他推开门,风灌进来!  \n";
        let opts = mj_text::format::FormatOptions::default();
        let edits: Vec<_> = mj_text::format::plan(text, &opts)
            .into_iter()
            .map(|e| (e.range, e.new))
            .collect();

        let mut b = buf(text);
        b.replace_ranges(&edits);
        assert_eq!(
            b.contents(),
            mj_text::format::format(text, &opts),
            "缓冲里的结果必须与纯函数排版一致"
        );

        b.undo();
        assert_eq!(b.contents(), text, "撤销应完整回到排版前");
    }

    // ---- 选区 ----

    #[test]
    fn selection_spans_from_anchor_to_cursor() {
        let mut b = buf("雪落了一夜");
        b.move_to(0);
        b.start_selection();
        b.move_right();
        b.move_right();
        assert_eq!(b.selected_text().as_deref(), Some("雪落"));
    }

    /// 从右往左选同样成立——用户会两个方向都用。
    #[test]
    fn selection_works_backwards() {
        let mut b = buf("雪落了一夜");
        b.move_to(6); // 「了」之前
        b.start_selection();
        b.move_left();
        b.move_left();
        assert_eq!(b.selected_text().as_deref(), Some("雪落"));
    }

    #[test]
    fn no_selection_without_anchor() {
        let mut b = buf("雪落");
        b.move_right();
        assert!(b.selection().is_none());
        assert!(b.selected_text().is_none());
    }

    /// 锚点与光标重合 = 空选区，应视为无选区（否则状态栏会显示「选中 0 字」）。
    #[test]
    fn empty_selection_is_none() {
        let mut b = buf("雪落");
        b.move_to(0);
        b.start_selection();
        assert!(b.selection().is_none(), "未移动光标时不该有选区");
    }

    #[test]
    fn start_selection_is_idempotent() {
        let mut b = buf("雪落了");
        b.move_to(0);
        b.start_selection();
        b.move_right();
        b.start_selection(); // 再次调用不该重置锚点
        b.move_right();
        assert_eq!(
            b.selected_text().as_deref(),
            Some("雪落"),
            "锚点应保持在起点"
        );
    }

    #[test]
    fn clear_selection_removes_it() {
        let mut b = buf("雪落");
        b.move_to(0);
        b.start_selection();
        b.move_right();
        b.clear_selection();
        assert!(b.selection().is_none());
    }

    /// 编辑后选区必须失效——文本变了，旧坐标会高亮到错的地方。
    #[test]
    fn editing_clears_selection() {
        let mut b = buf("雪落");
        b.move_to(0);
        b.start_selection();
        b.move_right();
        assert!(b.selection().is_some());
        b.insert("新");
        assert!(b.selection().is_none(), "编辑后选区应失效");
    }

    /// 选区按 grapheme 对齐：不该选中半个 emoji。
    #[test]
    fn selection_respects_graphemes() {
        let mut b = buf("👨‍👩‍👧雪");
        b.move_to(0);
        b.start_selection();
        b.move_right();
        assert_eq!(
            b.selected_text().as_deref(),
            Some("👨‍👩‍👧"),
            "应整体选中 ZWJ 家族"
        );
    }

    #[test]
    fn move_to_snaps_to_boundary() {
        let mut b = buf("雪落");
        b.move_to(1); // 「雪」的中间
        assert_eq!(b.cursor(), 0, "应向下取整到边界");
    }
}
