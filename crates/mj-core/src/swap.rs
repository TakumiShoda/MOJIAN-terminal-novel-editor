//! 崩溃恢复文件（.swp）。见 doc.md §6.3、§9。
//!
//! 契约：`.<chapter>.swp` 与章节文件同目录；启动时检测到则提示恢复。
//!
//! 与自动保存的分工：
//! - **自动保存**写的是正文本身，有节流（空闲 3 秒 / 累计 200 字），
//!   因为每次都要序列化 front matter + 原子写 + fsync，太频繁会卡顿；
//! - **swp** 写的是「万一现在断电，还没进正文的那部分」，故节流必须比自动保存更紧。
//!   两者不是一回事：自动保存之间的窗口，正是 swp 存在的意义。
//!
//! swp 只存正文，不存 front matter——恢复时把它灌回缓冲即可，
//! 元数据以磁盘上的章节文件为准。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// 由章节文件路径推出 swp 路径：`0010-开篇.md` -> `.0010-开篇.md.swp`。
///
/// 前缀点让它在 `ls` 里默认隐藏，且 §5.1 的 `.gitignore` 已排除 `.swp`。
pub fn swap_path(chapter_path: &Path) -> PathBuf {
    let name = chapter_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("chapter");
    let parent = chapter_path.parent().unwrap_or(Path::new("."));
    parent.join(format!(".{name}.swp"))
}

/// 写 swp。
///
/// 同样走原子写：swp 本身写坏了，恢复时就是一份半截的稿子，
/// 那比没有 swp 更糟——用户会以为救回来了。
pub fn write(chapter_path: &Path, body: &str) -> Result<()> {
    crate::atomic::write(&swap_path(chapter_path), body.as_bytes())
}

/// 删除 swp。正文已安全落盘后调用。
pub fn remove(chapter_path: &Path) -> Result<()> {
    let p = swap_path(chapter_path);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        // 本来就没有，不算错。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io { path: p, source }),
    }
}

/// 一份待恢复的 swp。
#[derive(Debug, Clone, PartialEq)]
pub struct Recovery {
    pub swap_path: PathBuf,
    /// swp 里的正文（崩溃时未保存的版本）。
    pub swap_body: String,
    /// 章节文件里的正文（上次成功保存的版本）。
    pub saved_body: String,
}

impl Recovery {
    /// swp 与已保存的正文是否确有差异。
    ///
    /// 无差异说明是上次正常退出时没清理干净的残留——静默删掉即可，
    /// 不该拿一个「恢复吗？」的问题去打扰用户。
    pub fn differs(&self) -> bool {
        self.swap_body != self.saved_body
    }
}

/// 检测某章是否有待恢复的 swp。
///
/// `saved_body` 是章节文件里当前的正文，用于比对。
pub fn detect(chapter_path: &Path, saved_body: &str) -> Result<Option<Recovery>> {
    let p = swap_path(chapter_path);
    let swap_body = match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::Io { path: p, source }),
    };

    // swp 也可能是 CRLF（用户在 Windows 上崩的，或手动动过）——统一归一化，
    // 否则会因为行尾差异误报「有未保存的改动」（ADR 0003）。
    let swap_body = mj_text::eol::normalize(&swap_body);

    Ok(Some(Recovery {
        swap_path: p,
        swap_body,
        saved_body: saved_body.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("0010-开篇.md");
        std::fs::write(&p, "+++\n+++\n正文").unwrap();
        (d, p)
    }

    #[test]
    fn swap_path_is_hidden_sibling() {
        let p = Path::new("/books/x/chapters/0010-开篇.md");
        assert_eq!(
            swap_path(p),
            Path::new("/books/x/chapters/.0010-开篇.md.swp")
        );
    }

    #[test]
    fn writes_and_detects() {
        let (_d, ch) = tmp();
        write(&ch, "　　未保存的新内容").unwrap();

        let r = detect(&ch, "　　旧内容").unwrap().unwrap();
        assert_eq!(r.swap_body, "　　未保存的新内容");
        assert_eq!(r.saved_body, "　　旧内容");
        assert!(r.differs(), "内容不同应报告差异");
    }

    #[test]
    fn detect_returns_none_without_swap() {
        let (_d, ch) = tmp();
        assert!(detect(&ch, "正文").unwrap().is_none());
    }

    /// swp 与已保存内容相同 = 上次正常退出的残留，不该打扰用户。
    #[test]
    fn identical_swap_is_not_a_real_recovery() {
        let (_d, ch) = tmp();
        write(&ch, "　　一样的内容").unwrap();
        let r = detect(&ch, "　　一样的内容").unwrap().unwrap();
        assert!(!r.differs(), "内容相同不应提示恢复");
    }

    #[test]
    fn remove_deletes_swap() {
        let (_d, ch) = tmp();
        write(&ch, "x").unwrap();
        assert!(swap_path(&ch).exists());
        remove(&ch).unwrap();
        assert!(!swap_path(&ch).exists());
    }

    #[test]
    fn remove_is_idempotent() {
        let (_d, ch) = tmp();
        remove(&ch).unwrap();
        remove(&ch).unwrap(); // 不存在也不该报错
    }

    /// swp 里的 CRLF 不得被当成「有改动」（ADR 0003）。
    #[test]
    fn crlf_in_swap_does_not_cause_false_positive() {
        let (_d, ch) = tmp();
        std::fs::write(swap_path(&ch), "第一行\r\n第二行").unwrap();
        let r = detect(&ch, "第一行\n第二行").unwrap().unwrap();
        assert!(!r.differs(), "仅行尾不同不应报告为改动");
    }

    #[test]
    fn swap_survives_cjk_and_emoji() {
        let (_d, ch) = tmp();
        let body = "　　雪落了一夜。👨‍👩‍👧「你来了。」";
        write(&ch, body).unwrap();
        let r = detect(&ch, "").unwrap().unwrap();
        assert_eq!(r.swap_body, body, "swp 应逐字保存");
    }
}
