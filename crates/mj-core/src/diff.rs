//! 两级 diff。见 doc.md §6.9。
//!
//! **先按段落（行）做 Myers 找出变动块，再对变动块内部做字符级 diff。**
//! 直接对十万字做字符级 diff 会慢且噪声大——噪声尤其致命：中文里
//! 「他推开门」改成「她推开门」，字符级 diff 会把全段拆成一堆碎片，
//! 用户看不出改的是那一个字。
//!
//! `[MUST]` 中文 diff 按 **grapheme** 而非 byte 切分，否则会切出乱码。

use unicode_segmentation::UnicodeSegmentation;

/// 行内的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    Equal(String),
    Insert(String),
    Delete(String),
}

impl DiffOp {
    pub fn text(&self) -> &str {
        match self {
            Self::Equal(s) | Self::Insert(s) | Self::Delete(s) => s,
        }
    }
}

/// 一个变动块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// 旧文中的**行**区间。
    pub old_lines: std::ops::Range<usize>,
    /// 新文中的**行**区间。
    pub new_lines: std::ops::Range<usize>,
    /// 旧文的字节区间——「单块恢复」要靠它把旧内容贴回去（§6.9）。
    pub old_range: std::ops::Range<usize>,
    pub new_range: std::ops::Range<usize>,
    /// 块内的字符级差异。仅当新旧都只有内容改动（非纯增/纯删）时才细分。
    pub ops: Vec<DiffOp>,
    pub kind: HunkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Insert,
    Delete,
    Replace,
}

/// 整篇差异的摘要（§6.9 顶部摘要：`+312 字 / -87 字 / 3 处改动`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffSummary {
    pub added: usize,
    pub removed: usize,
    pub hunks: usize,
}

/// 两级 diff。
pub fn diff(old: &str, new: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // 行首字节偏移，供把行号换回字节区间（「单块恢复」要用）。
    let old_offsets = line_offsets(old);
    let new_offsets = line_offsets(new);

    let mut out = Vec::new();
    // 第一级：按行做 Myers。
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &old_lines, &new_lines) {
        use similar::DiffTag::*;
        let (tag, ol, nl) = op.as_tag_tuple();
        if tag == Equal {
            continue;
        }

        let kind = match tag {
            Insert => HunkKind::Insert,
            Delete => HunkKind::Delete,
            _ => HunkKind::Replace,
        };

        // 第二级：只对「替换」块做字符级细分。
        //
        // 纯增/纯删没有可对照的另一半，细分只会把整段拆成一堆 Insert 碎片，
        // 反而不如整段显示清楚。
        let ops = if kind == HunkKind::Replace {
            let o = join_lines(&old_lines[ol.clone()]);
            let n = join_lines(&new_lines[nl.clone()]);
            char_diff(&o, &n)
        } else {
            Vec::new()
        };

        out.push(DiffHunk {
            old_range: byte_span(&old_offsets, &ol, old.len()),
            new_range: byte_span(&new_offsets, &nl, new.len()),
            old_lines: ol,
            new_lines: nl,
            ops,
            kind,
        });
    }
    out
}

/// 差异摘要。字数按「含标点」口径——与状态栏同源，用户不必换算。
pub fn summarize(hunks: &[DiffHunk], old: &str, new: &str) -> DiffSummary {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut s = DiffSummary {
        hunks: hunks.len(),
        ..Default::default()
    };
    for h in hunks {
        let o = join_lines(&old_lines[h.old_lines.clone()]);
        let n = join_lines(&new_lines[h.new_lines.clone()]);
        s.removed += mj_text::count::count_with_punct(&o);
        s.added += mj_text::count::count_with_punct(&n);
    }
    s
}

