//! EOL 归一化的属性测试。见 doc.md §10。
//!
//! 手写用例只能覆盖想得到的组合。行尾处理的坑恰恰在想不到的地方：
//! 连续 CR、CR 在末尾、CRLF 被拆开、中文之间夹 CR……交给 proptest 穷举。

use proptest::prelude::*;

use mj_text::eol::{LineEnding, denormalize, normalize};

/// 生成含各种行尾与中文的随机文本。
fn text_with_line_endings() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("\r\n".to_string()),
            Just("\r".to_string()),
            Just("\n".to_string()),
            Just("　　".to_string()), // 全角空格（段首缩进）
            Just("雪".to_string()),
            Just("a".to_string()),
            Just("。".to_string()),
            Just("".to_string()),
        ],
        0..40,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    /// 归一化幂等：normalize(normalize(x)) == normalize(x)。
    #[test]
    fn normalize_is_idempotent(s in text_with_line_endings()) {
        let once = normalize(&s);
        let twice = normalize(&once);
        prop_assert_eq!(once, twice);
    }

    /// 归一化后不含任何 CR——这是内存表示的铁律（偏移量一致性依赖它）。
    #[test]
    fn normalized_text_has_no_cr(s in text_with_line_endings()) {
        prop_assert!(!normalize(&s).contains('\r'));
    }

    /// 往返稳定：LF 文本 -> 写出 -> 读回，必须逐字节回到原文。
    /// 保证同一份稿子在 Windows 与 macOS 间来回打开不会漂移。
    #[test]
    fn roundtrip_preserves_normalized_text(s in text_with_line_endings()) {
        let normalized = normalize(&s);
        for eol in [LineEnding::Lf, LineEnding::Native] {
            let written = denormalize(&normalized, eol);
            let read_back = normalize(&written);
            prop_assert_eq!(&read_back, &normalized, "{:?} 往返漂移", eol);
        }
    }

    /// 归一化不改变行数——只换行尾表示，不增删行。
    #[test]
    fn normalize_preserves_line_count(s in text_with_line_endings()) {
        // 原文的行数：CRLF 算一个换行，孤立 CR 也算一个。
        let expected = {
            let mut n = 0usize;
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\r' => {
                        if chars.peek() == Some(&'\n') { chars.next(); }
                        n += 1;
                    }
                    '\n' => n += 1,
                    _ => {}
                }
            }
            n
        };
        prop_assert_eq!(normalize(&s).matches('\n').count(), expected);
    }

    /// 归一化只动行尾，不碰其他字符。
    #[test]
    fn normalize_preserves_non_eol_chars(s in text_with_line_endings()) {
        let strip = |t: &str| t.chars().filter(|c| *c != '\r' && *c != '\n').collect::<String>();
        prop_assert_eq!(strip(&normalize(&s)), strip(&s));
    }
}
