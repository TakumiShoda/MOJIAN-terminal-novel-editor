//! 一键排版。见 doc.md §6.5。
//!
//! 三条核心约束（全是验收项）：
//! 1. **幂等**：`format(format(x)) == format(x)`，由 proptest 保证（§10 发布门禁）；
//! 2. **可预览**：`plan` 返回编辑列表而非新字符串——这样才能预览、逐条取消、
//!    精确映射到光标位置；
//! 3. **可撤销**：一次排版 = 一个撤销组（由 mj-tui 侧保证）。
//!
//! **总原则：拿不准就不动。** 排版是在动用户的正文——多改一处不如少改一处。
//! 孤立引号、看不出是不是 URL 的点号、可能是英文缩写的撇号，一律放过。

use std::ops::Range;

use unicode_general_category::{GeneralCategory as G, get_general_category};

/// 段首缩进方式（§6.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphIndent {
    /// 两个 U+3000。中文正文的标准形态。
    #[default]
    FullWidthTwo,
    /// 去掉段首缩进。
    None,
    /// 原样保留。
    Keep,
}

/// 排版规则开关（§6.5 规则表）。默认值即表中的「默认」列。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FormatOptions {
    pub trim_trailing: bool,
    pub collapse_blank: bool,
    pub paragraph_indent: ParagraphIndent,
    pub unify_ellipsis: bool,
    pub unify_dash: bool,
    pub punct_to_full_width: bool,
    pub unify_quotes: bool,
    /// **默认关**：中文网文习惯不加中英空格。
    pub cjk_latin_space: bool,
    pub full_width_digits: bool,
    pub strip_inline_space: bool,
    pub repeat_punct: bool,
    pub line_join: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            trim_trailing: true,
            collapse_blank: true,
            paragraph_indent: ParagraphIndent::FullWidthTwo,
            unify_ellipsis: true,
            unify_dash: true,
            punct_to_full_width: true,
            unify_quotes: true,
            cjk_latin_space: false,
            full_width_digits: true,
            strip_inline_space: true,
            repeat_punct: false,
            line_join: false,
        }
    }
}

impl FormatOptions {
    /// 全关。用于只跑单条规则的测试与「只做某一项」的场景。
    pub fn none() -> Self {
        Self {
            trim_trailing: false,
            collapse_blank: false,
            paragraph_indent: ParagraphIndent::Keep,
            unify_ellipsis: false,
            unify_dash: false,
            punct_to_full_width: false,
            unify_quotes: false,
            cjk_latin_space: false,
            full_width_digits: false,
            strip_inline_space: false,
            repeat_punct: false,
            line_join: false,
        }
    }
}

/// 一处改动。`rule` 用于预览时告诉用户「这是哪条规则干的」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub new: String,
    pub rule: &'static str,
}

/// 规则优先级：数值小者胜（对应 §6.5 表中从上到下）。
fn priority(rule: &str) -> u8 {
    match rule {
        "trim_trailing" => 0,
        "collapse_blank" => 1,
        "paragraph_indent" => 2,
        "unify_ellipsis" => 3,
        "unify_dash" => 4,
        "punct_to_full_width" => 5,
        "unify_quotes" => 6,
        "cjk_latin_space" => 7,
        "full_width_digits" => 8,
        "strip_inline_space" => 9,
        "repeat_punct" => 10,
        "line_join" => 11,
        _ => 255,
    }
}

/// 排版最多迭代的遍数。
///
/// 实测 2 遍足够收敛；留出余量是为了极端输入。超过则记警告并就此打住——
/// 排版差一点点，好过卡死或改出个用户没预期的结果。
const MAX_PASSES: usize = 8;

/// 生成编辑计划。
///
/// 返回的编辑**互不重叠**、按 range 升序，且**就是 `format` 会做的全部改动**
/// ——预览所见即所得。
///
/// # 为什么要迭代
///
/// 单遍扫描无法收敛：规则之间不只是「判据要预判别人的输出」，更根本的是
/// **几条规则的联合输出本身还需要排版**。proptest 用 `。  .。-` 打出了这一点：
/// 删掉 CJK 之间的空格 + 把 `.` 全角化，联合起来造出了一个 `。。。`，
/// 而那正是省略号规则的输入——它在下一遍才看得见。
///
/// 这类「改动创造新改动」的情形没法靠打补丁堵死（我试过三轮，
/// 每堵一个 proptest 就找出下一个）。故改为跑到不动点，再把
/// 「原文 → 最终文本」的净差异作为编辑列表返回。
///
/// 这样三件事同时成立：
/// - `format` 幂等（不动点的定义）；
/// - 预览诚实（列表就是最终结果的差异）；
/// - 契约不变（`format == apply(text, plan(text))`，仍是 §6.5 写的那个式子）。
pub fn plan(text: &str, opts: &FormatOptions) -> Vec<Edit> {
    let final_text = run_to_fixpoint(text, opts);
    if final_text == text {
        return Vec::new();
    }
    // 首遍的候选用来给差异块归属规则名——预览要告诉用户「这是哪条规则干的」。
    let first_pass = single_pass(text, opts);
    diff_to_edits(text, &final_text, &first_pass)
}

/// 反复施加单遍排版，直到文本不再变化。
fn run_to_fixpoint(text: &str, opts: &FormatOptions) -> String {
    let mut cur = text.to_owned();
    for _ in 0..MAX_PASSES {
        let edits = single_pass(&cur, opts);
        if edits.is_empty() {
            return cur;
        }
        let next = apply(&cur, &edits);
        if next == cur {
            // 有编辑却没变化：说明编辑全是空操作或被跳过了。就此打住，避免死循环。
            return cur;
        }
        cur = next;
    }
    tracing::warn!(passes = MAX_PASSES, "排版未在限定遍数内收敛，就此打住");
    cur
}

