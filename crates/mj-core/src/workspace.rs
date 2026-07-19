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

    /// 用户词典（专名，注入 jieba）。§5.1。
    pub fn user_dict_file(&self) -> PathBuf {
        self.dict_dir().join("user.txt")
    }

    /// 用户混淆集（覆盖/追加内置）。§5.1。
    pub fn confusion_file(&self) -> PathBuf {
        self.dict_dir().join("confusion.tsv")
    }

    /// 已忽略的校对问题（按 hash）。§5.1、§6.8。
    pub fn ignore_file(&self) -> PathBuf {
        self.dict_dir().join("ignore.json")
    }

    /// 用户自建主题目录（§6.10：主题定义为 TOML，放 themes/*.toml）。
    pub fn themes_dir(&self) -> PathBuf {
        self.root.join("themes")
    }

    pub fn theme_file(&self, name: &str) -> PathBuf {
        self.themes_dir().join(format!("{name}.toml"))
    }

    /// 读用户主题的 TOML 文本。不存在返回 None（用内置同名主题）。
    ///
    /// 只回文本、不解析：颜色是 ratatui 的类型，按 §4 分层归 mj-tui，
    /// 本 crate 不认得也不该认得。
    pub fn read_theme(&self, name: &str) -> Option<String> {
        let path = self.theme_file(name);
        match std::fs::read_to_string(&path) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "读主题文件失败，用内置主题");
                None
            }
        }
    }

    /// 列出用户自建的主题名（不含内置）。
    pub fn list_user_themes(&self) -> Vec<String> {
        let dir = self.themes_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("toml"))
                    .then(|| p.file_stem()?.to_str().map(|s| s.to_string()))
                    .flatten()
            })
            .collect();
        out.sort();
        out
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

    /// 某本书的索引缓存（§5.1：`books/<book-id>/.index.sqlite`）。
    ///
    /// 索引放在书目录内而非 workspace 根：这样一本书的目录是自足的，
    /// 用户整个拷走仍能用（§1 纯文本为真相——他确实会这么干）。
    pub fn book_index_file(&self, book: crate::id::BookId) -> PathBuf {
        self.books_dir()
            .join(book.to_string())
            .join(".index.sqlite")
    }

    /// 建立标准目录骨架。幂等：已存在不报错。
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.dict_dir(),
            self.logs_dir(),
            self.books_dir(),
            self.crash_dir(),
            self.themes_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        }
        Ok(())
    }
}
