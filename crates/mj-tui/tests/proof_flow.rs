//! 校对面板 F7 的端到端流程。见 doc.md §6.8。
//!
//! 走真实按键（`press_for_test` → `on_key`），断言磁盘/缓冲的实际变化，
//! 不走 demo 钩子——只在钩子里跑过的功能等于没验证过用户按不按得到它。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mj_core::config::Config;
use mj_core::id::{BookId, ChapterId};
use mj_core::store::Store;
use mj_core::workspace::Workspace;
use mj_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

struct Fixture {
    dir: tempfile::TempDir,
    book: BookId,
    ch: ChapterId,
}

fn setup(body: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::resolve(Some(dir.path().to_path_buf())).unwrap();
    ws.ensure_layout().unwrap();
    let mut store = Store::new(ws, Config::default());
    let book = store.create_book("雪夜行", "沈砚").unwrap();
    let vol = store.create_volume(book.id, "第一卷", None).unwrap();
    let ch = store.create_chapter(book.id, vol, "第一章", None).unwrap();
    store
        .save_body(book.id, &mj_core::model::ChapterBody::new(ch, body))
        .unwrap();
    Fixture {
        dir,
        book: book.id,
        ch,
    }
}

impl Fixture {
    fn store(&self) -> Store {
        let ws = Workspace::resolve(Some(self.dir.path().to_path_buf())).unwrap();
        Store::new(ws, Config::default())
    }

    fn app(&self) -> App {
        let mut app = App::new(self.store(), Config::default()).unwrap();
        app.open_first_book_for_demo().unwrap();
        app.open_chapter_for_test(self.ch).unwrap();
        app
    }

    fn add_character(&self, name: &str) {
        let mut store = self.store();
        store.create_character(self.book, name).unwrap();
    }
}

/// F7 打开面板并报出错别字。
#[test]
fn f7_finds_a_typo() {
    let f = setup("现场气氛如火如茶，众人叫好。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(1), "该报出「如火如茶」");
}

/// 干净文本：面板开着但没有问题。
#[test]
fn f7_on_clean_text_shows_nothing() {
    let f = setup("他推开门，风雪扑面而来，冷得他打了个寒战。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(0));
}

/// `a` 应用建议：缓冲里的错别字被改正。
#[test]
fn apply_suggestion_fixes_the_buffer() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Char('a'), NONE).unwrap();

    let text = app.buffer_text_for_test().unwrap();
    assert!(text.contains("如火如荼"), "应改成正确写法：{text:?}");
    assert!(!text.contains("如火如茶"), "错写不该还在：{text:?}");
    // 改完重新校对，那条应消失。
    assert_eq!(app.proof_visible_for_test(), Some(0));
}

/// Enter 跳转：关面板、光标落到问题处、焦点回编辑器。
#[test]
fn enter_jumps_and_closes_panel() {
    let f = setup("开头一句。\n\n现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Enter, NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), None, "跳转后面板关闭");
}

/// `I` 永久忽略：写进 dict/ignore.json，下次校对不再报。
#[test]
fn permanent_ignore_persists_across_reopen() {
    let f = setup("现场气氛如火如茶。\n");

    {
        let mut app = f.app();
        app.press_for_test(KeyCode::F(7), NONE).unwrap();
        assert_eq!(app.proof_visible_for_test(), Some(1));
        app.press_for_test(KeyCode::Char('I'), NONE).unwrap();
        assert_eq!(app.proof_visible_for_test(), Some(0), "忽略后当场消失");
    }

    // ignore.json 应已落盘。
    let ignore = f.dir.path().join("dict").join("ignore.json");
    assert!(ignore.exists(), "永久忽略应写入 dict/ignore.json");

    // 重开一个 App，再 F7，那条不该再出现。
    let mut app2 = f.app();
    app2.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(
        app2.proof_visible_for_test(),
        Some(0),
        "已忽略的问题跨会话不再出现"
    );
}

/// `i` 本次忽略：只从列表摘掉，不落盘。
#[test]
fn session_ignore_does_not_persist() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    app.press_for_test(KeyCode::Char('i'), NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), Some(0));

    let ignore = f.dir.path().join("dict").join("ignore.json");
    assert!(!ignore.exists(), "本次忽略不该写盘");
}

/// 角色名驱动一致性检查：与「沈砚」一字之差的「沈研」被标可疑。
#[test]
fn character_name_drives_consistency_check() {
    let f = setup("那天沈研走进门。\n");
    f.add_character("沈砚");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert_eq!(
        app.proof_visible_for_test(),
        Some(1),
        "应根据角色名报出「沈研」可疑"
    );
}

/// 按显示宽度读屏：TestBackend 里一个 CJK 占两格，第二格是空格，
/// 逐格拼会在每个汉字之间塞进空格，搜什么都搜不到。
fn screen_text(app: &mut App, w: u16, h: u16) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|fr| app.render_for_test(fr)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut x = 0;
        while x < buf.area.width {
            let s = buf[(x, y)].symbol();
            out.push_str(s);
            x += (UnicodeWidthStr::width(s) as u16).max(1);
        }
        out.push('\n');
    }
    out
}

