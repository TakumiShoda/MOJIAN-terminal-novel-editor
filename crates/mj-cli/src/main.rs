//! 墨简（mojian）入口。见 doc.md §12.2。
//!
//! M0：workspace 建好、日志、config 加载、panic hook 恢复终端、ratatui 起窗。
//! 各无头子命令待后续里程碑填充。

mod logging;

use clap::{Parser, Subcommand};
use mj_core::{Workspace, config::Config, lock::WorkspaceLock};

#[derive(Parser)]
#[command(name = "mj", version, about = "终端小说写作器")]
struct Cli {
    /// 覆盖默认 workspace 路径
    #[arg(long, global = true)]
    workspace: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 统计字数
    Count {
        #[arg(long)]
        book: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 排版；--check 只报告不改，退出码非零表示需要排版
    Format {
        path: std::path::PathBuf,
        #[arg(long)]
        check: bool,
    },
    /// 导出为 txt / md / epub
    Export {
        /// 书 id 或书名
        book: String,
        #[arg(long, value_parser = ["txt", "md", "epub"])]
        format: String,
        #[arg(short, long)]
        out: std::path::PathBuf,
    },
    /// 从 Markdown 导入成一本新书（`#` 书名 / `##` 卷 / `###` 章）
    Import {
        file: std::path::PathBuf,
        /// 文件里没有 `# 书名` 时用它
        #[arg(long, default_value = "导入的书")]
        title: String,
    },
    /// 版本历史
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
    /// 配置
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// 探测终端能力（truecolor / 字体 / 键盘协议 / 剪贴板）并打印报告
    Doctor,
}

#[derive(Subcommand)]
enum HistoryAction {
    List { chapter: String },
}

#[derive(Subcommand)]
enum ConfigAction {
    Check,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let ws = Workspace::resolve(cli.workspace)?;
    ws.ensure_layout()?;

    // guard 必须活到 main 结束：drop 时才 flush 日志（崩溃前那几条最有用）。
    let _log_guard = logging::init(&ws.logs_dir())?;
    tracing::info!(workspace = %ws.root().display(), "启动");

    match cli.command {
        // 无子命令 = 启动 TUI
        None => run_tui(&ws),
        Some(Command::Config {
            action: ConfigAction::Check,
        }) => config_check(&ws),
        Some(Command::Doctor) => doctor(&ws),
        Some(Command::Export { book, format, out }) => export(&ws, &book, &format, &out),
        Some(Command::Import { file, title }) => import(&ws, &file, &title),
        Some(_) => {
            eprintln!("mj: 该子命令尚未实现（见 doc.md §11 里程碑）。");
            std::process::exit(1);
        }
    }
}

fn run_tui(ws: &Workspace) -> anyhow::Result<()> {
    // 单实例锁：两个实例同写一份稿子必然互相覆盖（doc.md §9）。
    let _lock = WorkspaceLock::acquire(&ws.lock_file())?;

    let config = Config::load(&ws.config_file())?;

    // panic hook 必须在起窗之前装——起窗之后到装 hook 之间若 panic，终端就废了。
    let dump = mj_tui::CrashDump::new();
    mj_tui::panic::install(ws.crash_dir(), dump.clone());

    let store = mj_core::Store::new(ws.clone(), config.clone());
    mj_tui::app::run(store, config)
}

/// `mj export`（doc.md §12.2）。
fn export(ws: &Workspace, book: &str, format: &str, out: &std::path::Path) -> anyhow::Result<()> {
    let Some(fmt) = mj_core::export::Format::parse(format) else {
        anyhow::bail!("不认识 {format} 格式；可用 txt、md 或 epub");
    };
    let config = Config::load(&ws.config_file())?;
    let store = mj_core::Store::new(ws.clone(), config);
    let b = mj_core::export::resolve_book(&store, book)?;
    mj_core::export::export_to_file(&store, b.id, fmt, out)?;

    let words: u64 = b
        .volumes
        .iter()
        .flat_map(|v| &v.chapters)
        .filter_map(|c| c.word_count)
        .sum();
    println!(
        "已导出《{}》（{} 卷 {} 章，约 {} 字）→ {}",
        b.title,
        b.volumes.len(),
        b.chapter_count(),
        words,
        out.display()
    );
    Ok(())
}

/// `mj import`：从 Markdown 建一本新书。
fn import(ws: &Workspace, file: &std::path::Path, title: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("读不了 {}：{e}", file.display()))?;
    let config = Config::load(&ws.config_file())?;
    let mut store = mj_core::Store::new(ws.clone(), config);
    let id = mj_core::export::import_markdown(&mut store, &text, title)?;
    let b = store.load_book(id)?;
    println!(
        "已导入《{}》（{} 卷 {} 章）",
        b.title,
        b.volumes.len(),
        b.chapter_count()
    );
    Ok(())
}

/// `mj doctor`：探测终端能力并打印报告（doc.md §12.2）。
///
/// §2.1 的终端能力表标了 `[VERIFY]`——不得照抄。我们没法在开发机上把
/// kitty/alacritty/WT 都验一遍，但用户可以：这条命令在**他自己的终端里**跑，
/// 报的是实际探测结果，拿不准的地方明说是推断。
fn doctor(ws: &Workspace) -> anyhow::Result<()> {
    let config = Config::load(&ws.config_file())?;
    let report = mj_tui::doctor::Report::build(
        &mj_tui::font::EnvProbe::from_process(),
        &config.appearance.font_family,
        config.appearance.font_size,
    );
    print!("{}", report.render());
    println!();
    println!("workspace: {}", ws.root().display());
    println!("配置文件:  {}", ws.config_file().display());
    Ok(())
}

/// `mj config check`：校验并打印生效值（doc.md §8）。
fn config_check(ws: &Workspace) -> anyhow::Result<()> {
    let path = ws.config_file();
    let config = Config::load(&path)?;

    if path.exists() {
        println!("配置文件: {}", path.display());
    } else {
        println!("配置文件: {}（不存在，以下为默认值）", path.display());
    }
    println!("workspace: {}", ws.root().display());
    println!();
    println!("{}", toml::to_string_pretty(&config)?);
    Ok(())
}
