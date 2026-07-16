//! 墨简（mojian）入口。见 doc.md §12.2。
//!
//! M0 骨架：子命令已声明，实现待各里程碑填充。

use clap::{Parser, Subcommand};

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
    /// 导出
    Export {
        book: String,
        #[arg(long, value_parser = ["txt", "md", "epub"])]
        format: String,
        #[arg(short, long)]
        out: std::path::PathBuf,
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

    match cli.command {
        // 无子命令 = 启动 TUI
        None => {
            eprintln!("mj: TUI 尚未实现（M0 骨架）。见 doc.md §11 里程碑。");
            Ok(())
        }
        Some(_) => {
            eprintln!("mj: 该子命令尚未实现（M0 骨架）。");
            Ok(())
        }
    }
}
