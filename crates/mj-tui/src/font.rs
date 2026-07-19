//! FontController、终端探测与配置片段生成。见 doc.md §2.1、§6.10。
//!
//! # 三级降级（§2.1）
//!
//! 1. **能改**：探测到支持的终端 → 运行时切字体族/字号，退出时恢复。
//! 2. **不能改字体、但能改观感**：外观预设（主题/栏宽/留白）——绝大多数用户
//!    实际感知到的「换字体」其实是这一层，见 `theme.rs`。
//! 3. **什么都不能改**：`[MUST]` 生成对应终端的配置片段，让用户粘贴后重启终端。
//!    这是把「做不到」变成「帮你做到」的关键，文档特意点了「不要省」。
//!
//! # `[VERIFY]` 未在真机验证的部分
//!
//! §2.1 的能力表标了 `[VERIFY]`：**不得照抄文档表格**，各终端实际能力要在真机上验。
//! 本文件的能力判定目前是**按文档实现、未经真机验证**的：
//! - OSC 50 改字体族：xterm/urxvt 系号称支持，未验；空参数是否等于「恢复默认」尤其存疑。
//! - kitty 远程控制：需终端侧 `allow_remote_control`，没开时 `kitty @` 会失败——
//!   我们据此判定失败并退回不可用，这条**逻辑**是可靠的（真去跑一次，看结果）。
//! - WezTerm 运行时改字体族需终端侧配合，故这里按「不可用 + 给配置片段」处理。
//!
//! 结论：`mj doctor` 才是判定真实能力的手段——它在**用户自己的终端里**跑一遍并报告。
//! 用户看到的能力描述一律来自实际探测，不来自这张表。

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FontCap: u8 {
        const SET_FAMILY = 0b001;
        const SET_SIZE   = 0b010;
        const RESET      = 0b100;
    }
}

/// 本进程是否真的改过终端字体。
///
/// 退出与 panic 路径据此决定要不要发恢复序列——没改过就别发，
/// 免得给不相干的终端塞一段它可能看不懂的转义。
static FONT_CHANGED: AtomicBool = AtomicBool::new(false);

fn mark_font_changed() {
    FONT_CHANGED.store(true, Ordering::SeqCst);
}

pub trait FontController: Send {
    fn id(&self) -> &'static str;
    fn caps(&self) -> FontCap;
    fn set_family(&mut self, family: &str) -> anyhow::Result<()>;
    fn set_size(&mut self, pt: f32) -> anyhow::Result<()>;
    fn reset(&mut self) -> anyhow::Result<()>;
}

/// 探测到的终端种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalKind {
    #[default]
    Unknown,
    Kitty,
    WezTerm,
    /// xterm / urxvt 系（OSC 50）。
    XtermLike,
    Alacritty,
    WindowsTerminal,
    VsCode,
    AppleTerminal,
    ITerm2,
}

impl TerminalKind {
    /// 按环境变量探测。见 §6.10 的 `detect()` 注释。
    pub fn detect() -> Self {
        Self::from_env(&EnvProbe::from_process())
    }

    /// 可测的探测核心。
    ///
    /// 顺序有讲究：先认**专属变量**（KITTY_WINDOW_ID / WEZTERM_PANE / WT_SESSION），
    /// 它们比 TERM 可靠得多——TERM 常被 tmux、ssh、或用户自己改成 `xterm-256color`，
    /// 照着它判定会把一堆终端误认成 xterm 而发出根本不生效的 OSC 50。
    pub fn from_env(e: &EnvProbe) -> Self {
        if e.kitty_window_id.is_some() {
            return Self::Kitty;
        }
        if e.wezterm_pane.is_some() {
            return Self::WezTerm;
        }
        if e.wt_session.is_some() {
            return Self::WindowsTerminal;
        }
        if let Some(tp) = e.term_program.as_deref() {
            let tp = tp.to_ascii_lowercase();
            if tp.contains("wezterm") {
                return Self::WezTerm;
            }
            if tp.contains("iterm") {
                return Self::ITerm2;
            }
            if tp.contains("vscode") {
                return Self::VsCode;
            }
            if tp.contains("apple_terminal") {
                return Self::AppleTerminal;
            }
            if tp.contains("alacritty") {
                return Self::Alacritty;
            }
        }
        if let Some(t) = e.term.as_deref() {
            let t = t.to_ascii_lowercase();
            if t.contains("kitty") {
                return Self::Kitty;
            }
            if t.contains("alacritty") {
                return Self::Alacritty;
            }
            // 只有明确是 xterm/urxvt 系才认 OSC 50。
            if t.starts_with("xterm") || t.starts_with("rxvt") || t.starts_with("urxvt") {
                return Self::XtermLike;
            }
        }
        Self::Unknown
    }