/// 把「原文 → 最终文本」的差异转成编辑列表。
///
/// 用字符级 diff：结果天然互不重叠、按序排列，且是最小改动集。
fn diff_to_edits(old: &str, new: &str, first_pass: &[Edit]) -> Vec<Edit> {
    let new_chars: Vec<char> = new.chars().collect();

    // 字符下标 → 字节偏移。diff 给的是字符下标，而 Edit 用字节。
    let mut char_to_byte: Vec<usize> = old.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(old.len());

    let diff = similar::TextDiff::from_chars(old, new);
    let mut out = Vec::new();

    // **自己记住读到原文哪儿了**，不要信 Insert 的 old_range。
    //
    // similar 给的是一份「按序执行的脚本」：Delete 的 new_index、Insert 的 old_index
    // 只是位置标记，不是有意义的区间——实测能看到 `Insert old=1..1` 出现在
    // `Delete old=0..4` 之后。照字面用就会生成互相重叠的编辑，apply 之后正文直接错乱
    // （`２０２６　雪` 被排成 `２　2026　０２６　雪`）。
    // 按顺序消费 old、把 Insert 锚在「当前读到的位置」，才是这份脚本的正确读法。
    let mut old_pos = 0usize;

    for op in diff.ops() {
        use similar::DiffTag::*;
        let (tag, old_range, new_range) = op.as_tag_tuple();
        match tag {
            Equal => old_pos = old_range.end,
            Delete => {
                out.push(make_edit(
                    &char_to_byte,
                    old_range.start..old_range.end,
                    String::new(),
                    first_pass,
                ));
                old_pos = old_range.end;
            }
            Insert => {
                let text: String = new_chars[new_range].iter().collect();
                out.push(make_edit(&char_to_byte, old_pos..old_pos, text, first_pass));
            }
            Replace => {
                let text: String = new_chars[new_range].iter().collect();
                out.push(make_edit(
                    &char_to_byte,
                    old_range.start..old_range.end,
                    text,
                    first_pass,
                ));
                old_pos = old_range.end;
            }
        }
    }
    out
}

/// 由字符区间造一条编辑（转成字节偏移并归属规则）。
fn make_edit(
    char_to_byte: &[usize],
    chars: Range<usize>,
    new: String,
    first_pass: &[Edit],
) -> Edit {
    let start = char_to_byte[chars.start];
    let end = char_to_byte[chars.end];
    Edit {
        range: start..end,
        new,
        rule: attribute(start..end, first_pass),
    }
}

/// 给一处差异块找个规则名：取首遍里与之重叠的规则。
///
/// 找不到说明这块是多条规则联合作用的产物（或第二遍才出现的），
/// 据实标为「组合规则」而不是硬安一个——预览上骗人比不说更糟。
fn attribute(range: Range<usize>, first_pass: &[Edit]) -> &'static str {
    first_pass
        .iter()
        .find(|e| e.range.start < range.end && range.start < e.range.end)
        .map(|e| e.rule)
        .unwrap_or("组合规则")
}

/// 单遍排版：各规则扫描当前文本产生候选，按优先级裁决重叠。
fn single_pass(text: &str, opts: &FormatOptions) -> Vec<Edit> {
    let mut cands = Vec::new();

    // 「排版后每个字符是否会是 CJK」——依赖上下文的两条规则共用它。
    // 见 effective_cjk_mask 的说明：不预判就不收敛。
    let mask = effective_cjk_mask(&text.chars().collect::<Vec<_>>(), opts);

    // 各规则独立扫描原文产生候选，重叠由 resolve 按优先级裁决。
    //
    // 为什么不「依次施加、后一条看前一条的结果」：那样编辑就无法映射回原文的
    // 坐标，预览与逐条取消都无从谈起（§6.5 明确要求返回编辑列表而非新字符串）。
    // 代价是每条规则的判据必须自己稳健——不能指望别的规则先把输入洗干净。
    // collapse_blank 要整段吞掉的空行区间。trim_trailing 需要避让它们。
    let collapsible = if opts.collapse_blank {
        blank_runs(text)
    } else {
        Vec::new()
    };

    if opts.trim_trailing {
        rule_trim_trailing(text, &mut cands, &collapsible);
    }
    if opts.collapse_blank {
        rule_collapse_blank(&mut cands, &collapsible);
    }
    if opts.paragraph_indent != ParagraphIndent::Keep {
        rule_paragraph_indent(text, opts.paragraph_indent, &mut cands);
    }
    if opts.unify_ellipsis {
        rule_unify_ellipsis(text, &mut cands);
    }
    if opts.unify_dash {
        rule_unify_dash(text, &mut cands);
    }
    if opts.punct_to_full_width {
        rule_punct_to_full_width(text, &mut cands, &mask);
    }
    if opts.unify_quotes {
        rule_unify_quotes(text, &mut cands);
    }
    if opts.cjk_latin_space {
        rule_cjk_latin_space(text, &mut cands, &mask);
    }
    if opts.full_width_digits {
        rule_full_width_digits(text, &mut cands);
    }
    if opts.strip_inline_space {
        rule_strip_inline_space(text, &mut cands, &mask);
    }
    if opts.repeat_punct {
        rule_repeat_punct(text, &mut cands);
    }
    if opts.line_join {
        rule_line_join(text, &mut cands);
    }

    resolve(text, cands)
}

/// 应用编辑。按 range **倒序**应用，避免前面的改动让后面的偏移失效。
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut out = text.to_owned();
    let mut sorted: Vec<&Edit> = edits.iter().collect();

    // 倒序应用，避免前面的改动让后面的偏移失效。
    //
    // 同起点时**长的先做**：段首缩进是 `0..0` 的插入，而破折号是 `0..2` 的替换，
    // 两者起点相同。若先插入 `　　`，`0..2` 就落进了全角空格的字节中间——
    // proptest 用 `"--"` 当场打出这个 panic。先替换再插入则两不相干。
    sorted.sort_by_key(|e| {
        (
            std::cmp::Reverse(e.range.start),
            std::cmp::Reverse(e.range.end),
        )
    });

    for e in sorted {
        // 两端都要验边界。只验 start 是不够的——`replace_range` 对 end
        // 同样会 panic，而 proptest 用 "--" 当场把这个漏洞打了出来
        // （§0 禁令 5 的同源问题：任何对正文的字节切片都要自证落在边界上）。
        let ok = e.range.start <= e.range.end
            && e.range.end <= out.len()
            && out.is_char_boundary(e.range.start)
            && out.is_char_boundary(e.range.end);
        if !ok {
            // 编辑列表与文本对不上：跳过而非 panic——
            // 排版失败顶多是没排上，不该把用户的正文搞坏。
            tracing::warn!(rule = e.rule, range = ?e.range, "编辑区间非法，跳过");
            continue;
        }
        out.replace_range(e.range.clone(), &e.new);
    }
    out
}

