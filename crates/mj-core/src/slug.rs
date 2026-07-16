//! 由用户标题生成跨平台安全的目录/文件名。见 doc.md §5.1、§9。
//!
//! 磁盘上的目录名形如 `010-diyi-juan`、`0010-kaipian`：排序号 + slug。
//! **slug 仅供人眼**——真相在 toml 里的 id 与 title（§5.1）。所以 slug 可以有损，
//! 但绝不能生成一个在某个平台上创建失败的名字：那会让「新建章节」在 Windows 上
//! 随机报错，而用户只是给章节起了个叫「第一章：雪夜」的名字。
//!
//! Windows 的限制远严于 Unix：
//! - 保留字符 `< > : " / \ | ? *` 与 0x00-0x1F
//! - 保留设备名 CON / PRN / AUX / NUL / COM1-9 / LPT1-9（含带扩展名的形式）
//! - 结尾不得为空格或点
//!
//! 我们对所有平台统一施加最严格的规则，这样同一份 workspace 能在平台间搬运。

/// 单个路径分量的最大字节数。
///
/// 多数文件系统限制 255 字节（不是字符）。中文一字 3 字节，留出排序号前缀与
/// `.md` 后缀的余量，取 100 字节——章节名再长也没有可读性收益。
const MAX_SLUG_BYTES: usize = 100;

/// Windows 保留设备名。比较时忽略大小写，且忽略扩展名（`CON.md` 同样非法）。
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 把用户标题转成跨平台安全的路径分量。
///
/// 保留中文（现代文件系统均支持，且 `第一章-雪夜` 比 `diyi-zhang` 可读得多），
/// 只替换真正危险的字符。空标题或全是非法字符时回退为 `untitled`。
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_was_sep = false;

    for c in title.chars() {
        let mapped = match c {
            // Windows 保留字符 + 路径分隔符。
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => Some('-'),
            // 控制字符。
            c if (c as u32) < 0x20 || c as u32 == 0x7F => Some('-'),
            // 空白（含全角空格 U+3000）统一为连字符：文件名里的空格在 shell 与
            // 脚本中都是麻烦。
            c if c.is_whitespace() => Some('-'),
            c => {
                last_was_sep = false;
                out.push(c);
                None
            }
        };
        // 连续的分隔符压成一个，避免 `第一章---雪夜`。
        if let Some(sep) = mapped
            && !last_was_sep
        {
            out.push(sep);
            last_was_sep = true;
        }
    }

    // 结尾的空格与点在 Windows 上会被静默剥离，导致文件名与预期不符。
    let trimmed = out.trim_matches(|c: char| c == '-' || c == '.' || c.is_whitespace());
    let mut result = truncate_at_char_boundary(trimmed, MAX_SLUG_BYTES).to_owned();

    // 截断后可能又露出结尾的分隔符。
    result = result.trim_end_matches(['-', '.']).to_owned();

    if result.is_empty() || is_reserved(&result) {
        // 保留名前缀下划线即可规避，同时保留可读性。
        return if result.is_empty() {
            "untitled".to_owned()
        } else {
            format!("_{result}")
        };
    }
    result
}

/// 是否为 Windows 保留设备名（忽略大小写与扩展名）。
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// 按字节上限截断，但不切开 UTF-8 字符。
///
/// 直接 `&s[..n]` 会在中文中间切断并 panic（doc.md §0 禁令 5 的同源问题）。
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    #[allow(clippy::string_slice)] // 上面已确保 end 落在字符边界。
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_chinese_readable() {
        assert_eq!(slugify("第一章 雪夜"), "第一章-雪夜");
        assert_eq!(slugify("雪夜行"), "雪夜行");
    }

    /// Windows 保留字符必须替换，否则新建章节在 Windows 上直接失败。
    #[test]
    fn replaces_windows_reserved_chars() {
        assert_eq!(slugify("第一章:雪夜"), "第一章-雪夜");
        assert_eq!(slugify("a/b\\c"), "a-b-c");
        assert_eq!(slugify("what?"), "what");
        assert_eq!(slugify("a<b>c|d*e\"f"), "a-b-c-d-e-f");
    }

    /// 全角标点在各平台文件名里都合法，且比替换成连字符更可读——
    /// slug 是给人眼看的（doc.md §5.1），故保留。
    /// 只有 ASCII 的 `:` `?` 等才是 Windows 的雷。
    #[test]
    fn keeps_fullwidth_punctuation() {
        assert_eq!(slugify("第一章：雪夜"), "第一章：雪夜");
        assert_eq!(slugify("你在哪？"), "你在哪？");
        assert_eq!(slugify("《雪夜行》"), "《雪夜行》");
    }

    #[test]
    fn collapses_consecutive_separators() {
        assert_eq!(slugify("第一章::雪夜"), "第一章-雪夜");
        assert_eq!(slugify("a   b"), "a-b");
    }

    /// 全角空格是中文正文里的缩进符，出现在标题里也要处理。
    #[test]
    fn handles_fullwidth_space() {
        assert_eq!(slugify("第一章　雪夜"), "第一章-雪夜");
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        // Windows 会静默剥离结尾的点与空格。
        assert_eq!(slugify("第一章..."), "第一章");
        assert_eq!(slugify("第一章   "), "第一章");
    }

    #[test]
    fn escapes_windows_device_names() {
        assert_eq!(slugify("CON"), "_CON");
        assert_eq!(slugify("con"), "_con");
        assert_eq!(slugify("NUL"), "_NUL");
        assert_eq!(slugify("COM1"), "_COM1");
        // 带扩展名的形式同样非法。
        assert_eq!(slugify("con.md"), "_con.md");
        // 但只是「以保留名开头」不算。
        assert_eq!(slugify("CONTENTS"), "CONTENTS");
    }

    #[test]
    fn falls_back_for_empty_input() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("   "), "untitled");
        assert_eq!(slugify("///"), "untitled");
    }

    /// 截断不得切开中文字符——否则文件名是乱码，甚至不是合法 UTF-8。
    #[test]
    fn truncates_without_splitting_chars() {
        let long = "雪".repeat(200); // 600 字节
        let s = slugify(&long);
        assert!(
            s.len() <= MAX_SLUG_BYTES,
            "应截断到 {MAX_SLUG_BYTES} 字节内"
        );
        assert!(s.chars().all(|c| c == '雪'), "不应出现乱码: {s}");
        // 能被正常解释为字符串即证明未切断（String 本身保证 UTF-8 合法）。
        assert_eq!(s.len() % 3, 0, "中文 3 字节，截断应落在字符边界");
    }

    /// 生成的名字必须不含任何平台的非法字符。
    #[test]
    fn output_is_always_path_safe() {
        let long = "很长的标题".repeat(50);
        let inputs = [
            "第一章：雪夜",
            "a/b",
            "CON",
            "",
            "...",
            "a\0b",
            "tab\there",
            long.as_str(),
        ];
        for i in inputs {
            let s = slugify(i);
            assert!(!s.is_empty(), "{i:?} 产出空名");
            assert!(
                !s.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']),
                "{i:?} -> {s:?} 含非法字符"
            );
            assert!(
                !s.chars().any(|c| (c as u32) < 0x20),
                "{i:?} -> {s:?} 含控制字符"
            );
            assert!(
                !s.ends_with('.') && !s.ends_with(' '),
                "{i:?} -> {s:?} 结尾非法"
            );
            assert!(!is_reserved(&s), "{i:?} -> {s:?} 是保留设备名");
        }
    }
}
