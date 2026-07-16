//! 视口：软换行、虚拟化、光标屏幕定位。见 doc.md §6.3、§7.2。
//!
//! `[MUST]` 视口虚拟化：只渲染可见行，10 万字章节滚动不掉帧。
//! 故这里**不**预先折行整个文档——只折可见范围内的段落。

use crate::editor::Buffer;

/// 软换行续行的缩进列数。
///
/// 与中文段首的两个全角空格（4 列）对齐：这样续行的正文与段首的正文左边缘齐平，
/// 段落边界一眼可辨，不必靠「有没有缩进」去猜。
/// 代价是每行少 4 列可用宽度——换来的是读自己的稿子时不会把折行误读成新段。
pub const WRAP_INDENT: usize = 4;

/// 一条**显示行**（软换行后的行），指向缓冲里的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    /// 在缓冲中的字节区间（不含行尾换行符）。
    pub range: std::ops::Range<usize>,
    /// 所属逻辑行（段落）号。
    pub logical_line: usize,
    /// 是否是该段落的首个显示行。
    pub is_paragraph_start: bool,
    /// 渲染时该行左侧应补的空格列数。续行为 `WRAP_INDENT`，段首为 0
    /// （段首的缩进来自正文里真实存在的全角空格，不是渲染加的）。
    pub indent: usize,
}

/// 视口状态。
#[derive(Debug, Clone)]
pub struct Viewport {
    /// 顶部**逻辑行**号。不是显示行——理由见 `scroll_to_cursor`。
    top_logical: usize,
    height: usize,
    width: usize,
}

impl Viewport {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            top_logical: 0,
            height,
            width,
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn top_logical(&self) -> usize {
        self.top_logical
    }

    /// 生成可见的显示行。
    ///
    /// 只处理从 `top_logical` 起、够填满视口的那些段落——这就是虚拟化：
    /// 10 万字章节里也只折几十行，与文档长度无关。
    pub fn visible_lines(&self, buf: &Buffer) -> Vec<DisplayLine> {
        let rope = buf.text();
        let total_logical = rope.len_lines();
        let mut out = Vec::new();

        let mut logical = self.top_logical;
        while logical < total_logical && out.len() < self.height {
            let start = rope.line_to_byte(logical);
            let end = if logical + 1 < total_logical {
                rope.line_to_byte(logical + 1)
            } else {
                rope.len_bytes()
            };

            let para = line_text(buf, start..end);

            if para.is_empty() {
                // 空段落也占一行——否则空行被吞掉，段间距全乱。
                out.push(DisplayLine {
                    range: start..start,
                    logical_line: logical,
                    is_paragraph_start: true,
                    indent: 0,
                });
            } else {
                for (i, r) in self.wrap_paragraph(&para).into_iter().enumerate() {
                    if out.len() >= self.height {
                        break;
                    }
                    out.push(DisplayLine {
                        range: (start + r.start)..(start + r.end),
                        logical_line: logical,
                        is_paragraph_start: i == 0,
                        indent: if i == 0 { 0 } else { WRAP_INDENT },
                    });
                }
            }
            logical += 1;
        }
        out
    }

    /// 滚动使光标可见。
    ///
    /// 以**逻辑行**为单位滚动，而非显示行。理由：按显示行滚动需要知道光标之前
    /// 所有段落各折成了多少行——那就得折整个文档，虚拟化白做了。
    /// 代价是超长段落跨屏时略粗糙，换来的是 O(视口) 而非 O(全文)。
    pub fn scroll_to_cursor(&mut self, buf: &Buffer) {
        let cursor_logical = buf.cursor_line();

        if cursor_logical < self.top_logical {
            self.top_logical = cursor_logical;
            return;
        }

        // 往下滚：逐步推进顶部，直到光标所在段落进入可见范围。
        // 循环而非直接算，因为每段折成几行不定。
        let max_top = buf.text().len_lines().saturating_sub(1);
        while self.top_logical < max_top && !self.contains_logical(buf, cursor_logical) {
            self.top_logical += 1;
        }
    }

    /// 折一个段落：首行用满宽，续行让出 `WRAP_INDENT` 列。
    ///
    /// 不能直接调一次 `wrap_line`——它假设所有行等宽，而续行要缩进，可用宽度不同。
    /// 但也只需调**两次**：首行按满宽折出第一行，剩下的整体按续行宽度折一次。
    ///
    /// （最初写成「每折一行就把剩余文本重折一遍」，那对长段落是平方级的：
    /// 性能测试立刻从 <1ms 涨到 1.17ms 并报警。留此注记以免重蹈。）
    fn wrap_paragraph(&self, para: &str) -> Vec<std::ops::Range<usize>> {
        // 首行按满宽折，只取第一段。
        let Some(head) = mj_text::width::wrap_line(para, self.width).first().cloned() else {
            return Vec::new();
        };

        let mut out = vec![head.clone()];
        if head.end >= para.len() {
            return out; // 一行就装下了
        }

        // 剩余部分按续行宽度折一次，全部收下。
        let cont_width = self.width.saturating_sub(WRAP_INDENT).max(1);
        let Some(rest) = mj_text::width::slice(para, head.end..para.len()) else {
            return out;
        };
        for r in mj_text::width::wrap_line(rest, cont_width) {
            out.push((head.end + r.start)..(head.end + r.end));
        }
        out
    }