/// 排版 = plan + apply。
#[inline]
pub fn format(text: &str, opts: &FormatOptions) -> String {
    apply(text, &plan(text, opts))
}

/// 裁决重叠：按优先级取胜者，丢弃与已选区间重叠的候选。
fn resolve(text: &str, mut cands: Vec<Edit>) -> Vec<Edit> {
    // 先剔除空操作：new 与原文一致的编辑不该出现在预览里，
    // 否则「3 处改动」里有 2 处什么都没改，用户会以为程序在骗他。
    cands.retain(|e| text.get(e.range.clone()).is_some_and(|old| old != e.new));

    // 按 (起点, 优先级) 排序：同起点时优先级高的排前面。
    cands.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(priority(a.rule).cmp(&priority(b.rule)))
    });

    let mut out: Vec<Edit> = Vec::with_capacity(cands.len());
    for e in cands {
        // 与已接受的任一编辑重叠则丢弃。已接受者优先级不低于当前
        // （同起点时排序保证；不同起点时先到先得）。
        if let Some(last) = out.last()
            && e.range.start < last.range.end
        {
            tracing::warn!(
                rule = e.rule,
                winner = last.rule,
                range = ?e.range,
                "排版规则区间重叠，按优先级裁决"
            );
            continue;
        }
        out.push(e);
    }
    out
}

// ============ 规则实现 ============

/// 删除行尾空白。
///
/// `collapsible` 是 collapse_blank 将整段吞掉的空行区间——落在其中的行不必再修剪。
/// 若不避让，两条规则会在同一段空白上抢地盘：trim 优先级高，赢下来只删了空格，
/// 把 collapse 挤掉，于是第二遍排版才压缩空行——幂等破了
/// （proptest 用 `" \n\n"` 抓到）。
fn rule_trim_trailing(text: &str, out: &mut Vec<Edit>, collapsible: &[Range<usize>]) {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim_end();
        let in_collapsible = collapsible
            .iter()
            .any(|r| r.start <= offset && offset < r.end);

        if trimmed.len() < content.len() && !in_collapsible {
            out.push(Edit {
                range: (offset + trimmed.len())..(offset + content.len()),
                new: String::new(),
                rule: "trim_trailing",
            });
        }
        offset += line.len();
    }
}

/// 连续 2 行以上空行的区间（按整行计）。空行 = 只含空白的行。
///
/// 用户手里的「空行」常带着几个空格，只认完全空的行会让 collapse_blank
/// 在真实稿子上失灵。
fn blank_runs(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut start: Option<usize> = None;
    let mut end = 0usize;
    let mut count = 0usize;

    for line in text.split_inclusive('\n') {
        let is_blank = line.trim().is_empty() && line.ends_with('\n');
        if is_blank {
            if start.is_none() {
                start = Some(offset);
                count = 0;
            }
            count += 1;
            end = offset + line.len();
        } else if let Some(s) = start.take()
            && count >= 2
        {
            out.push(s..end);
        }
        offset += line.len();
    }
    if let Some(s) = start
        && count >= 2
    {
        out.push(s..end);
    }
    out
}

/// 连续空行压缩为 1。整段换成一个 `"\n"`——含其中的空格，一并洗掉。
fn rule_collapse_blank(out: &mut Vec<Edit>, runs: &[Range<usize>]) {
    for r in runs {
        out.push(Edit {
            range: r.clone(),
            new: "\n".to_string(),
            rule: "collapse_blank",
        });
    }
}

/// 段首缩进。
fn rule_paragraph_indent(text: &str, mode: ParagraphIndent, out: &mut Vec<Edit>) {
    const INDENT: &str = "\u{3000}\u{3000}";
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);

        // 空行不缩进——它是排版留白，不是段落。
        // （也避免与 trim_trailing 争夺同一段空白。）
        if content.trim().is_empty() {
            offset += line.len();
            continue;
        }

        let lead_len = content.len() - content.trim_start().len();
        let want = match mode {
            ParagraphIndent::FullWidthTwo => INDENT,
            ParagraphIndent::None => "",
            ParagraphIndent::Keep => unreachable!("Keep 不该走到这里"),
        };
        let lead = content.get(..lead_len).unwrap_or("");
        if lead != want {
            out.push(Edit {
                range: offset..(offset + lead_len),
                new: want.to_string(),
                rule: "paragraph_indent",
            });
        }
        offset += line.len();
    }
}

/// 省略号统一为 `……`。
fn rule_unify_ellipsis(text: &str, out: &mut Vec<Edit>) {
    // 按「省略号类字符的极大连续段」整体匹配，而不是逐字符、也不是按单一字符分段：
    //
    // - 逐字符替换会让 `…` → `……` → `………`，永远不收敛；
    // - 按单一字符分段则 `。。。...` 会变成两段各自的 `……` = `………`，
    //   而它本身又是一段省略号，第二遍还要再收缩一次——proptest 抓到的正是这个。
    //
    // 整段换成 `……` 才天然幂等：`……` 自己也是一段，换完还是 `……`（空操作被滤掉）。
    for (start, end, seg) in class_runs(text, is_ellipsis_char) {
        let n = seg.chars().count();

        // 只在两种**明确**的情形下动手：
        // - 全是 `…`（`…`、`……`、`………` 都是省略号，规整成 `……`）；
        // - 一个 `…` 都没有、且够 3 个（`...`、`。。。`、`。。。...`）。
        let all_dots = seg.chars().all(|c| c == '…');
        let no_real_ellipsis = !seg.contains('…');

        if all_dots || (no_real_ellipsis && n >= 3) {
            out.push(Edit {
                range: start..end,
                new: "……".to_string(),
                rule: "unify_ellipsis",
            });
        }
        // 混合段（`。……`、`。。。…`）一律**不动**——§6.5：拿不准就不动。
        //
        // `。……` 里的句号极可能是真句号（「他说。……然后走了」），整段吞掉就把它吃了。
        // 而想「只规整其中的省略号部分」也不行：`。。。…` 会变成 `……`+`……`=`………`，
        // 下一遍再缩成 `……`，永远差一拍（proptest 抓到）。
        // 与其猜错，不如原样留着——用户自己看得见，我们看不准。
    }
}

