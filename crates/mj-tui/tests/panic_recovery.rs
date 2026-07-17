//! M0 验收：崩溃不留残端，且不吞正文。见 doc.md §9、§11。
//!
//! 这些测试必须真的 panic 一次——「hook 装了」和「hook 起作用了」是两回事。
//!
//! panic hook 是**进程级**的，本文件各测试共享同一个。而 `install` 是幂等的
//! ——它**替换**上一个 hook，不是叠加。所以两个测试并行时，后装的会顶掉先装的：
//! 先装的那个测试 panic 时，dump 就写进了另一个测试的目录，自己这边一个文件没有。
//!
//! 我起初以为「各用各的 tempdir 就互不干扰」，macOS 上也确实一直是绿的——
//! 直到 CI 在 Windows 上翻车（线程调度不同，交错顺序就变了）。
//! 那不是 Windows 的问题，是这些测试本来就有竞态，macOS 只是运气好。
//!
//! 故用一把锁把它们串起来：装 hook → panic → 检查，全程独占。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use mj_tui::CrashDump;

/// 序列化所有会碰 panic hook 的测试。
static HOOK_LOCK: Mutex<()> = Mutex::new(());

/// 取得独占权。锁中毒无所谓——我们只关心互斥，不关心它守的那个 `()`。
fn lock_hook() -> MutexGuard<'static, ()> {
    HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 在子线程里触发 panic，验证 hook 把未保存正文写到了 crash 目录。
///
/// 用子线程而非子进程：panic hook 是进程级的，子线程 panic 同样会触发它，
/// 而 `catch_unwind` 能挡住测试进程被带崩。
#[test]
fn panic_dumps_unsaved_text_to_crash_dir() {
    let _guard = lock_hook();
    let dir = tempfile::tempdir().unwrap();
    let crash_dir = dir.path().join("crash");

    let dump = CrashDump::new();
    dump.set("ch_7Q2M4KZA", "　　雪落了一夜。他推开门，风裹着雪灌进来。");

    mj_tui::panic::install(crash_dir.clone(), dump);

    // 静音默认 hook 的 backtrace 输出，避免污染测试输出。
    let result = std::panic::catch_unwind(|| {
        panic!("模拟崩溃");
    });
    assert!(result.is_err(), "应当 panic");

    // 正文必须落盘。
    let dumped = read_all_dumps(&crash_dir);
    assert_eq!(dumped.len(), 1, "应恰好产生一个 dump 文件，实得 {dumped:?}");

    let (name, content) = &dumped[0];
    assert!(name.contains("ch_7Q2M4KZA"), "文件名应含章节标识: {name}");
    assert_eq!(
        content, "　　雪落了一夜。他推开门，风裹着雪灌进来。",
        "dump 内容必须与缓冲逐字一致——这是用户唯一的稿子"
    );
}

/// 没有未保存内容时不应产生空文件——否则 crash 目录会被噪声塞满。
#[test]
fn panic_without_buffers_writes_nothing() {
    let _guard = lock_hook();
    let dir = tempfile::tempdir().unwrap();
    let crash_dir = dir.path().join("crash");

    mj_tui::panic::install(crash_dir.clone(), CrashDump::new());

    let _ = std::panic::catch_unwind(|| panic!("模拟崩溃"));

    assert!(
        read_all_dumps(&crash_dir).is_empty(),
        "无未保存内容时不应写文件"
    );
}

/// 重复 install 不得让 hook 层层叠加——否则一次 panic 会恢复多遍终端、
/// dump 多份文件。这里以「dump 文件恰好一份」作为不叠加的可观测证据。
#[test]
fn repeated_install_does_not_stack_hooks() {
    let _guard = lock_hook();
    let dir = tempfile::tempdir().unwrap();
    let crash_dir = dir.path().join("crash");

    for _ in 0..3 {
        let dump = CrashDump::new();
        dump.set("ch_1", "正文");
        mj_tui::panic::install(crash_dir.clone(), dump);
    }

    let _ = std::panic::catch_unwind(|| panic!("模拟崩溃"));

    assert_eq!(
        read_all_dumps(&crash_dir).len(),
        1,
        "install 三次后 panic 一次，仍应只 dump 一份"
    );
}

fn read_all_dumps(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read_to_string(e.path()).unwrap_or_default(),
            )
        })
        .collect()
}
