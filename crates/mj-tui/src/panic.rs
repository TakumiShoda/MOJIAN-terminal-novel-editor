//! Panic hook：崩溃时先救终端与正文，再打印 backtrace。
//!
//! 见 doc.md §6.10、§9。要做三件事，顺序不能反：
//! 1. 恢复终端（离开 alternate screen、关 raw mode、重置字体）——否则用户终端永久变形；
//! 2. dump 未保存缓冲到 `crash/<ts>-<chapter>.txt`——正文比进程重要；
//! 3. 打印 backtrace。
//!
//! ratatui 自带的 hook 只做第 1 步的前两项（且是私有的），故自行实现。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// panic 时可供 dump 的未保存缓冲。
///
/// 用 `Mutex<Vec<_>>` 而非 channel：panic hook 里没法 await，也不能依赖
/// 事件循环还活着——它很可能正是 panic 的那个线程。
#[derive(Clone, Default)]
pub struct CrashDump(Arc<Mutex<Vec<Buffer>>>);

#[derive(Clone)]
pub struct Buffer {
    /// 章节标识，用于文件名；无归属的缓冲用 "unknown"。
    pub chapter: String,
    pub text: String,
}

impl CrashDump {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记/更新一份缓冲。编辑器应在正文变脏时调用。
    pub fn set(&self, chapter: impl Into<String>, text: impl Into<String>) {
        let chapter = chapter.into();
        let text = text.into();
        // 锁中毒说明另一线程已在 panic：此时仍要尽力写入，故取 into_inner。
        let mut guard = match self.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.iter_mut().find(|b| b.chapter == chapter) {
            Some(b) => b.text = text,
            None => guard.push(Buffer { chapter, text }),
        }
    }

    pub fn clear(&self, chapter: &str) {
        let mut guard = match self.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.retain(|b| b.chapter != chapter);
    }

    fn take_all(&self) -> Vec<Buffer> {
        match self.0.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>;

/// 进程启动时的原始 hook，只捕获一次。
///
/// 重复 `install` 时用它作为链尾，而不是把上一个 mj hook 串进去——
/// 否则一次 panic 会恢复两遍终端、打印两遍 backtrace。
static ORIGINAL_HOOK: std::sync::OnceLock<PanicHook> = std::sync::OnceLock::new();

/// 安装 panic hook。
///
/// `crash_dir` 为 dump 目标目录；`dump` 是共享的未保存缓冲句柄。
///
/// 幂等：重复调用只替换自己，不会层层叠加。
pub fn install(crash_dir: PathBuf, dump: CrashDump) {
    // 首次调用时 take_hook 拿到的是标准库默认 hook，存起来永久复用。
    // 之后再调用，take_hook 拿到的是我们自己上一次装的，直接丢弃。
    let taken = std::panic::take_hook();
    ORIGINAL_HOOK.get_or_init(|| taken);
    let previous = |info: &std::panic::PanicHookInfo<'_>| {
        if let Some(h) = ORIGINAL_HOOK.get() {
            h(info);
        }
    };

    std::panic::set_hook(Box::new(move |info| {
        // 1. 先恢复终端。这一步失败也要继续——救正文比救终端重要。
        restore_terminal();

        // 2. dump 未保存缓冲。
        let written = write_dumps(&crash_dir, dump.take_all());

        // 3. 交还给默认 hook 打印 backtrace。此时已不在 alternate screen，输出可见。
        previous(info);

        // 用户此刻正盯着一屏 backtrace，必须明确告诉他稿子在哪——
        // 否则他会以为刚写的东西全没了。
        //
        // 这是 §0 禁止事项 2（TUI 期间禁印 stdout/stderr）的唯一豁免点：
        // 此刻 TUI 已经死了、终端已恢复，不存在「撕裂界面」的问题；
        // 而「稿子存哪了」只能靠这里告诉用户，进日志他看不见。
        #[allow(clippy::print_stderr)]
        if !written.is_empty() {
            eprintln!("\n墨简已崩溃，但未保存的正文已保存到：");
            for p in &written {
                eprintln!("  {}", p.display());
            }
        }
    }));
}

/// 恢复终端：关 raw mode、离开 alternate screen、重置字体。
fn restore_terminal() {
    // 字体重置必须在离开 alternate screen 之前发——OSC 序列要送到宿主终端。
    // FontController 在 panic 上下文里不可用（可能正持锁），故直接发原始序列。
    // 见 doc.md §2.1：仅对支持的终端有效，其余无副作用。
    crate::font::emit_reset_sequence();

    // 忽略错误：此处没有补救手段，且不能在 panic hook 里再 panic。
    let _ = ratatui::try_restore();
}

/// 把缓冲写到 crash 目录，返回实际写成的路径。
fn write_dumps(crash_dir: &std::path::Path, buffers: Vec<Buffer>) -> Vec<PathBuf> {
    if buffers.is_empty() {
        return Vec::new();
    }
    if std::fs::create_dir_all(crash_dir).is_err() {
        return Vec::new();
    }

    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut written = Vec::new();

    for b in buffers {
        if b.text.is_empty() {
            continue;
        }
        let path = crash_dir.join(format!("{}-{}.txt", ts, sanitize(&b.chapter)));
        // 这里用 std::fs::write 而非 mj-core 的原子写：panic 路径上要尽量少做事，
        // 且 crash dump 是一次性新文件，没有「覆盖到一半」的风险。
        if std::fs::write(&path, &b.text).is_ok() {
            written.push(path);
        }
    }
    written
}

/// 章节名可能含路径分隔符或空格，落成文件名前先净化。
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn dump_stores_and_updates_by_chapter() {
        let d = CrashDump::new();
        d.set("ch_1", "雪落了一夜。");
        d.set("ch_1", "雪落了一夜。他推开门。");
        d.set("ch_2", "第二章");

        let all = d.take_all();
        assert_eq!(all.len(), 2, "同一章应更新而非追加");
        let ch1 = all.iter().find(|b| b.chapter == "ch_1").unwrap();
        assert_eq!(ch1.text, "雪落了一夜。他推开门。");
    }

    #[test]
    fn clear_removes_saved_buffer() {
        let d = CrashDump::new();
        d.set("ch_1", "x");
        d.clear("ch_1");
        assert!(d.take_all().is_empty());
    }

    #[test]
    fn writes_dump_files_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let buffers = vec![Buffer {
            chapter: "ch_7Q2M".into(),
            text: "　　雪落了一夜。".into(),
        }];

        let written = write_dumps(dir.path(), buffers);

        assert_eq!(written.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&written[0]).unwrap(),
            "　　雪落了一夜。"
        );
    }

    #[test]
    fn skips_empty_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let written = write_dumps(
            dir.path(),
            vec![Buffer {
                chapter: "a".into(),
                text: String::new(),
            }],
        );
        assert!(written.is_empty(), "空缓冲不该产生文件");
    }

    #[test]
    fn sanitizes_chapter_into_filename() {
        assert_eq!(sanitize("第一章 雪夜"), "第一章_雪夜");
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize(""), "unknown");
    }
}