fn is_ellipsis_char(c: char) -> bool {
    matches!(c, '.' | '。' | '·' | '…')
}

/// 破折号统一为 `——`。
fn rule_unify_dash(text: &str, out: &mut Vec<Edit>) {
    // 同上：整段匹配。`--—` 若按单一字符分段会得到 `————`，第二遍才收敛。
    for (start, end, seg) in class_runs(text, is_dash_char) {
        let n = seg.chars().count();
        // 单个 `-` 是连字符（well-known），不动。
        let is_dash = seg.contains('—') || n >= 2;
        if is_dash {
            out.push(Edit {
                range: start..end,
                new: "——".to_string(),
                rule: "unify_dash",
            });
        }
    }
}

fn is_dash_char(c: char) -> bool {
    matches!(c, '-' | '—' | '－')
}

/// 可全角化的半角标点。
const PUNCT_MAP: &[(char, char)] = &[
    (',', '，'),
    ('.', '。'),
    ('?', '？'),
    ('!', '！'),
    (':', '：'),
    (';', '；'),
    ('(', '（'),
    (')', '）'),
];

fn full_width_of(c: char) -> Option<char> {
    PUNCT_MAP.iter().find(|(h, _)| *h == c).map(|(_, f)| *f)
}

/// 每个字符「**排版之后**是否会是 CJK」。
///
/// 为什么不能直接用 `is_cjk`：`punct_to_full_width` 会把挨着 CJK 的半角标点
/// 变成全角，而全角标点本身就是 CJK——于是它又能让**它的**邻居够格转换。
/// 这是个传递闭包，必须一次算到不动点。
///
/// 否则 `雪!!` 每跑一遍排版只转一个感叹号（第一遍 `雪！!`，第二遍 `雪！！`），
/// 永远不收敛；`雪!  「` 则是第一遍转标点、第二遍才删空格。两者 proptest 都抓到了。
fn effective_cjk_mask(chars: &[char], opts: &FormatOptions) -> Vec<bool> {
    let mut mask: Vec<bool> = chars.iter().map(|c| is_cjk(*c)).collect();
    if !opts.punct_to_full_width {
        return mask;
    }

    // 传播只沿着连续的可转标点进行，长度受限于文本，故必定收敛。
    loop {
        let mut changed = false;
        for i in 0..chars.len() {
            if mask[i] || full_width_of(chars[i]).is_none() {
                continue;
            }
            let left = i.checked_sub(1).is_some_and(|j| mask[j]);
            let right = mask.get(i + 1).copied().unwrap_or(false);
            if left || right {
                mask[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    mask
}

/// 半角标点→全角，**仅当前后至少一侧为 CJK**（保护 `v2.0`、`a, b`、URL）。
fn rule_punct_to_full_width(text: &str, out: &mut Vec<Edit>, mask: &[bool]) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, (off, c)) in chars.iter().enumerate() {
        let Some(full) = full_width_of(*c) else {
            continue;
        };
        // 看邻居的「最终」CJK 性，而非当前字面。
        let left = i.checked_sub(1).is_some_and(|j| mask[j]);
        let right = mask.get(i + 1).copied().unwrap_or(false);

        // 至少一侧是 CJK 才转。`v2.0` 两侧是数字 → 不转；
        // `a, b` 两侧是字母/空格 → 不转；URL 里的点同理。
        if !left && !right {
            continue;
        }
        out.push(Edit {
            range: *off..(*off + c.len_utf8()),
            new: full.to_string(),
            rule: "punct_to_full_width",
        });
    }
}

/// 直引号 → 弯引号。配对状态机，按段重置。
fn rule_unify_quotes(text: &str, out: &mut Vec<Edit>) {
    let mut offset = 0usize;
    for para in text.split_inclusive('\n') {
        unify_quotes_in_paragraph(para, offset, out);
        offset += para.len();
    }
}

fn unify_quotes_in_paragraph(para: &str, base: usize, out: &mut Vec<Edit>) {
    // 段内双引号必须成偶数个，否则**不动**（§6.5：拿不准就不动），
    // 由校对模块报 punct.unpaired_quote（M5）。
    let positions: Vec<usize> = para
        .char_indices()
        .filter(|(_, c)| *c == '"')
        .map(|(i, _)| i)
        .collect();
    if positions.len().is_multiple_of(2) {
        for (n, off) in positions.iter().enumerate() {
            out.push(Edit {
                range: (base + off)..(base + off + 1),
                new: if n.is_multiple_of(2) { "“" } else { "”" }.to_string(),
                rule: "unify_quotes",
            });
        }
    }

    // 单引号更危险：`don't` 里的撇号不是引号。只在两侧都不是字母时才当引号处理。
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    let quote_idx: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(i, (_, c))| {
            if *c != '\'' {
                return false;
            }
            let prev = i.checked_sub(1).and_then(|j| chars.get(j)).map(|(_, c)| *c);
            let next = chars.get(i + 1).map(|(_, c)| *c);
            // 词内撇号（don't、it's）放过。
            !(prev.is_some_and(|p| p.is_alphabetic()) && next.is_some_and(|n| n.is_alphabetic()))
        })
        .map(|(i, _)| i)
        .collect();

    if quote_idx.len().is_multiple_of(2) {
        for (n, i) in quote_idx.iter().enumerate() {
            let (off, c) = chars[*i];
            out.push(Edit {
                range: (base + off)..(base + off + c.len_utf8()),
                new: if n.is_multiple_of(2) { "‘" } else { "’" }.to_string(),
                rule: "unify_quotes",
            });
        }
    }
}

/// 中英/中数之间加空格。默认关。
fn rule_cjk_latin_space(text: &str, out: &mut Vec<Edit>, mask: &[bool]) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for i in 0..chars.len().saturating_sub(1) {
        let (_, a) = chars[i];
        let (bo, b) = chars[i + 1];

        // 两处预判都不能省：
        // - CJK 性用 mask（`a!「` 里的 `!` 排版后是 `！`，是 CJK）；
        // - 拉丁性要认全角（`ａ` 会被 full_width_digits 转成半角）。
        // 少任何一个，都是「第一遍转字符、第二遍才加空格」——幂等就破了。
        let a_cjk = mask[i];
        let b_cjk = mask[i + 1];
        let boundary = (a_cjk && is_latin_like(b)) || (is_latin_like(a) && b_cjk);
        if boundary {
            out.push(Edit {
                range: bo..bo,
                new: " ".to_string(),
                rule: "cjk_latin_space",
            });
        }
    }
}

