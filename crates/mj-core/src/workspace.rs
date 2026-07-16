//! Workspace 路径解析与目录布局。见 doc.md §5.1。
//!
//! 真相是磁盘：本模块只负责「路径在哪」与「目录存在」，不缓存任何正文。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Workspace 根目录及其标准子路径（doc.md §5.1）。
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// 解析 workspace 根目录，优先级：显式指定 > 环境变量 > 平台默认。
    ///
    /// 平台默认走 `directories`，即 Linux `~/.local/share/mojian`、
    /// macOS `~/Library/Application Support/mojian`。doc.md §5.1 写的是 Linux 路径，
    /// 此处按平台惯例取值——跨平台一等支持（§9）优先于文档里的示例路径。
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self> {
        if let Some(p) = explicit {
            return Ok(Self { root: p });
        }
        if let Some(p) = std::env::var_os("MOJIAN_WORKSPACE") {
            return Ok(Self {
                root: PathBuf::from(p),
            });
        }
        let dirs = directories::ProjectDirs::from("", "", "mojian").ok_or(Error::NoHomeDir)?;
        Ok(Self {
            root: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn library_file(&self) -> PathBuf {
        self.root.join("library.toml")
    }

    pub fn dict_dir(&self) -> PathBuf {
        self.root.join("dict")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn books_dir(&self) -> PathBuf {
        self.root.join("books")
    }

    /// panic 时未保存缓冲的落盘处（doc.md §9）。
    pub fn crash_dir(&self) -> PathBuf {
        self.root.join("crash")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.root.join(".lock")
    }

    /// 建立标准目录骨架。幂等：已存在不报错。
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.dict_dir(),
            self.logs_dir(),
            self.books_dir(),
            self.crash_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        }
        Ok(())
    }
}
