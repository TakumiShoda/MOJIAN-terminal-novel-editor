//! 领域模型、存储、索引、版本历史。见 doc.md §5。

pub mod appearance;
pub mod atomic;
pub mod chapter_file;
pub mod config;
pub mod diff;
pub mod error;
pub mod export;
pub mod history;
pub mod id;
pub mod index;
pub mod lock;
pub mod model;
pub mod proof_external;
pub mod proofing;
pub mod slug;
pub mod store;
pub mod swap;
pub mod workspace;

pub use error::{Error, Result};
pub use store::Store;
pub use workspace::Workspace;

/// 当前时间的 RFC3339 字符串（本地时区），如 `2026-07-16T10:00:00+09:00`。
///
/// 时间在磁盘上存字符串而非时间戳：用户会直接读这些 toml/md 文件
/// （§1 纯文本为真相），`1752624000` 对他毫无意义。
pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}