/// 全角数字/字母 → 半角。
fn rule_full_width_digits(text: &str, out: &mut Vec<Edit>) {
    for (off, c) in text.char_indices() {
        let u = c as u32;
        // 全角 ！(FF01) 到 ～(FF5E) 中，只取数字与字母——
        // 全角标点是我们**想要**的形态，绝不能转回半角。
        let half = match u {
            0xFF10..=0xFF19 => Some(char::from_u32(u - 0xFF10 + 0x30)),
            0xFF21..=0xFF3A => Some(char::from_u32(u - 0xFF21 + 0x41)),
            0xFF41..=0xFF5A => Some(char::from_u32(u - 0xFF41 + 0x61)),
            _ => None,
        };
        if let Some(Some(h)) = half {
            out.push(Edit {
                range: off..(off + c.len_utf8()),
                new: h.to_string(),
                rule: "full_width_digits",
            });
        }
    }
}

/// 删除 CJK 字符之间的多余空格。
fn rule_strip_inline_space(text: &str, out: &mut Vec<Edit>, mask: &[bool]) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let (off, c) = chars[i];
        if c != ' ' {
            i += 1;
            continue;
        }
        // 找到极大的空格段。
        let start = i;
        let mut end = i;
        while end < chars.len() && chars[end].1 == ' ' {
            end += 1;
        }
        // 用「最终」CJK 性判断：`雪!  「` 里的 `!` 排版后是 `！`（CJK），
        // 这两个空格该在**这一遍**就删掉，而不是等下一遍。
        let prev_cjk = start.checked_sub(1).is_some_and(|j| mask[j]);
        let next_cjk = mask.get(end).copied().unwrap_or(false);

        // 只删「CJK 之间」的空格。段首缩进由 paragraph_indent 管，
        // 中英空格由 cjk_latin_space 管，这里都不插手。
        if prev_cjk && next_cjk {
            let last_off = chars[end - 1].0;
            out.push(Edit {
                range: off..(last_off + 1),
                new: String::new(),
                rule: "strip_inline_space",
            });
        }
        i = end;
    }
}

/// 重复标点压缩。默认关。
fn rule_repeat_punct(text: &str, out: &mut Vec<Edit>) {
    for (start, end, ch) in runs(text) {
        // 省略号与破折号有专门的规则（且优先级更高），此处放过，
        // 免得 `。。。` 被压成 `。` 而不是 `……`。
        if matches!(ch, '.' | '。' | '·' | '…' | '-' | '—' | '－') {
            continue;
        }
        if !is_repeatable_punct(ch) {
            continue;
        }
        // 段内全是同一个字符，故字符数 = 字节数 / 该字符的字节数——
        // 不必再切片，也就不必再自证边界。
        if (end - start) / ch.len_utf8() >= 2 {
            out.push(Edit {
                range: start..end,
                new: ch.to_string(),
                rule: "repeat_punct",
            });
        }
    }
}

fn is_repeatable_punct(c: char) -> bool {
    matches!(c, '！' | '？' | '，' | '；' | '：' | '!' | '?')
}

/// 合并段内软换行。默认关。
///
/// 用于导入外部文本：有些来源把一段拆成多行，每行末尾并无标点。
fn rule_line_join(text: &str, out: &mut Vec<Edit>) {
    let mut offset = 0usize;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();

    for (i, line) in lines.iter().enumerate() {
        let Some(content) = line.strip_suffix('\n') else {
            offset += line.len();
            continue;
        };
        let next = lines.get(i + 1);

        // 只在「本行非空、下一行非空」时合并：空行是段落边界，必须留着。
        let should_join = !content.trim().is_empty()
            && next.is_some_and(|n| !n.trim().is_empty())
            // 本行末尾已有句末标点 → 那是真的段落结束，不合并。
            && !content
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, '。' | '！' | '？' | '…' | '」' | '』' | '”'));

        if should_join {
            let nl = offset + content.len();
            out.push(Edit {
                range: nl..(nl + 1),
                new: String::new(),
                rule: "line_join",
            });
        }
        offset += line.len();
    }
}

// ============ 工具 ============

/// 文本中所有「满足 `pred` 的字符的极大连续段」，返回 (起点, 终点)。
///
/// 与 `runs` 的区别：这里同一段内可以是**不同**字符（只要都属于同一类）。
/// 省略号、破折号必须这样匹配——`。。。...` 是一个省略号，不是两个。
/// 连同该段的文本一起返回，调用方就不必自己对正文做字节切片
/// （那既触发 §0 禁令 5 的检查，也把「凭什么落在边界上」的问题推给每个调用点）。
/// 这里的偏移全部来自 `char_indices`，边界性由构造保证。
fn class_runs(text: &str, pred: fn(char) -> bool) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut end = 0usize;

    for (off, c) in text.char_indices() {
        if pred(c) {
            if start.is_none() {
                start = Some(off);
            }
            end = off + c.len_utf8();
        } else if let Some(s) = start.take()
            && let Some(seg) = text.get(s..end)
        {
            out.push((s, end, seg));
        }
    }
    if let Some(s) = start
        && let Some(seg) = text.get(s..end)
    {
        out.push((s, end, seg));
    }
    out
}

