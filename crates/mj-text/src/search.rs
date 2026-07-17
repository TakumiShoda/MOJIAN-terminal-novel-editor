//! 查找替换。见 doc.md §6.6。
//!
//! # 全半角折叠为什么不能用 NFKC
//!
//! §6.6 点名了这个坑：NFKC 归一化会改变字节长度（`Ａ` 3 字节 → `A` 1 字节），
//! 命中位置就映射不回原文了。文档给的做法是「自建折叠表（一对一映射），
//! 在**逐字符比较层**折叠，位置天然对齐」。
//!
//! 这里照办，但补上文档略过的一步：即便是一对一的**字符**映射，字节长度仍然会变。
//! 故折叠时同步记下「折叠后的第 i 个字符原本在哪个字节」，匹配完照表回查。
//! 折叠表只收 1:1 的映射——`ﬁ → fi` 这种一对多会让字符对不上号，一律不收。
//!
//! 原文自始至终不被改写：折叠只发生在一份临时副本上，命中范围永远是原文坐标。

use std::ops::Range;

use crate::format::Edit;

/// 匹配模式（§6.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// 普通：逐字符比较（可折叠）。
    #[default]
    Literal,
    /// 全词：命中处两侧不得是同类字符。
    WholeWord,
    /// 正则（regex crate；`extended` 时切 fancy-regex 以支持 lookaround）。
    Regex,
}

/// 匹配选项（§6.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatchFlags {
    pub ignore_case: bool,
    /// 忽略全半角差异（`Ａ` 与 `A`、`１` 与 `1`）。
    pub fold_width: bool,
    /// 忽略中文标点差异（`，` 与 `,`）。
    pub fold_cjk_punct: bool,
    /// 扩展语法：切 fancy-regex 以支持 lookaround。仅 Regex 模式有意义。
    pub extended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub pattern: String,
    pub mode: MatchMode,
    pub flags: MatchFlags,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// 非法正则。§6.6 `[MUST]`：实时提示，**不得 panic**。
    #[error("正则表达式无效：{0}")]
    BadRegex(String),
    #[error("查找内容为空")]
    EmptyPattern,
}

/// 在文本中查找，返回**原文**的字节区间。
pub fn search_text(text: &str, q: &Query) -> Result<Vec<Range<usize>>, SearchError> {
    if q.pattern.is_empty() {
        return Err(SearchError::EmptyPattern);
    }
    match q.mode {
        MatchMode::Literal => Ok(search_literal(text, q, false)),
        MatchMode::WholeWord => Ok(search_literal(text, q, true)),
        MatchMode::Regex => search_regex(text, q),
    }
}

/// 替换预览：返回编辑列表而非新字符串——与排版同理，为了预览与逐条取消。
///
/// 正则模式下 `to` 支持 `$1` 捕获组引用（§6.6 `[MUST]`）。
pub fn replace_preview(text: &str, q: &Query, to: &str) -> Result<Vec<Edit>, SearchError> {
    if q.mode == MatchMode::Regex {
        return replace_regex(text, q, to);
    }
    Ok(search_text(text, q)?
        .into_iter()
        .map(|range| Edit {
            range,
            new: to.to_string(),
            rule: "replace",
        })
        .collect())
}

/// 一次命中的上下文（§6.6 结果面板：前后各 15 字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitContext {
    /// 命中在原文中的字节区间。
    pub range: Range<usize>,
    /// 行号（1 起）。
    pub line: usize,
    /// 前后各 15 字的上下文。
    pub context: String,
    /// 命中在 `context` 中的字节区间，供高亮。
    pub highlight: Range<usize>,
}

/// 上下文取前后各多少字（§6.6）。
const CONTEXT_CHARS: usize = 15;