    fn contains_logical(&self, buf: &Buffer, logical: usize) -> bool {
        self.visible_lines(buf)
            .iter()
            .any(|l| l.logical_line == logical)
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.top_logical = self.top_logical.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize, buf: &Buffer) {
        let max = buf.text().len_lines().saturating_sub(1);
        self.top_logical = (self.top_logical + n).min(max);
    }

    /// 光标在屏幕上的位置 (列, 行)，相对视口左上角。不可见时返回 None。
    ///
    /// 折行点的归属是有歧义的：同一个字节偏移既是上一显示行的 end，
    /// 也是下一显示行的 start。此处**优先归下一行**——用户在行尾输入时，
    /// 期待光标出现在新行的开头，而不是吊在上一行末尾的行外。
    pub fn cursor_screen_pos(&self, buf: &Buffer) -> Option<(u16, u16)> {
        let cursor = buf.cursor();
        let lines = self.visible_lines(buf);

        // 先找严格包含光标的行（start <= cursor < end）。
        for (row, line) in lines.iter().enumerate() {
            if cursor >= line.range.start && cursor < line.range.end {
                return Some((self.col_of(buf, line, cursor), row as u16));
            }
        }
        // 再退而求其次：落在某行末尾（段落末尾的正常位置）。
        for (row, line) in lines.iter().enumerate() {
            if cursor == line.range.end {
                return Some((self.col_of(buf, line, cursor), row as u16));
            }
        }
        None
    }

    fn col_of(&self, buf: &Buffer, line: &DisplayLine, cursor: usize) -> u16 {
        let text = line_slice(buf, line.range.start..cursor);
        // 加上续行缩进，否则光标会比字符左移 4 列。
        (line.indent + mj_text::width::display_width(&text)) as u16
    }
}

/// 取一段的文本，去掉行尾换行符。
fn line_text(buf: &Buffer, range: std::ops::Range<usize>) -> String {
    let s = line_slice(buf, range);
    s.trim_end_matches('\n').to_owned()
}

