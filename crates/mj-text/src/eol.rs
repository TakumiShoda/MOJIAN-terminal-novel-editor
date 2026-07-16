//! 行尾（EOL）归一化。见 doc.md §9 跨平台。
//!
//! 契约：**读入时统一 LF，写出时按配置**。
//!
//! 为什么内存里必须只有 LF：正文的字节偏移贯穿全项目——光标位置、`Edit::range`、
//! 校对 `Issue::range`、diff hunk、搜索命中。若内存里混着 CRLF，同一段文字在
//! Windows 和 macOS 上的偏移就不同，所有跨平台的位置计算全部错位。
//! 因此 CRLF 只存在于「刚读进来的字节」和「即将写出的字节」这两个瞬间。

use serde::{Deserialize, Serialize};

/// 写出时使用的行尾。读入永远归一化到 LF，故此枚举只影响写出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    /// 始终写 LF。对 git 友好，跨平台一致——这是默认。
    #[default]
    Lf,
    /// 按当前平台：Windows 写 CRLF，其余写 LF。
    Native,
}

impl LineEnding {
    /// 本枚举对应的实际行尾字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Native => {
                if cfg!(windows) {
                    "\r\n"
                } else {
                    "\n"
                }
            }
        }
    }
}

/// 归一化到 LF。读入正文时必须先过这一道。
///
/// 处理三种行尾：CRLF（Windows）、CR（老式 Mac / 某些导入文本）、LF。
/// 孤立的 CR 也归一化——它出现在正文里几乎必然是行尾，而非用户想要的字符。
pub fn normalize(text: &str) -> String {
    // 绝大多数正文不含 CR：先探测，无则零拷贝返回。
    if !text.contains('\r') {
        return text.to_owned();
    }

    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            // CRLF 吞掉 LF，孤立 CR 也算一个换行。
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// 是否需要归一化。用于避免无谓的分配。
pub fn needs_normalize(text: &str) -> bool {
    text.contains('\r')
}

/// 按指定行尾写出。输入必须是已归一化（只含 LF）的文本。
///
/// 目标为 LF 时零拷贝——这是默认路径，不该有成本。
pub fn denormalize(text: &str, eol: LineEnding) -> std::borrow::Cow<'_, str> {
    match eol.as_str() {
        "\n" => std::borrow::Cow::Borrowed(text),
        crlf => std::borrow::Cow::Owned(text.replace('\n', crlf)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_crlf() {
        assert_eq!(normalize("第一行\r\n第二行"), "第一行\n第二行");
    }

    #[test]
    fn normalizes_lone_cr() {
        assert_eq!(normalize("第一行\r第二行"), "第一行\n第二行");
    }

    #[test]
    fn leaves_lf_untouched() {
        assert_eq!(normalize("第一行\n第二行"), "第一行\n第二行");
    }

    #[test]
    fn handles_mixed_endings() {
        assert_eq!(normalize("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn handles_consecutive_crlf() {
        assert_eq!(normalize("a\r\n\r\nb"), "a\n\nb");
    }

    #[test]
    fn preserves_cjk_and_fullwidth_space() {
        assert_eq!(
            normalize("　　雪落了一夜。\r\n　　他推开门。"),
            "　　雪落了一夜。\n　　他推开门。"
        );
    }

    #[test]
    fn detects_need_for_normalize() {
        assert!(needs_normalize("a\r\nb"));
        assert!(!needs_normalize("a\nb"));
    }

    #[test]
    fn denormalize_lf_is_borrowed() {
        let out = denormalize("a\nb", LineEnding::Lf);
        assert!(
            matches!(out, std::borrow::Cow::Borrowed(_)),
            "LF 路径不应分配"
        );
        assert_eq!(out, "a\nb");
    }

    /// 归一化 → 反归一化 → 再归一化，必须回到原点。
    /// 这条保证了「跨平台打开同一份稿子不会累积垃圾」。
    #[test]
    fn roundtrip_is_stable() {
        let original = "　　雪落了一夜。\n　　他推开门，风裹着雪灌进来。\n";
        for eol in [LineEnding::Lf, LineEnding::Native] {
            let written = denormalize(original, eol);
            let read_back = normalize(&written);
            assert_eq!(read_back, original, "{eol:?} 往返后应回到原文");
        }
    }

    /// 归一化必须幂等：已经是 LF 的文本再过一遍不变。
    #[test]
    fn normalize_is_idempotent() {
        let inputs = ["a\r\nb", "a\rb", "a\nb", "", "\r\n", "　　中文\r\n"];
        for i in inputs {
            let once = normalize(i);
            let twice = normalize(&once);
            assert_eq!(once, twice, "输入 {i:?} 的归一化不幂等");
        }
    }
}