/// 文本中所有「同一字符的极大连续段」，返回 (起点, 终点, 该字符)。
fn runs(text: &str) -> Vec<(usize, usize, char)> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((start, c)) = it.next() {
        let mut end = start + c.len_utf8();
        while let Some((off, n)) = it.peek() {
            if *n != c {
                break;
            }
            end = off + n.len_utf8();
            it.next();
        }
        out.push((start, end, c));
    }
    out
}

/// 是否是 CJK 表意文字或中文标点。
///
/// **全角数字与字母不算 CJK**——它们是穿着全角外衣的拉丁字符，
/// 且 `full_width_digits` 会把它们转回半角。若把 `２` 当成 CJK，
/// `雪  !２０２６` 里的 `!` 会因「邻居是 CJK」被全角化，可最终文本里
/// 它的邻居是 `2`，根本不是 CJK——于是第二遍排版又要改回来，幂等就破了。
/// （proptest 正是用这个输入把它打出来的。）
fn is_cjk(c: char) -> bool {
    // 全角空格（U+3000）不算 CJK：它是空白（类别 Zs），是段首缩进符。
    // 「至少一侧是 CJK」的本意是「挨着中文字」，不是「挨着空白」。
    // 若把它算进去，`(!` 排版成 `　　(!` 之后，第二遍就会因为
    // 「左邻是 U+3000」把 `(` 转成 `（`——又一处幂等破口，
    // 同样是 proptest 抓到的。
    if c == '\u{3000}' {
        return false;
    }
    let u = c as u32;
    (0x4E00..=0x9FFF).contains(&u)          // 统一汉字
        || (0x3400..=0x4DBF).contains(&u)   // 扩展 A
        || (0x20000..=0x2EBEF).contains(&u) // 扩展 B+
        || (0x3001..=0x303F).contains(&u)   // CJK 标点（。、「」），跳过 U+3000
        // 全角标点（！，（）：），但**不含**全角数字/字母。
        || (0xFF01..=0xFF0F).contains(&u)
        || (0xFF1A..=0xFF20).contains(&u)
        || (0xFF3B..=0xFF40).contains(&u)
        || (0xFF5B..=0xFF65).contains(&u)
}

/// 拉丁字母或数字（含全角形式）。
fn is_latin_like(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c as u32, 0xFF10..=0xFF19 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A)
}

