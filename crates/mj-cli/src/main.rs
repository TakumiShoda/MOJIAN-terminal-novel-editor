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
        Some(Command::Count { book, json }) => count(&ws, book.as_deref(), json),
        Some(Command::Format { path, check }) => format_file(&ws, &path, check),
        Some(Command::History {
            action: HistoryAction::List { chapter },
        }) => history_list(&ws, &chapter),
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

/// `mj count [--book <id>] [--json]`：统计字数（doc.md §12.2、§6.4）。
///
/// 逐章从**实际正文**重算，不信 front matter 里的缓存——§5.2 明言不一致时以正文
/// 为准，而「统计字数」这条命令要是拿缓存糊弄，就失去了存在的意义。
fn count(ws: &Workspace, book: Option<&str>, json: bool) -> anyhow::Result<()> {
    use mj_text::count::{WordCount, count as count_text};

    let config = Config::load(&ws.config_file())?;
    let store = mj_core::Store::new(ws.clone(), config);

    let books = match book {
        Some(needle) => vec![mj_core::export::resolve_book(&store, needle)?],
        None => store.list_books()?,
    };

    // 每本书一行：书名、字数（两口径）、章数。顺带累一个全局合计。
    struct Row {
        title: String,
        wc: WordCount,
        chapters: usize,
    }
    let mut rows = Vec::new();
    for b in &books {
        let mut wc = WordCount::default();
        let mut chapters = 0usize;
        for vol in &b.volumes {
            for ch in &vol.chapters {
                if ch.damaged.is_some() {
                    continue;
                }
                match store.load_body(b.id, ch.id) {
                    Ok(body) => {
                        wc += count_text(&body.text.to_string());
                        chapters += 1;
                    }
                    Err(e) => tracing::warn!(chapter = %ch.id, error = %e, "统计：跳过读不出的章"),
                }
            }
        }
        rows.push(Row {
            title: b.title.clone(),
            wc,
            chapters,
        });
    }

    if json {
        // 机器口：每本一个对象 + 合计。WordCount 自己就能 serialize（六口径全给）。
        let total = rows.iter().fold(WordCount::default(), |a, r| a + r.wc);
        let books_json: Vec<_> = rows
            .iter()
            .map(|r| serde_json::json!({ "title": r.title, "chapters": r.chapters, "count": r.wc }))
            .collect();
        let out = serde_json::json!({ "books": books_json, "total": total });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    // 人眼口：含标点是投稿数的口径，不含标点是很多平台的口径，两个都给。
    for r in &rows {
        println!(
            "《{}》{} 章：{} 字（不含标点 {}，纯汉字 {}）",
            r.title, r.chapters, r.wc.with_punct, r.wc.no_punct, r.wc.han
        );
    }
    if rows.len() != 1 {
        let t = rows.iter().fold(WordCount::default(), |a, r| a + r.wc);
        let chapters: usize = rows.iter().map(|r| r.chapters).sum();
        println!(
            "合计 {} 本 {} 章：{} 字（不含标点 {}）",
            rows.len(),
            chapters,
            t.with_punct,
            t.no_punct
        );
    }
    Ok(())
}

/// `mj format <path> [--check]`：排版一个文件（doc.md §12.2、§6.5）。
///
/// **认得章节文件**：`path` 指向带 `+++` front matter 的章节文件时，只排正文、
/// 原样留住头部——否则排版规则会把 TOML 头里的直引号、换行搅乱。指向普通文本
/// 就整篇当正文排。核心排版是 mj-text 的纯函数，这里只管读盘写盘。
///
/// `--check` 照 rustfmt 的规矩：只报告不改，需要排版时退出码非零，好进 CI / 提交钩子。
fn format_file(ws: &Workspace, path: &std::path::Path, check: bool) -> anyhow::Result<()> {
    use mj_core::chapter_file::ChapterFile;

    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读不了 {}：{e}", path.display()))?;
    let opts = Config::load(&ws.config_file())?.format;

    // 是章节文件就拆出正文单独排；不是就整篇当正文。
    let chapter = ChapterFile::parse(&raw, mj_core::id::ChapterId::generate()).ok();
    let body = chapter.as_ref().map_or(raw.as_str(), |c| c.body.as_str());

    let edits = mj_text::format::plan(body, &opts);

    if check {
        if edits.is_empty() {
            println!("{}：已规范", path.display());
            return Ok(());
        }
        println!("{}：{} 处需要排版", path.display(), edits.len());
        // 非零退出码，但这不是「错误」——是状态，故不用 bail!（那会多印一行 Error）。
        std::process::exit(1);
    }

    if edits.is_empty() {
        println!("{}：已规范，未改动", path.display());
        return Ok(());
    }
    let formatted = mj_text::format::apply(body, &edits);
    // 章节文件要把头部原样拼回去，只换正文。
    let out = match chapter {
        Some(mut c) => {
            c.body = formatted;
            c.to_text()
                .map_err(|e| anyhow::anyhow!("回写 front matter 失败：{e}"))?
        }
        None => formatted,
    };
    mj_core::atomic::write(path, out.as_bytes())?;
    println!("{}：已排版（{} 处改动）", path.display(), edits.len());
    Ok(())
}

/// `mj history list <chapter>`：列一章的快照链（doc.md §12.2、§6.9）。
///
/// `<chapter>` 可以是章 id，也可以是标题的一部分。跨全库找：命中不到就报没找到，
/// 命中多个就把候选摆出来让人挑得更准——静默取第一个会让人对着别章的历史发懵。
fn history_list(ws: &Workspace, needle: &str) -> anyhow::Result<()> {
    use mj_core::history::History;

    let config = Config::load(&ws.config_file())?;
    let store = mj_core::Store::new(ws.clone(), config);

    // 跨全库找匹配的章：id 精确等，或标题包含。
    let mut hits = Vec::new();
    for b in store.list_books()? {
        for vol in &b.volumes {
            for ch in &vol.chapters {
                if ch.id.to_string() == needle || ch.title.contains(needle) {
                    hits.push((b.id, b.title.clone(), ch.id, ch.title.clone()));
                }
            }
        }
    }
    // 切片模式取唯一命中，省掉一个会被 §12.2 禁印规则连坐的 unwrap。
    let (book_id, chapter_id, chapter_title) = match hits.as_slice() {
        [] => anyhow::bail!("没找到章：{needle}"),
        [(bid, _, cid, ct)] => (*bid, *cid, ct.clone()),
        _ => {
            eprintln!("「{needle}」匹配到多章，请说得更准（用章 id）：");
            for (_, bt, cid, ct) in &hits {
                eprintln!("  {cid}  《{bt}》{ct}");
            }
            anyhow::bail!("匹配到 {} 章", hits.len());
        }
    };

    let history = History::new(&ws.books_dir().join(book_id.to_string()));
    let snaps = history.list(chapter_id); // 已按时间升序

    if snaps.is_empty() {
        println!("《{chapter_title}》还没有快照");
        return Ok(());
    }
    println!("《{chapter_title}》的快照（{} 个，旧 → 新）：", snaps.len());
    for s in &snaps {
        let pin = if s.pinned { " 📌" } else { "" };
        let label = s
            .label
            .as_deref()
            .map(|l| format!(" 「{l}」"))
            .unwrap_or_default();
        println!(
            "  {}  {}  {}字  {}{}{}",
            s.id,
            s.created.format("%Y-%m-%d %H:%M"),
            s.words,
            s.trigger.label(),
            label,
            pin
        );
    }
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