fn draw_ok(app: &mut App, w: u16, h: u16) -> bool {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render_for_test(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    !text.contains('\u{fffd}')
}

/// 面板渲染 + 关面板后正文下划线：各档宽度都不撕屏（§6.8 [MUST] 命中着色、§10）。
#[test]
fn proof_panel_and_underlines_render_across_widths() {
    let f = setup("现场气氛如火如茶，他跑的很快。\n");
    for (w, h) in [(60, 20), (80, 24), (120, 30), (200, 50)] {
        let mut app = f.app();
        app.press_for_test(KeyCode::F(7), NONE).unwrap();
        assert!(draw_ok(&mut app, w, h), "校对面板在 {w}x{h} 撕屏了");
        // Enter 关面板、留下划线，正文再画一遍。
        app.press_for_test(KeyCode::Enter, NONE).unwrap();
        assert!(draw_ok(&mut app, w, h), "正文下划线在 {w}x{h} 撕屏了");
    }
}

/// 外部后端的问题要真的出现在 F7 面板里（§6.8 的 ExternalProofreader）。
///
/// 用 `sh` 假装一个外部校对程序，故只在 unix 上跑；契约解析与偏移映射的
/// 平台无关部分由 mj-core 的单测覆盖。
#[cfg(unix)]
#[test]
fn external_backend_issues_reach_the_panel() {
    let f = setup("现场气氛好得很。\n");
    let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
    // 让外部程序在第 0 段的第 0..2 个**字符**上报一处问题。
    let script = r#"cat >/dev/null; printf '{"v":1,"issues":[{"para":0,"start":0,"end":2,"category":"Grammar","message":"外部报的问题","suggestions":[],"confidence":0.7}]}'"#;
    std::fs::write(
        ws.config_file(),
        format!(
            "[proof.external]\nenabled = true\ncommand = [\"sh\", \"-c\", \"{}\"]\n",
            script.replace('\\', "\\\\").replace('"', "\\\"")
        ),
    )
    .unwrap();

    let config = Config::load(&ws.config_file()).unwrap();
    let store = Store::new(ws, config.clone());
    let mut app = App::new(store, config).unwrap();
    app.open_first_book_for_demo().unwrap();
    app.open_chapter_for_test(f.ch).unwrap();

    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    let n = app.proof_visible_for_test().unwrap();
    assert!(n >= 1, "外部后端报的问题应出现在面板里，实际 {n} 条");
}

/// 外部后端坏掉时，校对照常完成、只多一句提示（§6.8「绝不影响编辑」）。
#[cfg(unix)]
#[test]
fn broken_external_backend_does_not_break_proofing() {
    let f = setup("现场气氛如火如茶。\n");
    let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
    std::fs::write(
        ws.config_file(),
        "[proof.external]\nenabled = true\ncommand = [\"sh\", \"-c\", \"exit 7\"]\n",
    )
    .unwrap();

    let config = Config::load(&ws.config_file()).unwrap();
    let store = Store::new(ws, config.clone());
    let mut app = App::new(store, config).unwrap();
    app.open_first_book_for_demo().unwrap();
    app.open_chapter_for_test(f.ch).unwrap();

    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    // 本地规则的那条错别字照样报出来。
    assert_eq!(
        app.proof_visible_for_test(),
        Some(1),
        "外部后端挂了，本地规则的结果不该跟着没"
    );
}

// ---- 模型校对的开启闸门（§6.8 第 3 条 [MUST]）----
//
// 这几个用例一律把 endpoint 指向 127.0.0.1:1（保留端口，连不上）。
// 否则开发机上真设了 ANTHROPIC_API_KEY 时，跑一次测试就会往外发一次真实请求
// ——测试不该花别人的钱。

/// 用给定的 [proof.llm] 配置起一个 App。
fn app_with_llm(f: &Fixture, extra_toml: &str) -> (Workspace, App) {
    let ws = Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap();
    std::fs::write(
        ws.config_file(),
        format!("[proof.llm]\nendpoint = \"http://127.0.0.1:1/\"\n{extra_toml}"),
    )
    .unwrap();
    let config = Config::load(&ws.config_file()).unwrap();
    let store = Store::new(
        Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap(),
        config.clone(),
    );
    let mut app = App::new(store, config).unwrap();
    app.open_first_book_for_demo().unwrap();
    app.open_chapter_for_test(f.ch).unwrap();
    (ws, app)
}

fn config_text(ws: &Workspace) -> String {
    std::fs::read_to_string(ws.config_file()).unwrap_or_default()
}

/// 没开：告诉用户怎么开，不弹框也不发请求。
#[test]
fn llm_off_by_default_and_says_how_to_enable() {
    let f = setup("正文。\n");
    let (_ws, mut app) = app_with_llm(&f, "");
    app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
        .unwrap();
    assert!(
        app.modal_stack_for_test().is_empty(),
        "没开就不该弹任何框：{:?}",
        app.modal_stack_for_test()
    );
    let t = app.toast_for_test().unwrap();
    assert!(t.contains("enabled"), "要告诉用户怎么开：{t}");
}

/// §6.8 [MUST]：开了但没同意 → 先弹说明，**不发正文**。
#[test]
fn enabling_without_consent_shows_the_dialog_first() {
    let f = setup("正文。\n");
    let (_ws, mut app) = app_with_llm(&f, "enabled = true\n");
    app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
        .unwrap();
    assert_eq!(
        app.modal_stack_for_test(),
        vec!["Consent".to_string()],
        "开了没同意应先弹说明框"
    );
}

/// 说明框默认停在「不同意」，Esc / n 都退出，且不写 consented。
#[test]
fn declining_consent_changes_nothing() {
    for key in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Enter] {
        let f = setup("正文。\n");
        let (ws, mut app) = app_with_llm(&f, "enabled = true\n");
        app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
            .unwrap();
        app.press_for_test(key, NONE).unwrap();

        assert!(
            app.modal_stack_for_test().is_empty(),
            "{key:?} 后说明框该关掉"
        );
        // Enter 走默认项——默认是「不同意」，手滑连按两下回车不该把稿子发出去。
        assert!(
            !config_text(&ws).contains("consented = true"),
            "{key:?}：不同意就不该落盘 consented\n{}",
            config_text(&ws)
        );
        let t = app.toast_for_test().unwrap_or("");
        assert!(t.contains("没有发出去"), "{key:?}：要明确说没发：{t}");
    }
}

