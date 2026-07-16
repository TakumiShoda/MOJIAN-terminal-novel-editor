//! 日志初始化。见 doc.md §9。
//!
//! doc.md §0 禁止事项 2：TUI 运行期间不得向 stdout/stderr 打印任何内容。
//! 因此日志一律进文件 `logs/mj.log`（按天轮转），级别可用 `RUST_LOG` 调。

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// 初始化文件日志。
///
/// 返回的 `WorkerGuard` 必须在 main 中持有到退出——drop 时才会 flush 掉缓冲的日志。
/// 丢掉它等于崩溃时的最后几条日志（最有用的那几条）永远写不出来。
pub fn init(logs_dir: &Path) -> anyhow::Result<WorkerGuard> {
    let appender = tracing_appender::rolling::daily(logs_dir, "mj.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false) // 日志文件里不要 ANSI 转义序列
        .init();

    Ok(guard)
}
