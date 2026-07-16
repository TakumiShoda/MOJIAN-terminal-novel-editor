//! 稳定 ID。见 doc.md §5.1、§5.3。
//!
//! 契约：**创建后永不变更**。重命名、移动、排序都不影响它。
//! 这是整个存储层的地基——文件路径会变，order 会变，标题会变，只有 id 不变。
//!
//! 形态：8 位 base32（Crockford 字母表，无 I/L/O/U，避免与 1/0 混淆）。
//! 8 位 base32 = 40 bit 随机，约 1.1e12 种。单本书的章节量级（1e3）下，
//! 生日碰撞概率约 4e-7，可忽略；且 `Store` 在创建时还会做一次实际存在性检查。

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Crockford base32：剔除 I、L、O、U，避免人眼与 1/0 混淆。
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// base32 字符数。与 doc.md §5.1「8位 base32 随机」一致。
const LEN: usize = 8;

/// 随机位数：每字符 5 bit。
const BITS: u32 = (LEN * 5) as u32;

pub trait Tag {
    /// 文本形态的前缀，如 `ch_7Q2M4KZA` 里的 `ch`。
    ///
    /// 前缀让 id 自带类型信息：日志里看到 `cr_...` 就知道是角色而非章节，
    /// 且能挡住「把 VolumeId 传进要 ChapterId 的地方」这类错误在数据层扩散。
    const PREFIX: &'static str;
}

macro_rules! define_tag {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name;
        impl Tag for $name {
            const PREFIX: &'static str = $prefix;
        }
    };
}

define_tag!(BookTag, "bk");
define_tag!(VolumeTag, "vo");
define_tag!(ChapterTag, "ch");
define_tag!(CharacterTag, "cr");

/// 稳定 ID。`T` 只用于类型区分，不占空间。
pub struct Id<T> {
    /// 低 40 bit 有效，高位恒为 0。用 u64 而非 [u8; 8] 是因为编解码全是位运算，
    /// 数组只会平添来回转换。
    raw: u64,
    _t: PhantomData<T>,
}

impl<T: Tag> Id<T> {
    /// 生成一个新的随机 ID。
    pub fn generate() -> Self {
        Self {
            raw: random_bits(),
            _t: PhantomData,
        }
    }

    /// 从原始位构造。仅供解析与测试；高于 40 bit 的位会被截断。
    pub fn from_raw(raw: u64) -> Self {
        Self {
            raw: raw & ((1u64 << BITS) - 1),
            _t: PhantomData,
        }
    }

    pub fn raw(self) -> u64 {
        self.raw
    }

    /// 不带前缀的 8 位 base32 主体。
    fn body(self) -> String {
        let mut buf = [0u8; LEN];
        for (i, slot) in buf.iter_mut().enumerate() {
            // 从高位往低位取，保证字符串序与数值序一致（便于排序与 diff 阅读）。
            let shift = BITS - 5 * (i as u32 + 1);
            *slot = ALPHABET[((self.raw >> shift) & 0x1F) as usize];
        }
        // SAFETY 替代方案：ALPHABET 全是 ASCII，from_utf8 必成功，但不用 unsafe。
        String::from_utf8(buf.to_vec()).unwrap_or_default()
    }
}

/// 生成 40 bit 随机数。
///
/// 不引入 rand crate：这里只需要「不可预测且不易碰撞」，不需要密码学强度，
/// 也不值得为此多一条依赖链。用系统提供的随机源。
fn random_bits() -> u64 {
    let mut buf = [0u8; 8];
    getrandom(&mut buf);
    u64::from_le_bytes(buf) & ((1u64 << BITS) - 1)
}

#[cfg(unix)]
fn getrandom(buf: &mut [u8; 8]) {
    // SAFETY: buf 是合法的可写内存，长度与传入一致。
    let n = unsafe { libc::getentropy(buf.as_mut_ptr().cast(), buf.len()) };
    if n != 0 {
        // getentropy 只在参数非法时失败（长度 > 256），此处不可能。
        // 但绝不能因此 panic 或产出可预测值——退回到时间 + 地址混合。
        fill_from_fallback(buf);
    }
}

#[cfg(windows)]
fn getrandom(buf: &mut [u8; 8]) {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    // SAFETY: buf 合法可写；传 null 算法句柄配合 USE_SYSTEM_PREFERRED_RNG 是文档指定用法。
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        fill_from_fallback(buf);
    }
}

/// 系统随机源不可用时的退路。
///
/// 质量低于系统源，但仍足以避免同一进程内的碰撞（计数器保证），
/// 且绝不返回常量——那会让所有章节撞同一个 id。
fn fill_from_fallback(buf: &mut [u8; 8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed =
        nanos.rotate_left(17) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (buf.as_ptr() as u64);
    *buf = mixed.to_le_bytes();
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseIdError {
    #[error("ID 缺少 `{expected}_` 前缀")]
    WrongPrefix { expected: &'static str },
    #[error("ID 主体应为 {LEN} 位 base32，实为 {actual} 位")]
    WrongLength { actual: usize },
    #[error("ID 含非法 base32 字符 `{ch}`")]
    BadChar { ch: char },
}

impl<T: Tag> FromStr for Id<T> {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s
            .strip_prefix(T::PREFIX)
            .and_then(|r| r.strip_prefix('_'))
            .ok_or(ParseIdError::WrongPrefix {
                expected: T::PREFIX,
            })?;

        if body.chars().count() != LEN {
            return Err(ParseIdError::WrongLength {
                actual: body.chars().count(),
            });
        }

        let mut raw = 0u64;
        for ch in body.chars() {
            // 大小写不敏感：用户手抄 id 时不该因为大小写失败。
            let up = ch.to_ascii_uppercase() as u8;
            let v = ALPHABET
                .iter()
                .position(|&a| a == up)
                .ok_or(ParseIdError::BadChar { ch })?;
            raw = (raw << 5) | v as u64;
        }
        Ok(Self::from_raw(raw))
    }
}

impl<T: Tag> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", T::PREFIX, self.body())
    }
}