/// 从缓冲里取一段字符串。
pub fn line_slice(buf: &Buffer, range: std::ops::Range<usize>) -> String {
    let rope = buf.text();
    let start = rope.byte_to_char(range.start);
    let end = rope.byte_to_char(range.end);
    rope.slice(start..end).to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::new(s, 500)
    }

    #[test]
    fn wraps_long_paragraph_by_width() {
        let b = buf("雪落了一夜他推开门");
        let vp = Viewport::new(6, 10); // 宽 6 = 3 个中文字
        let lines = vp.visible_lines(&b);
        assert!(lines.len() > 1, "长段落应被折行");
        assert!(lines[0].is_paragraph_start);
        assert!(!lines[1].is_paragraph_start, "折出的第二行不是段首");
        assert!(lines.iter().all(|l| l.logical_line == 0), "都属于同一段");
    }

    /// 续行缩进：段首不缩（它的缩进是正文里真实的全角空格），续行缩 4 列。
    #[test]
    fn continuation_lines_are_indented() {
        let b = buf("雪落了一夜他推开门风裹着雪灌进来");
        let vp = Viewport::new(12, 10);
        let lines = vp.visible_lines(&b);
        assert!(lines.len() > 1, "应折行");
        assert_eq!(lines[0].indent, 0, "段首不加渲染缩进");
        assert!(
            lines[1..].iter().all(|l| l.indent == WRAP_INDENT),
            "续行应缩进"
        );
    }

    /// 缩进后续行可用宽度变窄，故续行装的字比首行少。
    #[test]
    fn continuation_lines_are_narrower() {
        let b = buf("雪落了一夜他推开门风裹着雪灌进来冷得刺骨");
        let vp = Viewport::new(12, 10);
        let lines = vp.visible_lines(&b);
        let width_of = |i: usize| {
            let t = line_slice(&b, lines[i].range.clone());
            mj_text::width::display_width(&t)
        };
        assert!(width_of(0) <= 12);
        assert!(
            width_of(1) + WRAP_INDENT <= 12,
            "续行正文 + 缩进不得超过栏宽"
        );
    }

    /// 折行仍不得丢字——加了缩进逻辑后尤其要确认。
    #[test]
    fn wrap_with_indent_loses_no_text() {
        let text = "　　雪落了一夜。他推开门，风裹着雪灌进来，冷得刺骨。院里那株梅树已经压弯了腰。";
        let b = buf(text);
        for w in [8, 12, 20, 40] {
            let vp = Viewport::new(w, 50);
            let joined: String = vp
                .visible_lines(&b)
                .iter()
                .map(|l| line_slice(&b, l.range.clone()))
                .collect();
            assert_eq!(joined, text, "宽度 {w} 下丢字了");
        }
    }

    /// 光标在续行上时，屏幕列必须含缩进——否则光标与字符错开 4 列。
    #[test]
    fn cursor_on_continuation_line_accounts_for_indent() {
        let mut b = buf("雪落了一夜他推开门");
        let vp = Viewport::new(8, 10);
        let lines = vp.visible_lines(&b);
        assert!(lines.len() > 1);
        // 光标放在第二显示行的行首。
        b.move_to(lines[1].range.start);
        let (col, row) = vp.cursor_screen_pos(&b).unwrap();
        assert_eq!(row, 1);
        assert_eq!(col as usize, WRAP_INDENT, "续行行首的光标应在缩进之后");
    }

    #[test]
    fn empty_paragraph_occupies_one_line() {
        let b = buf("第一段\n\n第三段");
        let vp = Viewport::new(20, 10);
        let lines = vp.visible_lines(&b);
        assert_eq!(lines.len(), 3, "空行必须占一行，否则段间距会塌");
        assert_eq!(lines[1].range.start, lines[1].range.end, "中间是空行");
    }

    /// 虚拟化：只生成视口容得下的行数，与文档总长无关。
    #[test]
    fn only_renders_visible_lines() {
        let text = "　　雪落了一夜。\n".repeat(10_000);
        let b = buf(&text);
        let vp = Viewport::new(40, 25);
        let lines = vp.visible_lines(&b);
        assert!(lines.len() <= 25, "生成了 {} 行，超过视口高度", lines.len());
    }

    /// 10 万字章节生成可见行必须瞬时——这是「滚动不掉帧」的前提（§9 p99 < 16ms）。
    ///
    /// **只在 release 下断言时限**：§9 的性能预算说的是发布二进制。
    /// debug 构建下 grapheme 遍历慢 20 倍（实测 wrap_line 2µs → 44µs），
    /// 在 debug 里断言 release 的预算只会制造假警报——而假警报会让人
    /// 习惯性忽略性能测试，那比没有测试更糟。
    /// debug 下仍执行，只验证不 panic、不死循环。
    #[test]
    fn visible_lines_is_fast_on_large_chapter() {
        let text = "　　雪落了一夜，他推开门，风裹着雪灌进来。\n".repeat(5_000);
        let b = buf(&text);
        let vp = Viewport::new(40, 25);

        let t = std::time::Instant::now();
        for _ in 0..100 {
            let lines = vp.visible_lines(&b);
            assert!(lines.len() <= 25, "虚拟化失效");
        }
        let per_call = t.elapsed() / 100;

        if cfg!(not(debug_assertions)) {
            assert!(
                per_call < std::time::Duration::from_millis(1),
                "单次 {per_call:?}，10 万字章节下应 < 1ms（§9 按键到屏幕 p99 < 16ms）"
            );
        }
    }

    #[test]
    fn cursor_screen_pos_accounts_for_cjk_width() {
        let mut b = buf("雪落");
        b.move_to(3); // 「雪」之后
        let vp = Viewport::new(20, 10);
        assert_eq!(vp.cursor_screen_pos(&b), Some((2, 0)), "一个中文字宽 2 列");
    }

    #[test]
    fn cursor_screen_pos_on_second_line() {
        let mut b = buf("第一段\n第二段");
        b.move_to(10); // 第二段开头
        let vp = Viewport::new(20, 10);
        assert_eq!(vp.cursor_screen_pos(&b), Some((0, 1)));
    }

    #[test]
    fn scroll_to_cursor_follows_down() {
        let mut b = buf(&"行\n".repeat(100));
        b.move_to(b.len_bytes());
        let mut vp = Viewport::new(20, 10);
        vp.scroll_to_cursor(&b);
        assert!(vp.top_logical() > 0, "光标在文末时视口应已下滚");
        assert!(vp.cursor_screen_pos(&b).is_some(), "滚动后光标必须可见");
    }

    #[test]
    fn scroll_to_cursor_follows_up() {
        let mut b = buf(&"行\n".repeat(100));
        let mut vp = Viewport::new(20, 10);
        vp.scroll_down(50, &b);
        b.move_to(0);
        vp.scroll_to_cursor(&b);
        assert_eq!(vp.top_logical(), 0, "光标回到文首时应上滚到顶");
    }

    #[test]
    fn scroll_does_not_run_past_end() {
        let b = buf("一行");
        let mut vp = Viewport::new(20, 10);
        vp.scroll_down(999, &b);
        assert_eq!(vp.top_logical(), 0, "单行文档不应滚动");
    }

    #[test]
    fn narrow_width_does_not_panic() {
        let b = buf("　　雪落了一夜。他推开门。");
        for w in [1, 2, 3] {
            let vp = Viewport::new(w, 5);
            let _ = vp.visible_lines(&b);
        }
    }

    /// 可见行必须落在字符边界上——否则渲染会切出乱码。
    #[test]
    fn visible_ranges_are_on_boundaries() {
        let text = "　　雪落了一夜。👨‍👩‍👧他推开门。\n第二段";
        let b = buf(text);
        let vp = Viewport::new(8, 20);
        let s = b.contents();
        for l in vp.visible_lines(&b) {
            assert!(s.is_char_boundary(l.range.start), "start 不在边界");
            assert!(s.is_char_boundary(l.range.end), "end 不在边界");
        }
    }
}
