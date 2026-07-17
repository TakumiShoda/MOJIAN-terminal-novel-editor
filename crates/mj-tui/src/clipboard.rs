//! 剪贴板。见 doc.md §3。
//!
//! §3 的三级降级：**优先 OSC 52**（支持 SSH 场景），失败时降级到系统剪贴板
//! crate，再失败则内部剪贴板 `[SHOULD]`。
//!
//! M4 只实装 OSC 52 —— 它恰恰是最要紧的一级：这是个终端写作器，
//! 用户很可能正 ssh 在服务器上写。系统剪贴板 crate 在那种场景下反而没用
//! （它操作的是服务器的剪贴板，不是用户面前那台机器的）。
//!
//! 后两级留待 M6：`[SHOULD]` 而非 `[MUST]`，且要引新依赖。

use std::io::Write as _;

/// 把文本送进剪贴板。
///
/// OSC 52 是「写给终端」的转义序列：终端替我们把内容放进宿主机的剪贴板。
/// 对不支持的终端无副作用——它们会忽略未知的 OSC 序列。
///
/// **不返回 Result**：终端不会回话，我们无从知道它到底放没放进去。
/// 与其返回一个永远是 Ok 的 Result 骗调用方，不如老实说这是「尽力而为」。
pub fn copy(text: &str) {
    let payload = base64_encode(text.as_bytes());
    let mut out = std::io::stdout();
    // OSC 52 ; c（clipboard）; <base64> BEL
    let _ = write!(out, "\x1b]52;c;{payload}\x07");
    let _ = out.flush();
}

/// base64 编码。
///
/// 不引 base64 crate：只为这一处几行的编码多一条依赖不值当，
/// 而 OSC 52 的载荷格式是固定的标准 base64，不会变。
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;

        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        // 不足 3 字节时补 `=`。
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_known_vectors() {
        // RFC 4648 的测试向量。
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// 中文是多字节的，编码的是**字节**不是字符。
    #[test]
    fn encodes_cjk() {
        // 雪 = E9 9B AA
        assert_eq!(base64_encode("雪".as_bytes()), "6Zuq");
        // 能编码即可——重点是不 panic、长度正确。
        let s = base64_encode("　　雪落了一夜。".as_bytes());
        assert!(s.len().is_multiple_of(4), "base64 长度应是 4 的倍数: {s}");
    }

    #[test]
    fn encodes_emoji() {
        let s = base64_encode("👨‍👩‍👧".as_bytes());
        assert!(s.len().is_multiple_of(4));
        assert!(!s.is_empty());
    }

    /// 输出长度必须是 4 的倍数（补 `=` 补到位）。
    #[test]
    fn output_length_is_always_a_multiple_of_four() {
        for n in 0..20 {
            let data = vec![b'x'; n];
            let s = base64_encode(&data);
            assert!(
                s.len().is_multiple_of(4),
                "n={n} 时长度 {} 不是 4 的倍数",
                s.len()
            );
        }
    }

    #[test]
    fn copy_does_not_panic() {
        copy("雪落了一夜");
        copy("");
    }
}
