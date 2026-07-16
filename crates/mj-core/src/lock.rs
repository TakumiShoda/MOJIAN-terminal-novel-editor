//! 单实例锁。见 doc.md §9。
//!
//! 锁文件存 pid。陈旧锁（进程已不存在）自动清理——崩溃过一次就再也打不开自己的稿子，
//! 比不加锁更糟。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct WorkspaceLock {
    path: PathBuf,
}

impl WorkspaceLock {
    /// 尝试取得锁。已被活着的进程占用则返回 `Error::WorkspaceLocked`。
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(pid) = read_pid(path)
            && process_alive(pid)
        {
            return Err(Error::WorkspaceLocked {
                pid,
                path: path.to_owned(),
            });
        }
        // 无锁、锁损坏、或持有者已死 → 覆盖。
        crate::atomic::write(path, std::process::id().to_string().as_bytes())?;
        Ok(Self {
            path: path.to_owned(),
        })
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        // 释放失败没有补救手段，且 drop 中不能 panic；留下的陈旧锁下次会被自动清理。
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// 进程是否存活：`kill(pid, 0)` 不发信号，只做存在性与权限检查。
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: kill 带信号 0 不改变目标进程状态，仅做存在性探测。
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// 进程是否存活。
///
/// 只判断「能否打开句柄」是不够的：进程结束后，只要还有句柄未关闭，
/// `OpenProcess` 仍会成功。必须再查退出码——`STILL_ACTIVE` 才算活着。
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: 传入的 pid 无论是否有效，OpenProcess 都只返回句柄或 NULL，不会 UB。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // 打不开：进程不存在，或无权限。无权限时按「不存在」处理会误删他人的锁，
        // 但那需要跨用户共享 workspace——不在设计范围内（§9 单实例锁是本机语义）。
        return false;
    }

    let mut code: u32 = 0;
    // SAFETY: handle 由上面的 OpenProcess 返回且非空；code 是有效的可写指针。
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: handle 非空且尚未关闭。
    unsafe { CloseHandle(handle) };

    // STILL_ACTIVE 在 windows-sys 里是裸 i32（不是 newtype），故直接转型。
    ok != 0 && code == STILL_ACTIVE as u32
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    // 其余平台无探测手段：保守地认为锁有效（宁可提示用户，也不要覆盖活实例）。
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn acquires_and_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        {
            let _lock = WorkspaceLock::acquire(&path).unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists(), "drop 后应释放锁");
    }

    #[test]
    fn rejects_when_held_by_live_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        let _lock = WorkspaceLock::acquire(&path).unwrap();
        // 当前进程活着，第二次获取应失败。
        assert!(matches!(
            WorkspaceLock::acquire(&path),
            Err(Error::WorkspaceLocked { .. })
        ));
    }

    /// 陈旧锁必须能自动清理，否则崩溃一次就锁死了用户的 workspace。
    #[test]
    fn reclaims_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        // pid 1 之外挑一个几乎不可能存活的 pid。
        std::fs::write(&path, "999999").unwrap();
        assert!(WorkspaceLock::acquire(&path).is_ok(), "陈旧锁应被回收");
    }

    #[test]
    fn reclaims_corrupt_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        std::fs::write(&path, "not-a-pid").unwrap();
        assert!(
            WorkspaceLock::acquire(&path).is_ok(),
            "损坏的锁不应阻塞用户"
        );
    }
}