/// 字符级 diff，按 **grapheme** 切分（§6.9 `[MUST]`）。
///
/// 按 char 会把 `👨‍👩‍👧` 拆成好几段，按 byte 更是直接切出乱码。
pub fn char_diff(old: &str, new: &str) -> Vec<DiffOp> {
    let o: Vec<&str> = old.graphemes(true).collect();
    let n: Vec<&str> = new.graphemes(true).collect();

    let mut out: Vec<DiffOp> = Vec::new();
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &o, &n) {
        use similar::DiffTag::*;
        let (tag, orange, nrange) = op.as_tag_tuple();
        match tag {
            Equal => push_op(&mut out, DiffOp::Equal(o[orange].concat())),
            Delete => push_op(&mut out, DiffOp::Delete(o[orange].concat())),
            Insert => push_op(&mut out, DiffOp::Insert(n[nrange].concat())),
            Replace => {
                push_op(&mut out, DiffOp::Delete(o[orange].concat()));
                push_op(&mut out, DiffOp::Insert(n[nrange].concat()));
            }
        }
    }
    out
}

/// 合并相邻的同类段，免得输出一串碎片。
fn push_op(out: &mut Vec<DiffOp>, op: DiffOp) {
    if op.text().is_empty() {
        return;
    }
    match (out.last_mut(), &op) {
        (Some(DiffOp::Equal(a)), DiffOp::Equal(b)) => a.push_str(b),
        (Some(DiffOp::Insert(a)), DiffOp::Insert(b)) => a.push_str(b),
        (Some(DiffOp::Delete(a)), DiffOp::Delete(b)) => a.push_str(b),
        _ => out.push(op),
    }
}

/// 每行的起始字节偏移，外加末尾哨兵。
fn line_offsets(text: &str) -> Vec<usize> {
    let mut out = vec![0usize];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            out.push(i + 1);
        }
    }
    out
}

/// 行区间 → 字节区间。
fn byte_span(
    offsets: &[usize],
    lines: &std::ops::Range<usize>,
    total: usize,
) -> std::ops::Range<usize> {
    let start = offsets.get(lines.start).copied().unwrap_or(total);
    let end = offsets.get(lines.end).copied().unwrap_or(total);
    start..end
}

