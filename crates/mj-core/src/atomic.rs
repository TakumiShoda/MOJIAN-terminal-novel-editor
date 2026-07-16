//! 原子写。见 doc.md §0 禁止事项 1、§9 数据安全。
//!
//! 契约：`write(tmp)` → `fsync(tmp)` → `rename(tmp, target)` → `fsync(dir)`。
//!
//! 少任何一步都会在断电时留下截断或空洞的文件——那正是「静默丢失用户正文」。
//! 全项目所有写盘路径都必须走这里，不得直接 `fs::write`。

use std::io::Write as _;
use std::path::Path;

use crate::error::{Error, Result};

/// 原子地把 `bytes` 写到 `target`。
///
/// 临时文件与目标同目录——跨文件系统的 rename 不是原子的。
pub fn write(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target.parent().ok_or_else(|| Error::Io {
        path: target.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "目标路径没有父目录"),
    })?;

    let io_err = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io {
            path: path.clone(),
            source,
        }
    };

    // 同目录下的临时文件：带 pid 以免多实例互踩。
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        target.file_name().and_then(|s| s.to_str()).unwrap_or("mj"),
        std::process::id()
    ));

    // 作用域内写完并 fsync，确保 File 在 rename 前已 drop。
    {
        let mut f = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(bytes).map_err(io_err(&tmp))?;
        // 数据落盘后才能 rename——否则断电会 rename 出一个空文件。
        f.sync_all().map_err(io_err(&tmp))?;
    }

    // Windows 上 rename 覆盖已存在文件同样可行：std 走的是
    // `MoveFileExW(..., MOVEFILE_REPLACE_EXISTING)`（已核对 std 源码，非想当然）。
    // 故此处无需为 Windows 单开「先删再改名」的分支——那反而会制造一个
    // 「文件已删、改名未成」的丢稿窗口。
    std::fs::rename(&tmp, target).map_err(io_err(target))?;

    // rename 本身也要落盘，否则目录项可能还在页缓存里。
    // Windows 无法 open 目录做 fsync，此步在该平台跳过：
    // NTFS 的元数据日志已提供等价保证。
    #[cfg(unix)]
    {
        let d = std::fs::File::open(dir).map_err(io_err(dir))?;
        d.sync_all().map_err(io_err(dir))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn writes_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.md");
        write(&target, "　　雪落了一夜。".as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "　　雪落了一夜。"
        );
    }

    #[test]
    fn overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.md");
        write(&target, b"old").unwrap();
        write(&target, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }

    /// 写完不留临时文件——否则用户目录会被 .tmp 塞满，且 git 会看到噪声。
    #[test]
    fn leaves_no_tmp_behind() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.md"), b"x").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
    }
}