impl<T: Tag> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug 与 Display 一致：日志里看到的就是文件里的样子。
        write!(f, "{self}")
    }
}

// 手写这些 impl 而非 derive：derive 会给 T 加上无谓的约束
// （PhantomData<T> 让 derive 要求 T: Clone 等），而 T 只是个类型标记。
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T> Eq for Id<T> {}
impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}
impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

/// 序列化为带前缀的字符串，而非数字。
///
/// toml/json 里存 `id = "ch_7Q2M4KZA"` 而不是 `id = 123456`：
/// 用户会直接看这些文件（§1 纯文本为真相），数字对他毫无意义。
impl<T: Tag> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de, T: Tag> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

pub type BookId = Id<BookTag>;
pub type VolumeId = Id<VolumeTag>;
pub type ChapterId = Id<ChapterTag>;
pub type CharacterId = Id<CharacterTag>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn display_has_prefix_and_eight_chars() {
        let id = ChapterId::generate();
        let s = id.to_string();
        assert!(s.starts_with("ch_"), "{s}");
        assert_eq!(s.len(), 3 + LEN, "{s}");
    }

    #[test]
    fn roundtrips_through_string() {
        for _ in 0..1000 {
            let id = ChapterId::generate();
            let parsed: ChapterId = id.to_string().parse().unwrap();
            assert_eq!(id, parsed, "{id} 往返失败");
        }
    }

    #[test]
    fn roundtrips_extremes() {
        for raw in [0u64, 1, (1 << BITS) - 1] {
            let id = ChapterId::from_raw(raw);
            let parsed: ChapterId = id.to_string().parse().unwrap();
            assert_eq!(id.raw(), parsed.raw(), "raw={raw} 往返失败");
        }
    }

    #[test]
    fn zero_id_renders_as_all_zeros() {
        assert_eq!(ChapterId::from_raw(0).to_string(), "ch_00000000");
    }

    #[test]
    fn parse_is_case_insensitive() {
        let id = ChapterId::from_raw(0x00_1234_5678);
        let lower: ChapterId = id.to_string().to_lowercase().parse().unwrap();
        assert_eq!(id, lower);
    }

    #[test]
    fn rejects_wrong_prefix() {
        // 卷 id 不能被当成章 id 读进来——这类错配必须在解析层拦下。
        let vol = VolumeId::generate().to_string();
        assert_eq!(
            vol.parse::<ChapterId>(),
            Err(ParseIdError::WrongPrefix { expected: "ch" })
        );
    }

    #[test]
    fn rejects_bad_length_and_chars() {
        assert!(matches!(
            "ch_ABC".parse::<ChapterId>(),
            Err(ParseIdError::WrongLength { actual: 3 })
        ));
        // I/L/O/U 不在 Crockford 字母表里。
        assert!(matches!(
            "ch_IIIIIIII".parse::<ChapterId>(),
            Err(ParseIdError::BadChar { ch: 'I' })
        ));
    }

    #[test]
    fn alphabet_excludes_confusable_letters() {
        for c in *b"ILOU" {
            assert!(
                !ALPHABET.contains(&c),
                "{} 易与数字混淆，不应在字母表中",
                c as char
            );
        }
        assert_eq!(ALPHABET.len(), 32);
    }

    /// id 必须是随机的。若退化成常量或计数器，所有章节会撞在一起。
    #[test]
    fn generates_distinct_ids() {
        use std::collections::HashSet;
        let set: HashSet<_> = (0..10_000).map(|_| ChapterId::generate()).collect();
        assert_eq!(set.len(), 10_000, "1 万次生成出现碰撞");
    }

    /// 退路随机源也不得产出重复值——它在系统源失效时兜底。
    #[test]
    fn fallback_source_is_not_constant() {
        use std::collections::HashSet;
        let set: HashSet<u64> = (0..1000)
            .map(|_| {
                let mut b = [0u8; 8];
                fill_from_fallback(&mut b);
                u64::from_le_bytes(b)
            })
            .collect();
        assert!(set.len() > 990, "退路随机源碰撞过多: {}", set.len());
    }

    #[test]
    fn serde_roundtrip_uses_string_form() {
        let id = ChapterId::generate();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.contains("ch_"), "应存为带前缀的字符串: {json}");
        let back: ChapterId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn different_tags_are_distinct_types() {
        // 编译期保证：ChapterId 与 VolumeId 不可互相赋值。
        // 此处只验证前缀不同。
        assert_ne!(
            ChapterId::from_raw(1).to_string(),
            VolumeId::from_raw(1).to_string()
        );
    }
}