    /// 给人看的名字——「当前终端（Alacritty）不支持运行时更改字体」里那个词。
    pub fn label(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::WezTerm => "WezTerm",
            Self::XtermLike => "xterm/urxvt 系",
            Self::Alacritty => "Alacritty",
            Self::WindowsTerminal => "Windows Terminal",
            Self::VsCode => "VS Code 内置终端",
            Self::AppleTerminal => "Apple Terminal",
            Self::ITerm2 => "iTerm2",
            Self::Unknown => "未知终端",
        }
    }

    /// 该终端**预期**的字体能力。
    ///
    /// 注意「预期」二字：这是按 §2.1 的表给的先验，尚未真机验证（见文件头）。
    /// 真实能力以 `probe()` 的结果为准——比如 kitty 没开 allow_remote_control 时，
    /// 这里说能改字号，实际却改不了。
    pub fn expected_caps(self) -> FontCap {
        match self {
            // kitty：仅字号，走远程控制。
            Self::Kitty => FontCap::SET_SIZE | FontCap::RESET,
            // xterm/urxvt：OSC 50 改字体族。
            Self::XtermLike => FontCap::SET_FAMILY | FontCap::RESET,
            // 其余一律没有运行时能力——诚实地给空，UI 据此显示灰态。
            _ => FontCap::empty(),
        }
    }

    /// 不支持时给用户的一句话原因（§6.10 [MUST]：灰态 + 原因）。
    pub fn unsupported_reason(self) -> String {
        match self {
            Self::Kitty => "kitty 只能改字号，且需终端侧开启 allow_remote_control".into(),
            Self::XtermLike => "xterm/urxvt 系可改字体族，字号支持视具体终端而定".into(),
            Self::WezTerm => {
                "当前终端（WezTerm）运行时更改字体需终端侧配合，请用下方配置片段".into()
            }
            other => format!("当前终端（{}）不支持运行时更改字体", other.label()),
        }
    }

    /// 该终端的配置文件名，用于提示用户往哪贴。
    pub fn config_file_hint(self) -> Option<&'static str> {
        match self {
            Self::Kitty => Some("kitty.conf"),
            Self::WezTerm => Some("wezterm.lua"),
            Self::Alacritty => Some("alacritty.toml"),
            Self::WindowsTerminal => Some("Windows Terminal 的 settings.json"),
            Self::VsCode => Some("VS Code settings.json"),
            _ => None,
        }
    }
}

/// 探测用到的环境变量。抽成结构体是为了能在测试里构造，不必去动进程环境
/// （改进程环境在并行测试下会互相打架）。
#[derive(Debug, Clone, Default)]
pub struct EnvProbe {
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub kitty_window_id: Option<String>,
    pub wezterm_pane: Option<String>,
    pub wt_session: Option<String>,
    pub colorterm: Option<String>,
}

impl EnvProbe {
    pub fn from_process() -> Self {
        let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Self {
            term: v("TERM"),
            term_program: v("TERM_PROGRAM"),
            kitty_window_id: v("KITTY_WINDOW_ID"),
            wezterm_pane: v("WEZTERM_PANE"),
            wt_session: v("WT_SESSION"),
            colorterm: v("COLORTERM"),
        }
    }
}

/// 生成可粘贴的配置片段（§6.10 [MUST]）。
///
/// 「做不到」不等于「没辙」：改不了就把该改的配置替用户写好。返回 None 表示
/// 我们不知道这个终端的配置长什么样——那时候硬编一段假配置比不给更糟。
pub fn config_snippet(kind: TerminalKind, family: &str, size: f32) -> Option<String> {
    // 字号用整数呈现更像人手写的配置；带小数时保留一位。
    let size_s = if (size.fract()).abs() < f32::EPSILON {
        format!("{}", size as i64)
    } else {
        format!("{size:.1}")
    };
    Some(match kind {
        TerminalKind::Kitty => {
            format!("# ~/.config/kitty/kitty.conf\nfont_family {family}\nfont_size {size_s}\n")
        }
        TerminalKind::WezTerm => format!(
            "-- ~/.config/wezterm/wezterm.lua\nconfig.font = wezterm.font(\"{family}\")\nconfig.font_size = {size_s}\n"
        ),
        TerminalKind::Alacritty => format!(
            "# ~/.config/alacritty/alacritty.toml\n[font]\nsize = {size_s}\n\n[font.normal]\nfamily = \"{family}\"\n"
        ),
        TerminalKind::WindowsTerminal => format!(
            "// Windows Terminal settings.json —— 放进对应 profile 里\n\"font\": {{\n    \"face\": \"{family}\",\n    \"size\": {size_s}\n}}\n"
        ),
        TerminalKind::VsCode => format!(
            "// VS Code settings.json\n\"terminal.integrated.fontFamily\": \"{family}\",\n\"terminal.integrated.fontSize\": {size_s}\n"
        ),
        // 这几个要么没有稳定的配置文件格式（Apple Terminal 走图形界面），
        // 要么我们说不准（Unknown）。不猜。
        TerminalKind::XtermLike
        | TerminalKind::AppleTerminal
        | TerminalKind::ITerm2
        | TerminalKind::Unknown => {
            return None;
        }
    })
}