/// 同意后要落盘，下次不再问（§6.8 说的是「首次开启」）。
#[test]
fn accepting_consent_persists_it() {
    let f = setup("正文。\n");
    let (ws, mut app) = app_with_llm(&f, "enabled = true\n");
    app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
        .unwrap();
    app.press_for_test(KeyCode::Char('y'), NONE).unwrap();

    assert!(app.modal_stack_for_test().is_empty(), "同意后说明框该关掉");
    assert!(
        config_text(&ws).contains("consented = true"),
        "同意要写回配置，否则每次都问\n{}",
        config_text(&ws)
    );
    // 重开一个 App：不该再被问一遍。
    let config = Config::load(&ws.config_file()).unwrap();
    let store = Store::new(
        Workspace::resolve(Some(f.dir.path().to_path_buf())).unwrap(),
        config.clone(),
    );
    let mut app2 = App::new(store, config).unwrap();
    app2.open_first_book_for_demo().unwrap();
    app2.open_chapter_for_test(f.ch).unwrap();
    app2.run_command_for_test(mj_tui::commands::Command::ProofLlm)
        .unwrap();
    assert!(
        !app2.modal_stack_for_test().contains(&"Consent".to_string()),
        "同意过就不该再问"
    );
}

/// 配置里写了明文密钥 → 拒跑并说清怎么改（§6.8 [MUST]：密钥不得明文入配置）。
#[test]
fn plaintext_key_in_config_is_refused() {
    let f = setup("正文。\n");
    let (_ws, mut app) = app_with_llm(
        &f,
        "enabled = true\nconsented = true\napi_key = \"sk-ant-secret\"\n",
    );
    app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
        .unwrap();
    let t = app.toast_for_test().unwrap();
    assert!(t.contains("环境变量"), "要告诉用户改用环境变量：{t}");
    assert!(!t.contains("sk-ant-secret"), "提示里不能回显密钥：{t}");
}

/// 说明框各档宽度都不撕屏，且**关键信息一个都不能少**（§10、§6.8 [MUST]）。
///
/// 这是唯一一个截断即失效的框：用户要据此决定把稿子发不发出去，
/// 窄屏下把地址切掉一半，就等于没告知。
#[test]
fn consent_dialog_stays_readable_across_widths() {
    let f = setup("正文。\n");
    for (w, h) in [(60, 24), (80, 24), (120, 30), (200, 50)] {
        let (_ws, mut app) = app_with_llm(&f, "enabled = true\n");
        app.run_command_for_test(mj_tui::commands::Command::ProofLlm)
            .unwrap();
        assert!(draw_ok(&mut app, w, h), "同意框在 {w}x{h} 撕屏了");

        let text = screen_text(&mut app, w, h);
        // 「墨简管不着」在最长那行的**末尾**——60 列下这行必须折行才留得住它，
        // 只挑短句断言等于没测（那些行本来就不会被截）。
        for must in [
            "127.0.0.1",
            "ANTHROPIC_API_KEY",
            "不会自动扫描全书",
            "墨简管不着",
        ] {
            assert!(
                text.replace('\n', "").contains(must),
                "{w}x{h}：说明里少了「{must}」\n{text}"
            );
        }
    }
}

/// Esc 关闭面板。
#[test]
fn esc_closes_panel() {
    let f = setup("现场气氛如火如茶。\n");
    let mut app = f.app();
    app.press_for_test(KeyCode::F(7), NONE).unwrap();
    assert!(app.proof_visible_for_test().is_some());
    app.press_for_test(KeyCode::Esc, NONE).unwrap();
    assert_eq!(app.proof_visible_for_test(), None);
}