fn join_lines(lines: &[&str]) -> String {
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    #![allow(clippy::string_slice)] // 区间来自 diff，构造即保证落在边界上

    use super::*;

    #[test]
    fn identical_text_has_no_hunks() {
        let t = "　　雪落了一夜。\n　　他推开门。";
        assert!(diff(t, t).is_empty());
    }

    #[test]
    fn detects_replaced_line() {
        let old = "　　雪落了一夜。\n　　他推开门。";
        let new = "　　雪落了一夜。\n　　她推开门。";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Replace);
        assert_eq!(hunks[0].old_lines, 1..2, "只有第二行变了");
    }

    /// 两级 diff 的价值：改一个字，字符级要能指出**就是那一个字**。
    #[test]
    fn char_level_pinpoints_the_changed_character() {
        let old = "　　他推开门，风裹着雪灌进来。";
        let new = "　　她推开门，风裹着雪灌进来。";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 1);

        let deletes: Vec<&str> = hunks[0]
            .ops
            .iter()
            .filter_map(|o| match o {
                DiffOp::Delete(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deletes, ["他"], "应精确指出改的是「他」");
    }

    #[test]
    fn detects_insertion() {
        let old = "第一行";
        let new = "第一行\n第二行";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Insert);
    }

    #[test]
    fn detects_deletion() {
        let old = "第一行\n第二行";
        let new = "第一行";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Delete);
    }

    /// 纯增/纯删不做字符级细分——没有可对照的另一半，拆开只会更碎。
    #[test]
    fn pure_insert_has_no_char_ops() {
        let hunks = diff("a", "a\nb");
        assert!(hunks[0].ops.is_empty());
    }

    /// §6.9 [MUST]：按 grapheme 切分，不能切出乱码。
    #[test]
    fn char_diff_respects_graphemes() {
        let ops = char_diff("雪👨‍👩‍👧落", "雪落");
        let deleted: String = ops
            .iter()
            .filter_map(|o| match o {
                DiffOp::Delete(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deleted, "👨‍👩‍👧", "ZWJ 家族应整体删除，不能拆碎");
    }

    #[test]
    fn char_diff_handles_combining_marks() {
        let ops = char_diff("e\u{301}x", "x");
        let deleted: String = ops
            .iter()
            .filter_map(|o| match o {
                DiffOp::Delete(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deleted, "e\u{301}", "组合字符应整体处理");
    }

    /// 相邻同类段要合并，否则输出一串碎片。
    #[test]
    fn adjacent_ops_are_merged() {
        let ops = char_diff("abc", "xyz");
        let deletes = ops
            .iter()
            .filter(|o| matches!(o, DiffOp::Delete(_)))
            .count();
        assert_eq!(deletes, 1, "三个删除应合成一段，实得 {ops:?}");
    }

    /// 字节区间要能把旧内容原样取出来——「单块恢复」全靠它。
    #[test]
    fn old_range_extracts_the_original_block() {
        let old = "第一行\n第二行\n第三行";
        let new = "第一行\n改过的\n第三行";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 1);

        let block = &old[hunks[0].old_range.clone()];
        assert!(block.contains("第二行"), "取出的应是变动的那段: {block:?}");
        assert!(!block.contains("第一行"), "不该带上没变的行");
    }

    #[test]
    fn ranges_are_on_char_boundaries() {
        let old = "　　雪落了一夜。\n　　他推开门。";
        let new = "　　雪落了一夜。\n　　她推开门。";
        for h in diff(old, new) {
            assert!(old.is_char_boundary(h.old_range.start));
            assert!(old.is_char_boundary(h.old_range.end));
            assert!(new.is_char_boundary(h.new_range.start));
            assert!(new.is_char_boundary(h.new_range.end));
        }
    }

    #[test]
    fn summary_counts_added_and_removed() {
        let old = "雪落了。";
        let new = "雪落了一夜。";
        let hunks = diff(old, new);
        let s = summarize(&hunks, old, new);
        assert_eq!(s.hunks, 1);
        assert_eq!(s.removed, 4, "雪落了。= 4 字");
        assert_eq!(s.added, 6, "雪落了一夜。= 6 字");
    }

    #[test]
    fn multiple_hunks_are_reported_separately() {
        let old = "A\n同\nB\n同\nC";
        let new = "X\n同\nY\n同\nZ";
        let hunks = diff(old, new);
        assert_eq!(hunks.len(), 3, "三处不相邻的改动应是三个块");
    }

    #[test]
    fn empty_to_content() {
        let hunks = diff("", "雪落了");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Insert);
    }

    #[test]
    fn content_to_empty() {
        let hunks = diff("雪落了", "");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Delete);
    }

    #[test]
    fn both_empty() {
        assert!(diff("", "").is_empty());
    }

    /// §6.9 性能预算：5 万字章节做整章 diff，渲染 < 500ms。
    /// 这里只测 diff 本身；release 下才断言时限（debug 慢一个数量级，
    /// 在 debug 里断 release 的预算只会制造假警报）。
    #[test]
    fn diff_of_large_chapter_is_fast() {
        let para = "　　雪落了一夜，他推开门，风裹着雪灌进来。\n";
        let old = para.repeat(2500); // 约 5 万字
        // 改动散布在全篇。
        let new = old.replace("他推开门", "她推开门");

        let t = std::time::Instant::now();
        let hunks = diff(&old, &new);
        let elapsed = t.elapsed();

        assert!(!hunks.is_empty());
        if cfg!(not(debug_assertions)) {
            assert!(
                elapsed < std::time::Duration::from_millis(300),
                "5 万字 diff 用了 {elapsed:?}，§6.9 预算是渲染 < 500ms"
            );
        }
    }
}
