//! mj-core 错误类型。
//!
//! IO 错误一律带上路径：doc.md §0 禁止静默丢失正文，
//! 一条不知道是哪个文件写失败的错误，等于没有错误。

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("读写 {path} 失败")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("无法确定用户主目录，请用 --workspace 显式指定")]
    NoHomeDir,

    #[error("解析配置 {path} 失败")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("workspace 已被进程 {pid} 占用（锁文件 {path}）")]
    WorkspaceLocked { pid: u32, path: PathBuf },
}