/// 为一次命中取上下文。
///
/// 上下文与高亮区间**一起构造**：换行要显形成 `⏎`（否则一条结果撑成好几行），
/// 而 `⏎` 是 3 字节、`\n` 是 1 字节——先切片再替换的话，按原文算出的高亮区间
/// 就会落进 `⏎` 的字节中间。
///
/// 这正是 §6.6 警告 NFKC 的那个坑：**换字符就会改字节长度，位置就映射不回去**。
/// 折叠表那边小心避开了，这里却差点栽进去（proptest 用 `"\nA"` 抓到）。
/// 故边拼边记：走到命中的起止处，就把当时的输出长度记下来。
pub fn hit_context(text: &str, range: Range<usize>) -> HitContext {
    let line = text
        .get(..range.start)
        .map(|h| h.matches('\n').count() + 1)
        .unwrap_or(1);

    // 前后各取 15 个**字符**（不是字节）——中文一字 3 字节，按字节取会切出乱码。
    let before_start = text
        .get(..range.start)
        .map(|h| {
            h.char_indices()
                .rev()
                .take(CONTEXT_CHARS)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(range.start)
        })
        .unwrap_or(0);
    let after_end = text
        .get(range.end..)
        .map(|t| {
            let mut end = range.end;
            for (i, c) in t.char_indices().take(CONTEXT_CHARS) {
                end = range.end + i + c.len_utf8();
            }
            end
        })
        .unwrap_or(range.end);

    let mut context = String::new();
    let mut hl_start = 0usize;
    let mut hl_end = 0usize;

    for (off, c) in text
        .get(before_start..after_end)
        .unwrap_or("")
        .char_indices()
    {
        let abs = before_start + off;
        if abs == range.start {
            hl_start = context.len();
        }
        if abs == range.end {
            hl_end = context.len();
        }
        match c {
            '\n' => context.push('⏎'),
            other => context.push(other),
        }
    }
    // 命中一直到上下文末尾时，循环里碰不到 range.end。
    if range.end >= after_end {
        hl_end = context.len();
    }

    HitContext {
        range,
        line,
        context,
        highlight: hl_start..hl_end,
    }
}

// ============ 折叠 ============

/// 折叠后的文本，附带回查原文位置的索引。
struct Folded {
    text: String,
    /// 与 `text` 中的字符一一对应：(折叠后的字节偏移, 原文字节偏移, 原文字节长度)。
    ///
    /// 折叠是 1:1 的**字符**映射，但字节长度会变（`Ａ` 3 → `A` 1），
    /// 所以必须留这张表才回得去。这正是 NFKC 做不到的地方。
    map: Vec<(usize, usize, usize)>,
}

impl Folded {
    fn build(text: &str, flags: &MatchFlags) -> Self {
        let mut out = String::with_capacity(text.len());
        let mut map = Vec::new();

        for (orig_off, c) in text.char_indices() {
            let folded = fold_char(c, flags);
            map.push((out.len(), orig_off, c.len_utf8()));
            out.push(folded);
        }
        Self { text: out, map }
    }

    /// 把折叠坐标的区间映射回原文。
    fn to_original(&self, folded: Range<usize>) -> Option<Range<usize>> {
        // 空匹配（如正则 `a*` 匹配空串）没有对应的原文区间，跳过。
        if folded.start >= folded.end {
            return None;
        }
        let first = self.map.iter().find(|(f, _, _)| *f >= folded.start)?;
        let last = self
            .map
            .iter()
            .rev()
            .find(|(f, _, _)| *f < folded.end && *f >= folded.start)?;
        Some(first.1..(last.1 + last.2))
    }
}

/// 折叠单个字符。**只做 1:1 映射**——一对多会让字符对不上号。
fn fold_char(c: char, flags: &MatchFlags) -> char {
    let mut c = c;
    if flags.fold_width {
        c = fold_width_char(c);
    }
    if flags.fold_cjk_punct {
        c = fold_cjk_punct_char(c);
    }
    if flags.ignore_case {
        // to_lowercase 可能产生多个字符（如 'İ'），那就破坏 1:1。
        // 只取单字符的结果，否则原样保留。
        let mut it = c.to_lowercase();
        if let (Some(l), None) = (it.next(), it.next()) {
            c = l;
        }
    }
    c
}

/// 全角 ASCII → 半角。
fn fold_width_char(c: char) -> char {
    let u = c as u32;
    match u {
        // 全角 ！(FF01) ~ ～(FF5E) → 半角 !(21) ~ ~(7E)
        0xFF01..=0xFF5E => char::from_u32(u - 0xFF01 + 0x21).unwrap_or(c),
        // 全角空格 → 半角空格
        0x3000 => ' ',
        _ => c,
    }
}

