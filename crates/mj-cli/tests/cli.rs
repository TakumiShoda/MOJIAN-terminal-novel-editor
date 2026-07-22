//! 无头子命令的端到端测试。见 doc.md §12.2。
//!
//! 跑的是**真编出来的 `mj`**（`CARGO_BIN_EXE_mj`），不是内部函数——这些命令的价值
//! 就在于能被脚本 / CI 调用，那就得连退出码、stdout 一起验，跟用户敲的一模一样。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

use mj_core::config::Config;
use mj_core::history::{History, Trigger};
use mj_core::id::{BookId, ChapterId};
use mj_core::model::ChapterBody;
use mj_core::store::Store;
use mj_core::workspace::Workspace;

struct Fx {
    dir: tempfile::TempDir,
    book: BookId,
    ch1: ChapterId,
}

/// 建一个带一本书两章的工作区。
fn setup() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("测试书", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch1 = store
        .create_chapter(book.id, vol, "第一章 雪夜", None)
        .unwrap();
    let ch2 = store
        .create_chapter(book.id, vol, "第二章 城门", Some(ch1))
        .unwrap();
    store
        .save_body(
            book.id,
            &ChapterBody::new(ch1, "　　雪落了一夜。他推开门。\n"),
        )
        .unwrap();
    store
        .save_body(book.id, &ChapterBody::new(ch2, "　　城门已闭。\n"))
        .unwrap();
    Fx {
        dir,
        book: book.id,
        ch1,
    }
}

impl Fx {
    fn mj(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mj"));
        c.arg("--workspace").arg(self.dir.path());
        c
    }
    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

/// `(stdout, exit_code)`。
fn run(c: &mut Command) -> (String, i32) {
    let out = c.output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

// ---- count ----

#[test]
fn count_reports_two_word_counts() {
    let f = setup();
    let (out, code) = run(f.mj().args(["count"]));
    assert_eq!(code, 0);
    assert!(out.contains("测试书"), "{out}");
    assert!(out.contains("2 章"), "两章都要算上：{out}");
    // 含标点与不含标点两个口径都要给（§6.4）。
    assert!(out.contains("字") && out.contains("不含标点"), "{out}");
}

#[test]
fn count_json_is_valid_and_has_a_total() {
    let f = setup();
    let (out, code) = run(f.mj().args(["count", "--json"]));
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("--json 要吐合法 JSON");
    assert!(v["total"]["with_punct"].as_u64().unwrap() > 0, "{out}");
    assert_eq!(v["books"].as_array().unwrap().len(), 1);
}

#[test]
fn count_book_filter_narrows_to_one() {
    let f = setup();
    let (out, code) = run(f.mj().args(["count", "--book", &f.book.to_string()]));
    assert_eq!(code, 0);
    assert!(out.contains("测试书"), "{out}");
}

// ---- format ----

#[test]
fn format_check_exit_code_signals_whether_work_is_needed() {
    let f = setup();
    let bad = f.path().join("bad.txt");
    std::fs::write(&bad, "没有缩进的一段。\n\n\n\n空行太多的一段。\n").unwrap();

    // --check 不改文件，需要排版时退出码非零（照 rustfmt 的规矩）。
    let before = std::fs::read_to_string(&bad).unwrap();
    let (_, code) = run(f.mj().args(["format"]).arg(&bad).arg("--check"));
    assert_eq!(code, 1, "脏文件 --check 应退出 1");
    assert_eq!(
        std::fs::read_to_string(&bad).unwrap(),
        before,
        "--check 不该动文件"
    );

    // 真排一遍，再 --check 应过。
    let (_, code) = run(f.mj().args(["format"]).arg(&bad));
    assert_eq!(code, 0);
    let (_, code) = run(f.mj().args(["format"]).arg(&bad).arg("--check"));
    assert_eq!(code, 0, "排过之后应已规范");
}

/// 指向带 `+++` 头的章节文件时，只排正文、头部原样留住。
///
/// 这是最容易出错的一处：排版规则若碰到 TOML 头，直引号、换行都会被搅乱，
/// 章节文件就再也解析不回去了。
#[test]
fn format_preserves_chapter_front_matter() {
    let f = setup();
    let ch = f.path().join("dirty.md");
    std::fs::write(
        &ch,
        "+++\nid = \"ch_TESTAAAA\"\ntitle = \"脏正文\"\nstatus = \"draft\"\n+++\n没缩进。\n\n\n\n又一段。\n",
    )
    .unwrap();

    let (_, code) = run(f.mj().args(["format"]).arg(&ch));
    assert_eq!(code, 0);

    let after = std::fs::read_to_string(&ch).unwrap();
    assert!(after.contains("id = \"ch_TESTAAAA\""), "id 头丢了：{after}");
    assert!(
        after.contains("title = \"脏正文\""),
        "title 头丢了：{after}"
    );
    assert!(after.starts_with("+++\n"), "头必须还在最前：{after}");
    // 正文被收拾了：段首补上全角缩进，多余空行压成一个。
    assert!(after.contains("　　没缩进。"), "正文没排：{after}");
    assert!(!after.contains("\n\n\n"), "多余空行没压：{after}");
}

// ---- history list ----

#[test]
fn history_list_of_a_chapter_without_snapshots() {
    let f = setup();
    let (out, code) = run(f.mj().args(["history", "list", "第一章"]));
    assert_eq!(code, 0);
    assert!(out.contains("还没有快照"), "{out}");
}

#[test]
fn history_list_not_found_exits_nonzero() {
    let f = setup();
    let (_, code) = run(f.mj().args(["history", "list", "查无此章"]));
    assert_eq!(code, 1, "找不到章要非零退出，好让脚本发现");
}

#[test]
fn history_list_ambiguous_exits_nonzero_and_lists_candidates() {
    let f = setup();
    // 「第」同时匹配两章。
    let out = f.mj().args(["history", "list", "第"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("第一章") && err.contains("第二章"),
        "候选要列出来：{err}"
    );
}

/// 打过快照的章，列出来要看得见快照的 id、标签、触发原因。
#[test]
fn history_list_shows_a_seeded_snapshot() {
    let f = setup();
    // 直接用 mj-core 在磁盘上种一个快照（等价于 TUI 里按 F9）。
    let book_dir = Workspace::resolve(Some(f.path().to_path_buf()))
        .unwrap()
        .books_dir()
        .join(f.book.to_string());
    let history = History::new(&book_dir);
    let snap = history
        .snapshot(
            f.ch1,
            "　　雪落了一夜。他推开门。\n",
            Trigger::Manual,
            Some("投稿版".into()),
            mj_core::history::Retention::Thinned,
        )
        .unwrap()
        .expect("该建出一个快照");

    let (out, code) = run(f.mj().args(["history", "list", "第一章"]));
    assert_eq!(code, 0);
    assert!(out.contains(&snap.id.to_string()), "要列出快照 id：{out}");
    assert!(out.contains("投稿版"), "用户命名的标签要显示：{out}");
    assert!(out.contains("1 个"), "计数要对：{out}");
}