// ---- 后端 ----

/// 什么都做不到的后端——§2.1 的「三级」：Alacritty / WT / VS Code 内置终端等。
///
/// 不是失败，是诚实：UI 应据 `caps()` 显示灰态与原因，而不是假装能改。
#[derive(Debug, Default)]
pub struct NoopFont {
    kind: TerminalKind,
}

impl NoopFont {
    pub fn new(kind: TerminalKind) -> Self {
        Self { kind }
    }
}

impl FontController for NoopFont {
    fn id(&self) -> &'static str {
        "noop"
    }

    fn caps(&self) -> FontCap {
        FontCap::empty()
    }

    fn set_family(&mut self, _family: &str) -> anyhow::Result<()> {
        anyhow::bail!("{}", self.kind.unsupported_reason())
    }

    fn set_size(&mut self, _pt: f32) -> anyhow::Result<()> {
        anyhow::bail!("{}", self.kind.unsupported_reason())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// xterm / urxvt 系：OSC 50 改字体族。
///
/// `ESC ] 50 ; <font> BEL`。空参数**据称**是恢复默认——`[VERIFY]`，未真机验证。
#[derive(Debug, Default)]
pub struct Osc50Font;

impl Osc50Font {
    fn emit(&self, payload: &str) -> anyhow::Result<()> {
        let mut out = std::io::stdout();
        write!(out, "\x1b]50;{payload}\x07")?;
        out.flush()?;
        mark_font_changed();
        Ok(())
    }
}

impl FontController for Osc50Font {
    fn id(&self) -> &'static str {
        "osc50"
    }

    fn caps(&self) -> FontCap {
        FontCap::SET_FAMILY | FontCap::RESET
    }

    fn set_family(&mut self, family: &str) -> anyhow::Result<()> {
        self.emit(family)
    }

    fn set_size(&mut self, _pt: f32) -> anyhow::Result<()> {
        // OSC 50 的字号语法各家不一（xterm 有 `#+1` 之类的相对档位），
        // 没把握就别装作能做——UI 会据 caps() 把字号那栏置灰。
        anyhow::bail!("xterm/urxvt 系不支持直接指定字号，请用配置片段")
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.emit("")
    }
}

/// kitty：仅字号，走远程控制 `kitty @ set-font-size`。
///
/// 需终端侧 `allow_remote_control`。没开时命令会失败，我们如实报错——
/// 这条判定是可靠的：真去跑一次，看退出码。
#[derive(Debug, Default)]
pub struct KittyFont;

impl KittyFont {
    fn run(args: &[&str]) -> anyhow::Result<()> {
        let out = std::process::Command::new("kitty")
            .arg("@")
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("调不起 kitty 远程控制：{e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "kitty 远程控制被拒（多半是终端侧没开 allow_remote_control）：{}",
                err.trim()
            );
        }
        mark_font_changed();
        Ok(())
    }
}

impl FontController for KittyFont {
    fn id(&self) -> &'static str {
        "kitty"
    }

    fn caps(&self) -> FontCap {
        FontCap::SET_SIZE | FontCap::RESET
    }

    fn set_family(&mut self, _family: &str) -> anyhow::Result<()> {
        anyhow::bail!("kitty 只能改字号，改字体族请用配置片段")
    }

    fn set_size(&mut self, pt: f32) -> anyhow::Result<()> {
        Self::run(&["set-font-size", &format!("{pt}")])
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        // 0 = 回到 kitty.conf 里配的字号。
        Self::run(&["set-font-size", "0"])
    }
}

/// 依次探测，返回可用后端；都不可用返回 `NoopFont`（§6.10）。
pub fn detect() -> Box<dyn FontController> {
    for_kind(TerminalKind::detect())
}