/// 是否是标点（供外部判断）。
pub fn is_punctuation(c: char) -> bool {
    matches!(
        get_general_category(c),
        G::ConnectorPunctuation
            | G::DashPunctuation
            | G::ClosePunctuation
            | G::FinalPunctuation
            | G::InitialPunctuation
            | G::OtherPunctuation
            | G::OpenPunctuation
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 只开一条规则跑排版。
    fn only(f: impl FnOnce(&mut FormatOptions)) -> FormatOptions {
        let mut o = FormatOptions::none();
        f(&mut o);
        o
    }

    // ---- trim_trailing ----

    #[test]
    fn trims_trailing_whitespace() {
        let o = only(|o| o.trim_trailing = true);
        assert_eq!(format("雪落了  \n他推开门\t\n", &o), "雪落了\n他推开门\n");
    }

    #[test]
    fn trims_fullwidth_space_at_line_end() {
        let o = only(|o| o.trim_trailing = true);
        assert_eq!(format("雪落了　\n", &o), "雪落了\n");
    }

    // ---- collapse_blank ----

    #[test]
    fn collapses_consecutive_blank_lines() {
        let o = only(|o| o.collapse_blank = true);
        assert_eq!(format("第一段\n\n\n\n第二段", &o), "第一段\n\n第二段");
    }

    #[test]
    fn keeps_single_blank_line() {
        let o = only(|o| o.collapse_blank = true);
        assert_eq!(format("第一段\n\n第二段", &o), "第一段\n\n第二段");
    }

    /// 带空格的「空行」也算空行——真实稿子里它们随处可见。
    #[test]
    fn collapses_whitespace_only_lines() {
        let o = only(|o| o.collapse_blank = true);
        assert_eq!(format("第一段\n  \n\t\n第二段", &o), "第一段\n\n第二段");
    }

    // ---- paragraph_indent ----

    #[test]
    fn indents_paragraphs_with_two_fullwidth_spaces() {
        let o = only(|o| o.paragraph_indent = ParagraphIndent::FullWidthTwo);
        assert_eq!(format("雪落了一夜。\n", &o), "　　雪落了一夜。\n");
    }

    #[test]
    fn replaces_wrong_indent() {
        let o = only(|o| o.paragraph_indent = ParagraphIndent::FullWidthTwo);
        assert_eq!(format("    雪落了。\n", &o), "　　雪落了。\n");
        assert_eq!(
            format("　雪落了。\n", &o),
            "　　雪落了。\n",
            "一个全角空格应补成两个"
        );
    }

    #[test]
    fn indent_none_strips_leading_space() {
        let o = only(|o| o.paragraph_indent = ParagraphIndent::None);
        assert_eq!(format("　　雪落了。\n", &o), "雪落了。\n");
    }

    #[test]
    fn indent_keep_does_nothing() {
        let o = only(|o| o.paragraph_indent = ParagraphIndent::Keep);
        assert_eq!(format("    雪落了。\n", &o), "    雪落了。\n");
    }

    /// 空行不该被缩进——那会凭空造出一行全角空格。
    #[test]
    fn does_not_indent_blank_lines() {
        let o = only(|o| o.paragraph_indent = ParagraphIndent::FullWidthTwo);
        assert_eq!(
            format("雪落了。\n\n他推门。\n", &o),
            "　　雪落了。\n\n　　他推门。\n"
        );
    }

    // ---- unify_ellipsis ----

    #[test]
    fn unifies_ellipsis_forms() {
        let o = only(|o| o.unify_ellipsis = true);
        assert_eq!(format("他说...", &o), "他说……");
        assert_eq!(format("他说。。。", &o), "他说……");
        assert_eq!(format("他说···", &o), "他说……");
        assert_eq!(format("他说…", &o), "他说……");
    }

    /// `……` 已是目标形态，不该被再次加长——这是幂等的关键。
    #[test]
    fn ellipsis_already_correct_is_untouched() {
        let o = only(|o| o.unify_ellipsis = true);
        assert_eq!(format("他说……", &o), "他说……");
    }

    /// 单个句号不是省略号。
    #[test]
    fn single_period_is_not_ellipsis() {
        let o = only(|o| o.unify_ellipsis = true);
        assert_eq!(format("雪落了。", &o), "雪落了。");
        assert_eq!(format("v2.0", &o), "v2.0");
    }

    // ---- unify_dash ----

    #[test]
    fn unifies_dash_forms() {
        let o = only(|o| o.unify_dash = true);
        assert_eq!(format("他说--", &o), "他说——");
        assert_eq!(format("他说—", &o), "他说——");
        assert_eq!(format("他说——", &o), "他说——", "已正确则不动");
    }

    /// 单个连字符是 hyphen，不是破折号。
    #[test]
    fn single_hyphen_is_untouched() {
        let o = only(|o| o.unify_dash = true);
        assert_eq!(format("well-known", &o), "well-known");
    }

    // ---- punct_to_full_width ----

    #[test]
    fn converts_punct_next_to_cjk() {
        let o = only(|o| o.punct_to_full_width = true);
        assert_eq!(format("雪落了,他推门.", &o), "雪落了，他推门。");
        assert_eq!(format("真的?是的!", &o), "真的？是的！");
    }

    /// 版本号、英文句子、URL 不该被全角化（§6.5 明言）。
    #[test]
    fn protects_non_cjk_contexts() {
        let o = only(|o| o.punct_to_full_width = true);
        assert_eq!(format("v2.0", &o), "v2.0");
        assert_eq!(format("a, b", &o), "a, b");
        assert_eq!(format("http://x.com/a?b=1", &o), "http://x.com/a?b=1");
    }

    /// 一侧是 CJK 就转——这是规则的定义。
    #[test]
    fn converts_when_one_side_is_cjk() {
        let o = only(|o| o.punct_to_full_width = true);
        assert_eq!(format("雪,a", &o), "雪，a");
    }

    // ---- unify_quotes ----

    #[test]
    fn unifies_paired_double_quotes() {
        let o = only(|o| o.unify_quotes = true);
        assert_eq!(format("他说\"你来了\"。", &o), "他说“你来了”。");
    }

    /// 孤立引号不动（§6.5：拿不准就不动）。
    #[test]
    fn leaves_unpaired_quotes_alone() {
        let o = only(|o| o.unify_quotes = true);
        assert_eq!(format("他说\"你来了。", &o), "他说\"你来了。");
    }

    /// 英文缩写里的撇号不是引号。
    #[test]
    fn apostrophe_in_word_is_untouched() {
        let o = only(|o| o.unify_quotes = true);
        assert_eq!(format("don't stop", &o), "don't stop");
    }

    /// 引号配对按段重置——跨段配对会把两段的引号错配。
    #[test]
    fn quote_pairing_resets_per_paragraph() {
        let o = only(|o| o.unify_quotes = true);
        // 每段各一个引号 → 段内都是奇数 → 都不动。
        assert_eq!(format("他说\"甲\n乙\"完了", &o), "他说\"甲\n乙\"完了");
    }

    // ---- full_width_digits ----

    #[test]
    fn converts_fullwidth_alnum_to_halfwidth() {
        let o = only(|o| o.full_width_digits = true);
        assert_eq!(format("２０２６年", &o), "2026年");
        assert_eq!(format("ＡＢＣ", &o), "ABC");
        assert_eq!(format("ａｂｃ", &o), "abc");
    }

    /// 全角标点是我们想要的形态，绝不能被转回半角。
    #[test]
    fn does_not_convert_fullwidth_punctuation() {
        let o = only(|o| o.full_width_digits = true);
        assert_eq!(format("雪落了。「你来了」", &o), "雪落了。「你来了」");
    }

    // ---- strip_inline_space ----

    #[test]
    fn strips_space_between_cjk() {
        let o = only(|o| o.strip_inline_space = true);
        assert_eq!(format("雪 落 了", &o), "雪落了");
        assert_eq!(format("雪  落", &o), "雪落", "多个空格一并删");
    }

    /// 中英之间的空格不归这条规则管。
    #[test]
    fn keeps_space_between_cjk_and_latin() {
        let o = only(|o| o.strip_inline_space = true);
        assert_eq!(format("雪 a 落", &o), "雪 a 落");
    }

    // ---- repeat_punct（默认关）----

    #[test]
    fn collapses_repeated_punct_when_enabled() {
        let o = only(|o| o.repeat_punct = true);
        assert_eq!(format("真的！！！", &o), "真的！");
        assert_eq!(format("什么？？", &o), "什么？");
    }

    #[test]
    fn repeat_punct_is_off_by_default() {
        assert!(!FormatOptions::default().repeat_punct);
    }

    /// 省略号有专门规则，不该被 repeat_punct 压成一个句号。
    #[test]
    fn repeat_punct_leaves_ellipsis_to_its_own_rule() {
        let o = {
            let mut o = FormatOptions::none();
            o.repeat_punct = true;
            o.unify_ellipsis = true;
            o
        };
        assert_eq!(format("他说。。。", &o), "他说……");
    }

    // ---- line_join（默认关）----

    #[test]
    fn joins_soft_wrapped_lines_when_enabled() {
        let o = only(|o| o.line_join = true);
        assert_eq!(
            format("雪落了一夜\n他推开门。\n", &o),
            "雪落了一夜他推开门。\n"
        );
    }

    #[test]
    fn line_join_keeps_paragraph_breaks() {
        let o = only(|o| o.line_join = true);
        assert_eq!(
            format("第一段。\n\n第二段。\n", &o),
            "第一段。\n\n第二段。\n"
        );
    }

    // ---- 冲突裁决 ----

    /// `雪...` 上省略号（优先级 3）应压过标点全角化（优先级 5）。
    #[test]
    fn higher_priority_rule_wins_overlap() {
        let o = {
            let mut o = FormatOptions::none();
            o.unify_ellipsis = true;
            o.punct_to_full_width = true;
            o
        };
        assert_eq!(format("雪...", &o), "雪……", "不该变成 雪。。。");
    }

    #[test]
    fn plan_edits_never_overlap() {
        let text = "  雪 落 了...  \n\n\n他说\"来了\"２０２６  \n";
        let edits = plan(text, &FormatOptions::default());
        for w in edits.windows(2) {
            assert!(
                w[0].range.end <= w[1].range.start,
                "编辑重叠: {:?} 与 {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn plan_edits_are_sorted() {
        let text = "雪 落...  \n\n\n他说";
        let edits = plan(text, &FormatOptions::default());
        let mut prev = 0;
        for e in &edits {
            assert!(e.range.start >= prev, "未排序: {edits:?}");
            prev = e.range.start;
        }
    }

    /// 空操作不该出现在预览里——「3 处改动」里有 2 处没改会让人以为程序在骗他。
    #[test]
    fn plan_omits_noop_edits() {
        let text = "　　雪落了一夜。\n";
        let edits = plan(text, &FormatOptions::default());
        for e in &edits {
            assert_ne!(text.get(e.range.clone()).unwrap(), e.new, "空操作: {e:?}");
        }
    }

    // ---- apply ----

    #[test]
    fn apply_handles_multiple_edits_in_order() {
        let text = "abc";
        let edits = vec![
            Edit {
                range: 0..1,
                new: "X".into(),
                rule: "t",
            },
            Edit {
                range: 2..3,
                new: "Z".into(),
                rule: "t",
            },
        ];
        assert_eq!(apply(text, &edits), "XbZ");
    }

    /// 编辑列表与文本对不上时跳过而非 panic——排版失败顶多没排上，
    /// 不该把正文搞坏。
    #[test]
    fn apply_ignores_out_of_range_edits() {
        let edits = vec![Edit {
            range: 100..200,
            new: "X".into(),
            rule: "t",
        }];
        assert_eq!(apply("abc", &edits), "abc");
    }

    // ---- 综合 ----

    #[test]
    fn realistic_manuscript() {
        let text = "雪落了一夜...  \n\n\n\n他推开门,风灌进来!  \n";
        let got = format(text, &FormatOptions::default());
        assert_eq!(got, "　　雪落了一夜……\n\n　　他推开门，风灌进来！\n");
    }

    #[test]
    fn empty_text_is_unchanged() {
        assert_eq!(format("", &FormatOptions::default()), "");
    }

    /// 以下全是 proptest 挖出来的反例，手写用例一个都没覆盖到。留作回归。
    #[test]
    fn idempotence_regressions_found_by_proptest() {
        let o = FormatOptions::default();
        let cases = [
            // apply 只验了 range.start 的边界，`--` 让段首缩进的插入与
            // 破折号的替换撞在同一起点，切进了全角空格的字节中间 → panic。
            "--",
            // 全角数字曾被当成 CJK，`!` 因「邻居是 ２」被全角化，
            // 可 ２ 最终会变成 2，根本不是 CJK。
            "雪  !２０２６",
            // 段首缩进的 U+3000 曾被当成 CJK，于是第二遍把 `(` 转成了 `（`。
            "(!",
            // 两段省略号各自变 `……`，拼起来又成了一段更长的省略号。
            "。。。...",
            // `!` 变 `！` 之后才是 CJK，空格要到第二遍才被删。
            "雪!  「",
            // trim 与 collapse 抢同一段空白，trim 赢了却只删了空格。
            " \n\n",
            // 混合段：`。` 是真句号，不能被后面的省略号吞掉。
            "。 。。。",
            // 删空格 + 标点全角化，联合造出一个 `。。。`——
            // 而那正是省略号规则的输入，单遍扫描永远看不见。
            "。  .。-",
            // diff 脚本被照字面读，生成了互相重叠的编辑，正文直接错乱。
            "２０２６　雪雪——雪",
        ];

        for text in cases {
            let once = format(text, &o);
            let twice = format(&once, &o);
            assert_eq!(once, twice, "{text:?} 排版不幂等");
            assert!(
                plan(&once, &o).is_empty(),
                "{text:?} 排完还有改动: {:?}",
                plan(&once, &o)
            );
        }
    }

    /// `。……` 里的句号是真句号（「他说。……然后走了」），不该被吞掉。
    #[test]
    fn mixed_ellipsis_run_is_left_alone() {
        let o = only(|o| o.unify_ellipsis = true);
        assert_eq!(format("他说。……", &o), "他说。……");
    }

    /// 预览所见即所得：plan 的结果 apply 之后必须等于 format 的结果。
    #[test]
    fn plan_matches_format_output() {
        let o = FormatOptions::default();
        for text in [
            "雪  !２０２６",
            "。  .。-",
            "２０２６　雪雪——雪",
            "  雪...  \n\n\n",
        ] {
            assert_eq!(apply(text, &plan(text, &o)), format(text, &o), "{text:?}");
        }
    }

    #[test]
    fn already_formatted_text_is_unchanged() {
        let text = "　　雪落了一夜。\n\n　　他推开门，风裹着雪灌进来。\n";
        assert_eq!(
            format(text, &FormatOptions::default()),
            text,
            "已排好的不该再动"
        );
        assert!(
            plan(text, &FormatOptions::default()).is_empty(),
            "不该有改动"
        );
    }
}
