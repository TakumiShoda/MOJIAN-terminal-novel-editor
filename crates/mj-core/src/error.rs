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

    #[error("解析章节文件 {path} 失败：{message}")]
    ChapterParse { path: PathBuf, message: String },

    #[error("章节文件 {path} 的元数据已损坏，拒绝写入以免覆盖正文：{message}")]
    ChapterDamaged { path: PathBuf, message: String },

    #[error("找不到章 {id}")]
    ChapterNotFound { id: crate::id::ChapterId },

    #[error("找不到卷 {id}")]
    VolumeNotFound { id: crate::id::VolumeId },

    #[error("排序号耗尽——请检查该卷的 order 是否异常")]
    OrderExhausted,
}