/// 中文标点 → 对应的半角标点。只收 1:1 的。
///
/// `……` → `...` 这类一对多不收——它会让字符对不上号，
/// 命中位置就映射不回去了（这正是 §6.6 警告的那个坑的同源问题）。
fn fold_cjk_punct_char(c: char) -> char {
    match c {
        '。' => '.',
        '，' | '、' => ',',
        '；' => ';',
        '：' => ':',
        '？' => '?',
        '！' => '!',
        '（' => '(',
        '）' => ')',
        '「' | '『' | '“' | '”' => '"',
        '」' | '』' | '‘' | '’' => '\'',
        '《' | '〈' => '<',
        '》' | '〉' => '>',
        '【' => '[',
        '】' => ']',
        '—' | '－' => '-',
        '…' | '·' => '.',
        _ => c,
    }
}

// ============ 普通 / 全词 ============

fn search_literal(text: &str, q: &Query, whole_word: bool) -> Vec<Range<usize>> {
    let flags = q.flags;
    let hay = Folded::build(text, &flags);
    let needle: String = q.pattern.chars().map(|c| fold_char(c, &flags)).collect();

    if needle.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = hay.text.get(from..).and_then(|s| s.find(&needle)) {
        let start = from + rel;
        let end = start + needle.len();

        if let Some(orig) = hay.to_original(start..end)
            && (!whole_word || is_whole_word(text, &orig))
        {
            out.push(orig);
        }
        // 从命中的**下一个字符**继续找，避免重叠命中与死循环。
        from = next_char_boundary(&hay.text, start);
        if from >= hay.text.len() {
            break;
        }
    }
    out
}

fn next_char_boundary(s: &str, from: usize) -> usize {
    s.get(from..)
        .and_then(|r| r.chars().next())
        .map(|c| from + c.len_utf8())
        .unwrap_or(s.len())
}

/// 全词：命中两侧不得是「同类」字符。
///
/// 中文没有空格分词，逐字都是边界——故只对拉丁字母/数字施加这条约束。
/// 否则「雪」在「雪夜」里就永远搜不到，而那显然不是用户要的。
fn is_whole_word(text: &str, range: &Range<usize>) -> bool {
    let before = text.get(..range.start).and_then(|s| s.chars().next_back());
    let after = text.get(range.end..).and_then(|s| s.chars().next());

    let inner_first = text.get(range.clone()).and_then(|s| s.chars().next());
    let inner_last = text.get(range.clone()).and_then(|s| s.chars().next_back());

    let boundary_ok = |outer: Option<char>, inner: Option<char>| match (outer, inner) {
        (Some(o), Some(i)) => !(is_word_char(o) && is_word_char(i)),
        _ => true,
    };
    boundary_ok(before, inner_first) && boundary_ok(after, inner_last)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && c.is_ascii()
}

// ============ 正则 ============

fn search_regex(text: &str, q: &Query) -> Result<Vec<Range<usize>>, SearchError> {
    // 折叠同样适用于正则：在折叠副本上匹配，再照表把位置换回原文。
    let hay = Folded::build(text, &q.flags);

    if q.flags.extended {
        let re = fancy_regex::Regex::new(&q.pattern)
            .map_err(|e| SearchError::BadRegex(e.to_string()))?;
        let mut out = Vec::new();
        for m in re.find_iter(&hay.text) {
            let m = m.map_err(|e| SearchError::BadRegex(e.to_string()))?;
            if let Some(orig) = hay.to_original(m.start()..m.end()) {
                out.push(orig);
            }
        }
        return Ok(out);
    }

    let re = build_regex(q)?;
    Ok(re
        .find_iter(&hay.text)
        .filter_map(|m| hay.to_original(m.start()..m.end()))
        .collect())
}

fn build_regex(q: &Query) -> Result<regex::Regex, SearchError> {
    regex::RegexBuilder::new(&q.pattern)
        // 折叠已经把大小写抹平了；这里再开一次是冗余但无害的保险。
        .case_insensitive(q.flags.ignore_case)
        .build()
        .map_err(|e| SearchError::BadRegex(e.to_string()))
}