/// 按终端种类取后端。分出来是为了能测——`detect()` 依赖进程环境，测不了。
pub fn for_kind(kind: TerminalKind) -> Box<dyn FontController> {
    match kind {
        TerminalKind::Kitty => Box::new(KittyFont),
        TerminalKind::XtermLike => Box::new(Osc50Font),
        // WezTerm 运行时改字体族要终端侧配合，按不可用处理 + 给 lua 片段（§2.1）。
        other => Box::new(NoopFont::new(other)),
    }
}

/// 直接向 stdout 发送「恢复默认字体」的 OSC 50 序列。
///
/// 专供 panic hook 与退出路径：这两处不能依赖 `FontController`（可能正持锁，
/// 或本身就是 panic 的来源）。对不支持 OSC 50 的终端无副作用——
/// 它们会忽略未知的 OSC 序列。
///
/// `[VERIFY]` 空参数的 OSC 50 是否确为「恢复默认」，仍需真机验证。
pub fn emit_reset_sequence() {
    // 没改过字体就什么都不用做。
    if !FONT_CHANGED.load(Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b]50;\x07");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn env(term: Option<&str>, prog: Option<&str>) -> EnvProbe {
        EnvProbe {
            term: term.map(|s| s.into()),
            term_program: prog.map(|s| s.into()),
            ..EnvProbe::default()
        }
    }

    // ---- 终端探测 ----

    #[test]
    fn kitty_detected_by_its_own_var() {
        let e = EnvProbe {
            kitty_window_id: Some("1".into()),
            // 故意把 TERM 设成 xterm：专属变量应当压过它。
            term: Some("xterm-256color".into()),
            ..EnvProbe::default()
        };
        assert_eq!(TerminalKind::from_env(&e), TerminalKind::Kitty);
    }

    #[test]
    fn wezterm_and_wt_detected_by_their_vars() {
        let e = EnvProbe {
            wezterm_pane: Some("0".into()),
            ..EnvProbe::default()
        };
        assert_eq!(TerminalKind::from_env(&e), TerminalKind::WezTerm);

        let e = EnvProbe {
            wt_session: Some("abc".into()),
            ..EnvProbe::default()
        };
        assert_eq!(TerminalKind::from_env(&e), TerminalKind::WindowsTerminal);
    }

    /// TERM 常被 tmux/ssh/用户改成 xterm-256color——不能仅凭它就当 xterm，
    /// 否则会给一堆终端发根本不生效的 OSC 50。专属变量优先正是为此。
    #[test]
    fn terminal_specific_vars_beat_term() {
        let e = EnvProbe {
            kitty_window_id: Some("7".into()),
            term: Some("xterm-256color".into()),
            term_program: Some("WezTerm".into()),
            ..EnvProbe::default()
        };
        assert_eq!(
            TerminalKind::from_env(&e),
            TerminalKind::Kitty,
            "KITTY_WINDOW_ID 最硬"
        );
    }

    #[test]
    fn detects_by_term_program() {
        assert_eq!(
            TerminalKind::from_env(&env(None, Some("iTerm.app"))),
            TerminalKind::ITerm2
        );
        assert_eq!(
            TerminalKind::from_env(&env(None, Some("vscode"))),
            TerminalKind::VsCode
        );
        assert_eq!(
            TerminalKind::from_env(&env(None, Some("Apple_Terminal"))),
            TerminalKind::AppleTerminal
        );
    }

    #[test]
    fn detects_xterm_family_by_term() {
        assert_eq!(
            TerminalKind::from_env(&env(Some("xterm-256color"), None)),
            TerminalKind::XtermLike
        );
        assert_eq!(
            TerminalKind::from_env(&env(Some("rxvt-unicode"), None)),
            TerminalKind::XtermLike
        );
    }

    #[test]
    fn unknown_when_nothing_matches() {
        assert_eq!(
            TerminalKind::from_env(&EnvProbe::default()),
            TerminalKind::Unknown
        );
        assert_eq!(
            TerminalKind::from_env(&env(Some("dumb"), None)),
            TerminalKind::Unknown
        );
    }

    // ---- 能力与降级 ----

    #[test]
    fn unsupported_terminals_report_no_caps() {
        for k in [
            TerminalKind::Alacritty,
            TerminalKind::WindowsTerminal,
            TerminalKind::VsCode,
            TerminalKind::AppleTerminal,
            TerminalKind::Unknown,
        ] {
            assert!(k.expected_caps().is_empty(), "{k:?} 不该声称有字体能力");
        }
    }

    #[test]
    fn kitty_only_claims_size() {
        let c = TerminalKind::Kitty.expected_caps();
        assert!(c.contains(FontCap::SET_SIZE));
        assert!(!c.contains(FontCap::SET_FAMILY), "kitty 改不了字体族");
    }

    #[test]
    fn xterm_only_claims_family() {
        let c = TerminalKind::XtermLike.expected_caps();
        assert!(c.contains(FontCap::SET_FAMILY));
        assert!(!c.contains(FontCap::SET_SIZE));
    }

    /// §6.10 [MUST]：不支持时要有一句话原因，且必须点出是哪个终端。
    #[test]
    fn unsupported_reason_names_the_terminal() {
        let r = TerminalKind::Alacritty.unsupported_reason();
        assert!(r.contains("Alacritty"), "{r}");
        let r = TerminalKind::WindowsTerminal.unsupported_reason();
        assert!(r.contains("Windows Terminal"), "{r}");
    }

    #[test]
    fn noop_backend_refuses_but_reset_is_safe() {
        let mut f = NoopFont::new(TerminalKind::Alacritty);
        assert!(f.caps().is_empty());
        let e = f.set_family("Source Han Serif").unwrap_err().to_string();
        assert!(e.contains("Alacritty"), "报错要说清为什么：{e}");
        assert!(f.set_size(14.0).is_err());
        assert!(f.reset().is_ok(), "reset 应总是安全的");
    }

    #[test]
    fn for_kind_picks_the_right_backend() {
        assert_eq!(for_kind(TerminalKind::Kitty).id(), "kitty");
        assert_eq!(for_kind(TerminalKind::XtermLike).id(), "osc50");
        assert_eq!(for_kind(TerminalKind::Alacritty).id(), "noop");
        assert_eq!(for_kind(TerminalKind::WezTerm).id(), "noop");
    }

    // ---- 配置片段（§6.10 [MUST]）----

    #[test]
    fn kitty_snippet_has_both_fields() {
        let s = config_snippet(TerminalKind::Kitty, "Source Han Serif", 14.0).unwrap();
        assert!(s.contains("font_family Source Han Serif"), "{s}");
        assert!(s.contains("font_size 14"), "{s}");
        assert!(s.contains("kitty.conf"), "要告诉用户贴到哪个文件：{s}");
    }

    #[test]
    fn alacritty_snippet_is_valid_toml() {
        let s = config_snippet(TerminalKind::Alacritty, "Source Han Serif", 13.5).unwrap();
        // 片段本身要能被 TOML 解析——贴过去不能是一段语法错的东西。
        let stripped: String = s
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let v: toml::Value = toml::from_str(&stripped).expect("alacritty 片段应是合法 TOML");
        assert_eq!(
            v["font"]["normal"]["family"].as_str(),
            Some("Source Han Serif")
        );
        assert!(s.contains("13.5"), "带小数的字号要保留小数：{s}");
    }

    #[test]
    fn wezterm_snippet_is_lua() {
        let s = config_snippet(TerminalKind::WezTerm, "Source Han Serif", 14.0).unwrap();
        assert!(s.contains("wezterm.font(\"Source Han Serif\")"), "{s}");
        assert!(s.contains("config.font_size = 14"), "{s}");
    }

    #[test]
    fn windows_terminal_snippet_mentions_profile() {
        let s = config_snippet(TerminalKind::WindowsTerminal, "等距更纱黑体", 14.0).unwrap();
        assert!(s.contains("\"face\": \"等距更纱黑体\""), "{s}");
        assert!(s.contains("profile"), "要说明贴进哪个 profile：{s}");
    }

    /// 说不准的终端就不给片段——硬编一段假配置比不给更糟。
    #[test]
    fn no_snippet_when_we_dont_know_the_format() {
        assert!(config_snippet(TerminalKind::Unknown, "X", 14.0).is_none());
        assert!(config_snippet(TerminalKind::AppleTerminal, "X", 14.0).is_none());
    }

    #[test]
    fn config_file_hint_matches_snippet_targets() {
        // 有片段的终端都该说得出配置文件名，反之亦然。
        for k in [
            TerminalKind::Kitty,
            TerminalKind::WezTerm,
            TerminalKind::Alacritty,
            TerminalKind::WindowsTerminal,
            TerminalKind::VsCode,
        ] {
            assert!(config_snippet(k, "X", 14.0).is_some(), "{k:?} 该有片段");
            assert!(k.config_file_hint().is_some(), "{k:?} 该有配置文件名");
        }
    }

    #[test]
    fn emit_reset_is_safe_to_call() {
        emit_reset_sequence(); // 不得 panic
    }
}