fn replace_regex(text: &str, q: &Query, to: &str) -> Result<Vec<Edit>, SearchError> {
    if q.pattern.is_empty() {
        return Err(SearchError::EmptyPattern);
    }
    let hay = Folded::build(text, &q.flags);

    // fancy-regex 不支持 replace 的展开语义，故扩展语法下只做整体替换，
    // 不支持 $1。据实报错好过悄悄把 `$1` 当字面量写进用户的正文。
    if q.flags.extended {
        if to.contains('$') {
            return Err(SearchError::BadRegex(
                "扩展语法（lookaround）下暂不支持 $1 捕获组引用".into(),
            ));
        }
        return Ok(search_regex(text, q)?
            .into_iter()
            .map(|range| Edit {
                range,
                new: to.to_string(),
                rule: "replace",
            })
            .collect());
    }

    let re = build_regex(q)?;
    let mut out = Vec::new();
    for caps in re.captures_iter(&hay.text) {
        let Some(m) = caps.get(0) else { continue };
        let Some(orig) = hay.to_original(m.start()..m.end()) else {
            continue;
        };
        // 展开 $1 等捕获组引用（§6.6 [MUST]）。
        let mut new = String::new();
        caps.expand(to, &mut new);
        out.push(Edit {
            range: orig,
            new,
            rule: "replace",
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    #![allow(clippy::string_slice)] // 区间来自 search_text，构造即保证落在边界上

    use super::*;

    fn q(pattern: &str) -> Query {
        Query {
            pattern: pattern.into(),
            mode: MatchMode::Literal,
            flags: MatchFlags::default(),
        }
    }

    fn found(text: &str, query: &Query) -> Vec<String> {
        search_text(text, query)
            .unwrap()
            .into_iter()
            .map(|r| text[r].to_string())
            .collect()
    }

    // ---- 普通查找 ----

    #[test]
    fn finds_literal_matches() {
        let text = "雪落了一夜。雪停了。";
        let hits = search_text(text, &q("雪")).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(found(text, &q("雪")), ["雪", "雪"]);
    }

    #[test]
    fn finds_nothing_when_absent() {
        assert!(search_text("雪落了", &q("风")).unwrap().is_empty());
    }

    #[test]
    fn empty_pattern_is_an_error() {
        assert!(matches!(
            search_text("雪", &q("")),
            Err(SearchError::EmptyPattern)
        ));
    }

    /// 命中区间必须是**原文**坐标——这是整个模块的地基。
    #[test]
    fn hit_ranges_are_original_offsets() {
        let text = "　　雪落了一夜。";
        let hits = search_text(text, &q("雪")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].clone()], "雪");
        assert_eq!(hits[0].start, 6, "两个全角空格 = 6 字节");
    }

    #[test]
    fn overlapping_patterns_do_not_loop() {
        let text = "aaaa";
        let hits = search_text(text, &q("aa")).unwrap();
        // 从下一个字符继续找：aa(0..2), aa(1..3), aa(2..4)
        assert_eq!(hits.len(), 3);
    }

    // ---- 大小写 ----

    #[test]
    fn ignore_case_matches_both() {
        let mut query = q("Snow");
        query.flags.ignore_case = true;
        assert_eq!(found("snow SNOW Snow", &query).len(), 3);
    }

    #[test]
    fn case_sensitive_by_default() {
        assert_eq!(found("snow SNOW", &q("snow")).len(), 1);
    }

    // ---- 全半角折叠（§6.6 的难点）----

    #[test]
    fn fold_width_matches_fullwidth_alnum() {
        let mut query = q("A1");
        query.flags.fold_width = true;
        let text = "Ａ１";
        let hits = search_text(text, &query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].clone()], "Ａ１", "命中应是原文的全角形式");
    }

    /// 折叠后字节长度变了，位置仍要精确映射回原文——这正是不能用 NFKC 的原因。
    #[test]
    fn fold_width_maps_position_back_correctly() {
        let mut query = q("abc");
        query.flags.fold_width = true;
        // 全角 ａｂｃ 每字 3 字节，前面还有个中文字。
        let text = "雪ａｂｃ雪";
        let hits = search_text(text, &query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], 3..12, "应是原文的字节区间（3 + 3*3）");
        assert_eq!(&text[hits[0].clone()], "ａｂｃ");
    }

    #[test]
    fn fold_width_is_off_by_default() {
        assert!(search_text("Ａ", &q("A")).unwrap().is_empty(), "默认不折叠");
    }

    #[test]
    fn fold_width_handles_fullwidth_space() {
        let mut query = q(" ");
        query.flags.fold_width = true;
        assert_eq!(search_text("　", &query).unwrap().len(), 1);
    }

    // ---- 中文标点折叠 ----

    #[test]
    fn fold_cjk_punct_treats_comma_forms_as_same() {
        let mut query = q(",");
        query.flags.fold_cjk_punct = true;
        let text = "雪，落，了";
        let hits = search_text(text, &query).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(&text[hits[0].clone()], "，", "命中的是原文的全角逗号");
    }

    #[test]
    fn fold_cjk_punct_matches_period() {
        let mut query = q(".");
        query.flags.fold_cjk_punct = true;
        assert_eq!(search_text("雪落了。", &query).unwrap().len(), 1);
    }

    /// 反向也成立：用全角标点搜半角。
    #[test]
    fn fold_cjk_punct_works_in_reverse() {
        let mut query = q("，");
        query.flags.fold_cjk_punct = true;
        assert_eq!(search_text("a,b", &query).unwrap().len(), 1);
    }

    // ---- 全词 ----

    #[test]
    fn whole_word_excludes_substrings() {
        let mut query = q("snow");
        query.mode = MatchMode::WholeWord;
        assert_eq!(found("snow snowman", &query).len(), 1, "snowman 不算");
    }

    /// 中文没有空格分词，逐字都是边界——否则「雪」在「雪夜」里永远搜不到。
    #[test]
    fn whole_word_still_matches_cjk_substrings() {
        let mut query = q("雪");
        query.mode = MatchMode::WholeWord;
        assert_eq!(found("雪夜行", &query).len(), 1, "中文不该被全词挡住");
    }

    #[test]
    fn whole_word_matches_at_boundaries() {
        let mut query = q("a");
        query.mode = MatchMode::WholeWord;
        assert_eq!(found("a b a", &query).len(), 2);
        assert_eq!(found("ab", &query).len(), 0);
    }

    // ---- 正则 ----

    #[test]
    fn regex_finds_matches() {
        let mut query = q(r"雪\p{Han}");
        query.mode = MatchMode::Regex;
        assert_eq!(found("雪落 雪停", &query), ["雪落", "雪停"]);
    }

    /// §6.6 [MUST]：非法正则实时提示，**不得 panic**。
    #[test]
    fn invalid_regex_is_an_error_not_a_panic() {
        let mut query = q("[unclosed");
        query.mode = MatchMode::Regex;
        assert!(matches!(
            search_text("雪", &query),
            Err(SearchError::BadRegex(_))
        ));
    }

    #[test]
    fn regex_with_ignore_case() {
        let mut query = q("snow");
        query.mode = MatchMode::Regex;
        query.flags.ignore_case = true;
        assert_eq!(found("SNOW Snow", &query).len(), 2);
    }

    /// 折叠对正则同样生效——在折叠副本上匹配，再把位置换回原文。
    #[test]
    fn regex_respects_folding() {
        let mut query = q(r"\d+");
        query.mode = MatchMode::Regex;
        query.flags.fold_width = true;
        let text = "２０２６年";
        let hits = search_text(text, &query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].clone()], "２０２６", "命中的是原文的全角数字");
    }

    #[test]
    fn extended_regex_supports_lookaround() {
        let mut query = q(r"雪(?=夜)");
        query.mode = MatchMode::Regex;
        query.flags.extended = true;
        let hits = search_text("雪夜 雪天", &query).unwrap();
        assert_eq!(hits.len(), 1, "只有「雪夜」里的雪该命中");
    }

    #[test]
    fn extended_invalid_regex_is_an_error() {
        let mut query = q("(?<broken");
        query.mode = MatchMode::Regex;
        query.flags.extended = true;
        assert!(search_text("雪", &query).is_err());
    }

    // ---- 替换 ----

    #[test]
    fn replace_preview_produces_edits() {
        let text = "雪落了。雪停了。";
        let edits = replace_preview(text, &q("雪"), "风").unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(mj_text_apply(text, &edits), "风落了。风停了。");
    }

    /// §6.6 [MUST]：正则替换支持 `$1` 捕获组引用。
    #[test]
    fn regex_replace_expands_capture_groups() {
        let mut query = q(r"(\p{Han})落");
        query.mode = MatchMode::Regex;
        let text = "雪落了";
        let edits = replace_preview(text, &query, "$1停").unwrap();
        assert_eq!(mj_text_apply(text, &edits), "雪停了");
    }

    #[test]
    fn regex_replace_with_multiple_groups() {
        let mut query = q(r"(\w+)-(\w+)");
        query.mode = MatchMode::Regex;
        let text = "a-b";
        let edits = replace_preview(text, &query, "$2-$1").unwrap();
        assert_eq!(mj_text_apply(text, &edits), "b-a");
    }

    /// 折叠状态下替换：命中的是全角原文，替换后要精确落在它的位置上。
    #[test]
    fn replace_with_folding_targets_original_text() {
        let mut query = q("abc");
        query.flags.fold_width = true;
        let text = "雪ａｂｃ雪";
        let edits = replace_preview(text, &query, "X").unwrap();
        assert_eq!(mj_text_apply(text, &edits), "雪X雪");
    }

    /// 扩展语法下不支持 $1：据实报错，好过把 `$1` 当字面量写进正文。
    #[test]
    fn extended_replace_rejects_capture_refs() {
        let mut query = q("雪");
        query.mode = MatchMode::Regex;
        query.flags.extended = true;
        assert!(replace_preview("雪", &query, "$1").is_err());
    }

    #[test]
    fn replace_with_empty_removes_matches() {
        let text = "雪落雪";
        let edits = replace_preview(text, &q("雪"), "").unwrap();
        assert_eq!(mj_text_apply(text, &edits), "落");
    }

    // ---- 上下文 ----

    #[test]
    fn context_shows_surrounding_text() {
        let text = "　　雪落了一夜。他推开门，风裹着雪灌进来，冷得刺骨。";
        let hits = search_text(text, &q("风")).unwrap();
        let ctx = hit_context(text, hits[0].clone());
        assert!(ctx.context.contains('风'));
        assert_eq!(
            &ctx.context[ctx.highlight.clone()],
            "风",
            "高亮区间要对准命中"
        );
        assert_eq!(ctx.line, 1);
    }

    #[test]
    fn context_reports_line_number() {
        let text = "第一行\n第二行\n第三行雪";
        let hits = search_text(text, &q("雪")).unwrap();
        assert_eq!(hit_context(text, hits[0].clone()).line, 3);
    }

    /// 上下文按**字符**取，不能按字节——中文一字 3 字节，按字节切会切出乱码。
    #[test]
    fn context_does_not_split_cjk() {
        let text = "雪".repeat(50);
        let hits = search_text(&text, &q("雪")).unwrap();
        let ctx = hit_context(&text, hits[25].clone());
        // 能构造出 String 即证明没切碎。
        assert!(ctx.context.chars().all(|c| c == '雪'));
        assert!(ctx.context.chars().count() <= CONTEXT_CHARS * 2 + 1);
    }

    #[test]
    fn context_at_text_start_does_not_underflow() {
        let text = "雪落了";
        let hits = search_text(text, &q("雪")).unwrap();
        let ctx = hit_context(text, hits[0].clone());
        assert_eq!(ctx.highlight.start, 0);
    }

    #[test]
    fn context_makes_newlines_visible() {
        let text = "第一行\n雪";
        let hits = search_text(text, &q("雪")).unwrap();
        assert!(hit_context(text, hits[0].clone()).context.contains('⏎'));
    }

    /// proptest 抓到的：`\n`(1 字节) 显形成 `⏎`(3 字节)，长度变了，
    /// 拿原文偏移去减算出的高亮就落进了 `⏎` 的字节中间。
    /// 与 §6.6 警告 NFKC 的是同一个坑。留作回归。
    #[test]
    fn context_highlight_survives_newline_expansion() {
        for text in ["\nA", "雪\n雪", "\n\n雪", "a\nb\nc雪"] {
            let hits = search_text(text, &q(if text.contains('雪') { "雪" } else { "A" })).unwrap();
            for h in hits {
                let ctx = hit_context(text, h.clone());
                assert_eq!(
                    &ctx.context[ctx.highlight.clone()],
                    &text[h],
                    "{text:?} 的高亮区间错位"
                );
            }
        }
    }

    /// 命中一直顶到上下文末尾时，高亮的终点也要对。
    #[test]
    fn context_highlight_at_end_of_text() {
        let text = "雪落了雪";
        let hits = search_text(text, &q("雪")).unwrap();
        let last = hits.last().unwrap().clone();
        let ctx = hit_context(text, last.clone());
        assert_eq!(&ctx.context[ctx.highlight.clone()], &text[last]);
    }

    /// 测试用：套用编辑。
    fn mj_text_apply(text: &str, edits: &[Edit]) -> String {
        crate::format::apply(text, edits)
    }
}
