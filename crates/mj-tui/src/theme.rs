//! 主题与配色。见 doc.md §6.10、§2.1。
//!
//! `[MUST]` 探测 truecolor（`COLORTERM`），仅 256 色时自动降级取近似色——
//! 否则在老终端上主题会糊成一团，而用户只会觉得「这软件颜色是坏的」。
//!
//! 分层（§4）：主题**定义**是 TOML（`themes/*.toml`，用户可自建），读盘由
//! mj-core 负责；本模块只做「TOML 文本 → ratatui 颜色」的纯映射，故可整体单测。
//!
//! 内置四套（dark / light / sepia / high_contrast）编译进二进制，同时也是给用户
//! 照抄的格式范例。

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// 终端色深。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24 位真彩，直接下发 RGB。
    TrueColor,
    /// 仅 256 色，需取近似。
    Ansi256,
}

impl ColorDepth {
    /// 按环境变量探测（§6.10 [MUST]）。
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
            std::env::var("TERM_PROGRAM").ok().as_deref(),
        )
    }

    /// 可测的探测核心。
    ///
    /// `COLORTERM=truecolor|24bit` 是事实标准；此外 kitty/wezterm/iTerm 等
    /// 即便没设 COLORTERM 也支持真彩，按 TERM/TERM_PROGRAM 兜底认一下。
    /// 拿不准时**降级**而非上探：糊一点总比一片乱码强。
    pub fn from_env(
        colorterm: Option<&str>,
        term: Option<&str>,
        term_program: Option<&str>,
    ) -> Self {
        if let Some(ct) = colorterm {
            let ct = ct.to_ascii_lowercase();
            if ct.contains("truecolor") || ct.contains("24bit") {
                return Self::TrueColor;
            }
        }
        if let Some(t) = term {
            let t = t.to_ascii_lowercase();
            if t.contains("kitty") || t.contains("direct") || t.contains("truecolor") {
                return Self::TrueColor;
            }
        }
        if let Some(tp) = term_program {
            let tp = tp.to_ascii_lowercase();
            if tp.contains("wezterm") || tp.contains("iterm") || tp.contains("vscode") {
                return Self::TrueColor;
            }
        }
        Self::Ansi256
    }
}

/// 一个 RGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// 解析 `#rrggbb` / `rrggbb`（大小写不限）。
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Self(
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        ))
    }

    /// 按色深转成 ratatui 颜色。
    pub fn to_color(self, depth: ColorDepth) -> Color {
        match depth {
            ColorDepth::TrueColor => Color::Rgb(self.0, self.1, self.2),
            ColorDepth::Ansi256 => Color::Indexed(self.to_ansi256()),
        }
    }

    /// 取 xterm-256 调色板里最接近的一格。
    ///
    /// 256 色分三段：0–15 基础色（各终端定义不一，**不参与**匹配，免得主题
    /// 被用户的终端配色改得面目全非）、16–231 的 6×6×6 色立方、232–255 的 24 级灰。
    /// 在「色立方最近点」与「灰阶最近点」里取更近的那个。
    pub fn to_ansi256(self) -> u8 {
        let (cube_idx, cube_rgb) = self.nearest_cube();
        let (gray_idx, gray_rgb) = self.nearest_gray();
        if self.dist2(cube_rgb) <= self.dist2(gray_rgb) {
            cube_idx
        } else {
            gray_idx
        }
    }

    /// 6×6×6 色立方里最接近的一格。每档实际亮度为 0,95,135,175,215,255。
    fn nearest_cube(self) -> (u8, Rgb) {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let nearest_level = |v: u8| -> usize {
            let mut best = 0;
            let mut best_d = u32::MAX;
            for (i, l) in LEVELS.iter().enumerate() {
                let d = (v as i32 - *l as i32).unsigned_abs();
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
            best
        };
        let (ri, gi, bi) = (
            nearest_level(self.0),
            nearest_level(self.1),
            nearest_level(self.2),
        );
        let idx = 16 + 36 * ri + 6 * gi + bi;
        (idx as u8, Rgb(LEVELS[ri], LEVELS[gi], LEVELS[bi]))
    }

    /// 24 级灰阶（index 232..=255，亮度 8,18,…,238）里最接近的一格。
    fn nearest_gray(self) -> (u8, Rgb) {
        let avg = (self.0 as u32 + self.1 as u32 + self.2 as u32) / 3;
        let step = ((avg as i32 - 8) as f32 / 10.0).round().clamp(0.0, 23.0) as u8;
        let val = 8 + 10 * step;
        (232 + step, Rgb(val, val, val))
    }

    fn dist2(self, o: Rgb) -> u32 {
        let d = |a: u8, b: u8| {
            let x = a as i32 - b as i32;
            (x * x) as u32
        };
        d(self.0, o.0) + d(self.1, o.1) + d(self.2, o.2)
    }
}

/// 磁盘上的主题定义（`themes/*.toml`）。颜色写 `#rrggbb`。
///
/// 缺的槽位回落到内置主题的对应值——用户只想改两个颜色时不必抄全表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub fg: Option<String>,
    /// 次要文字（提示、未聚焦）。
    #[serde(default)]
    pub dim: Option<String>,
    /// 焦点边框、强调。
    #[serde(default)]
    pub accent: Option<String>,
    /// 普通边框。
    #[serde(default)]
    pub border: Option<String>,
    #[serde(default)]
    pub selection_bg: Option<String>,
    #[serde(default)]
    pub selection_fg: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    /// diff 增行 / 删行。
    #[serde(default)]
    pub insert: Option<String>,
    #[serde(default)]
    pub delete: Option<String>,
    #[serde(default)]
    pub status_bg: Option<String>,
    #[serde(default)]
    pub status_fg: Option<String>,
}

/// 解析好的主题：直接可用的 ratatui 颜色。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub error: Color,
    pub warning: Color,
    pub hint: Color,
    pub insert: Color,
    pub delete: Color,
    pub status_bg: Color,
    pub status_fg: Color,
}

/// 内置主题的 TOML（同时是给用户照抄的范例）。
pub const BUILTIN_DARK: &str = include_str!("themes/dark.toml");
pub const BUILTIN_LIGHT: &str = include_str!("themes/light.toml");
pub const BUILTIN_SEPIA: &str = include_str!("themes/sepia.toml");
pub const BUILTIN_HIGH_CONTRAST: &str = include_str!("themes/high_contrast.toml");

/// 内置主题名 → TOML。
pub fn builtin(name: &str) -> Option<&'static str> {
    match name {
        "dark" => Some(BUILTIN_DARK),
        "light" => Some(BUILTIN_LIGHT),
        "sepia" => Some(BUILTIN_SEPIA),
        "high_contrast" => Some(BUILTIN_HIGH_CONTRAST),
        _ => None,
    }
}

/// 全部内置主题名。
pub fn builtin_names() -> &'static [&'static str] {
    &["dark", "light", "sepia", "high_contrast"]
}

impl Theme {
    /// 由 spec 解析。缺失/非法的颜色回落到 `fallback` 的对应槽位。
    ///
    /// 非法颜色不报错、只回落并记日志：主题是外观，写错一行不该让人打不开稿子。
    pub fn from_spec(spec: &ThemeSpec, depth: ColorDepth, fallback: &Theme) -> Self {
        let pick = |v: &Option<String>, fb: Color, slot: &str| -> Color {
            match v {
                None => fb,
                Some(s) => match Rgb::parse(s) {
                    Some(rgb) => rgb.to_color(depth),
                    None => {
                        tracing::warn!(slot, value = %s, "主题颜色非法，回落到默认");
                        fb
                    }
                },
            }
        };
        Self {
            bg: pick(&spec.bg, fallback.bg, "bg"),
            fg: pick(&spec.fg, fallback.fg, "fg"),
            dim: pick(&spec.dim, fallback.dim, "dim"),
            accent: pick(&spec.accent, fallback.accent, "accent"),
            border: pick(&spec.border, fallback.border, "border"),
            selection_bg: pick(&spec.selection_bg, fallback.selection_bg, "selection_bg"),
            selection_fg: pick(&spec.selection_fg, fallback.selection_fg, "selection_fg"),
            error: pick(&spec.error, fallback.error, "error"),
            warning: pick(&spec.warning, fallback.warning, "warning"),
            hint: pick(&spec.hint, fallback.hint, "hint"),
            insert: pick(&spec.insert, fallback.insert, "insert"),
            delete: pick(&spec.delete, fallback.delete, "delete"),
            status_bg: pick(&spec.status_bg, fallback.status_bg, "status_bg"),
            status_fg: pick(&spec.status_fg, fallback.status_fg, "status_fg"),
        }
    }

    /// 兜底主题：不依赖任何解析，永远可用。用终端的具名色，
    /// 这样即便在 8 色终端上也不会瞎。
    pub fn fallback_dark() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Cyan,
            border: Color::DarkGray,
            selection_bg: Color::Cyan,
            selection_fg: Color::Black,
            error: Color::Red,
            warning: Color::Yellow,
            hint: Color::DarkGray,
            insert: Color::Green,
            delete: Color::Red,
            status_bg: Color::Reset,
            status_fg: Color::Reset,
        }
    }

    /// 按名字取内置主题；未知名字回落到 sepia（§8 的默认值）并记日志。
    pub fn load_builtin(name: &str, depth: ColorDepth) -> Self {
        let fallback = Self::fallback_dark();
        let toml_text = match builtin(name) {
            Some(t) => t,
            None => {
                tracing::warn!(theme = name, "未知内置主题，回落 sepia");
                BUILTIN_SEPIA
            }
        };
        match toml::from_str::<ThemeSpec>(toml_text) {
            Ok(spec) => Self::from_spec(&spec, depth, &fallback),
            Err(e) => {
                tracing::warn!(theme = name, error = %e, "内置主题解析失败，用兜底配色");
                fallback
            }
        }
    }

    /// 由用户主题文本解析（`themes/<name>.toml` 的内容）。解析失败回落内置。
    pub fn from_toml(text: &str, depth: ColorDepth, fallback_name: &str) -> Self {
        let fb = Self::load_builtin(fallback_name, depth);
        match toml::from_str::<ThemeSpec>(text) {
            Ok(spec) => Self::from_spec(&spec, depth, &fb),
            Err(e) => {
                tracing::warn!(error = %e, "用户主题解析失败，回落内置主题");
                fb
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    // ---- 色深探测 ----

    #[test]
    fn colorterm_truecolor_detected() {
        assert_eq!(
            ColorDepth::from_env(Some("truecolor"), None, None),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_env(Some("24bit"), None, None),
            ColorDepth::TrueColor
        );
    }

    #[test]
    fn plain_term_falls_back_to_256() {
        assert_eq!(
            ColorDepth::from_env(None, Some("xterm-256color"), None),
            ColorDepth::Ansi256,
            "拿不准要降级，不要上探"
        );
        assert_eq!(ColorDepth::from_env(None, None, None), ColorDepth::Ansi256);
    }

    #[test]
    fn kitty_and_wezterm_detected_without_colorterm() {
        assert_eq!(
            ColorDepth::from_env(None, Some("xterm-kitty"), None),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_env(None, None, Some("WezTerm")),
            ColorDepth::TrueColor
        );
    }

    // ---- 颜色解析 ----

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!(Rgb::parse("#ff8000"), Some(Rgb(255, 128, 0)));
        assert_eq!(Rgb::parse("FF8000"), Some(Rgb(255, 128, 0)));
    }

    #[test]
    fn rejects_bad_hex() {
        assert_eq!(Rgb::parse("#fff"), None, "三位简写不收");
        assert_eq!(Rgb::parse("#gggggg"), None);
        assert_eq!(Rgb::parse(""), None);
    }

    // ---- 256 色降级 ----

    #[test]
    fn truecolor_passes_rgb_through() {
        assert_eq!(
            Rgb(18, 52, 86).to_color(ColorDepth::TrueColor),
            Color::Rgb(18, 52, 86)
        );
    }

    #[test]
    fn pure_colors_map_to_cube_corners() {
        // 纯黑/纯白/纯红在色立方上有精确对应点。
        assert_eq!(Rgb(0, 0, 0).to_ansi256(), 16);
        assert_eq!(Rgb(255, 255, 255).to_ansi256(), 231);
        assert_eq!(Rgb(255, 0, 0).to_ansi256(), 196);
    }

    #[test]
    fn grays_prefer_the_gray_ramp() {
        // 中灰在灰阶上比在色立方上更接近，应落到 232..=255。
        let idx = Rgb(128, 128, 128).to_ansi256();
        assert!((232..=255).contains(&idx), "中灰应走灰阶，实际 {idx}");
    }

    #[test]
    fn downgrade_never_uses_terminal_base_16() {
        // 0..15 由用户终端配色决定，主题不该落到那里去。
        for r in (0..=255u8).step_by(17) {
            for g in (0..=255u8).step_by(17) {
                for b in (0..=255u8).step_by(17) {
                    let idx = Rgb(r, g, b).to_ansi256();
                    assert!(idx >= 16, "#{r:02x}{g:02x}{b:02x} 落到了基础色 {idx}");
                }
            }
        }
    }

    #[test]
    fn downgrade_is_close_enough() {
        // 近似应确实「近」：与所选格的色差不该离谱。
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        for r in (0..=255u8).step_by(23) {
            for g in (0..=255u8).step_by(23) {
                for b in (0..=255u8).step_by(23) {
                    let want = Rgb(r, g, b);
                    let idx = want.to_ansi256();
                    let got = if idx >= 232 {
                        let v = 8 + 10 * (idx - 232);
                        Rgb(v, v, v)
                    } else {
                        let i = idx - 16;
                        Rgb(
                            LEVELS[(i / 36) as usize],
                            LEVELS[((i % 36) / 6) as usize],
                            LEVELS[(i % 6) as usize],
                        )
                    };
                    // 每通道最大档距 95（0→95），欧氏距离平方上限放宽到 3*60^2。
                    assert!(
                        want.dist2(got) <= 3 * 60 * 60,
                        "#{r:02x}{g:02x}{b:02x} → {idx} 偏差过大"
                    );
                }
            }
        }
    }

    // ---- 主题解析 ----

    #[test]
    fn all_builtin_themes_parse() {
        for name in builtin_names() {
            let text = builtin(name).unwrap_or_else(|| panic!("{name} 应有内置定义"));
            let spec: ThemeSpec =
                toml::from_str(text).unwrap_or_else(|e| panic!("{name} 解析失败：{e}"));
            assert!(!spec.name.is_empty(), "{name} 应有 name 字段");
            // 关键槽位必须齐全，否则主题会半截回落到别的配色。
            assert!(spec.fg.is_some(), "{name} 缺 fg");
            assert!(spec.accent.is_some(), "{name} 缺 accent");
            assert!(spec.error.is_some(), "{name} 缺 error");
        }
    }

    #[test]
    fn unknown_theme_falls_back_without_panic() {
        let t = Theme::load_builtin("查无此主题", ColorDepth::TrueColor);
        let sepia = Theme::load_builtin("sepia", ColorDepth::TrueColor);
        assert_eq!(t, sepia, "未知主题应回落 sepia");
    }

    #[test]
    fn partial_user_theme_inherits_the_rest() {
        // 用户只改一个颜色，其余应继承回落主题而不是变成默认色块。
        let base = Theme::load_builtin("dark", ColorDepth::TrueColor);
        let t = Theme::from_toml(
            "name = \"我的\"\naccent = \"#ff0000\"\n",
            ColorDepth::TrueColor,
            "dark",
        );
        assert_eq!(t.accent, Color::Rgb(255, 0, 0), "改了的生效");
        assert_eq!(t.fg, base.fg, "没改的继承");
        assert_eq!(t.error, base.error);
    }

    #[test]
    fn invalid_color_falls_back_not_panics() {
        let base = Theme::load_builtin("dark", ColorDepth::TrueColor);
        let t = Theme::from_toml(
            "name = \"坏的\"\naccent = \"不是颜色\"\n",
            ColorDepth::TrueColor,
            "dark",
        );
        assert_eq!(t.accent, base.accent, "非法颜色回落，不 panic");
    }

    #[test]
    fn malformed_toml_falls_back_to_builtin() {
        let base = Theme::load_builtin("dark", ColorDepth::TrueColor);
        let t = Theme::from_toml("这不是 toml [[[", ColorDepth::TrueColor, "dark");
        assert_eq!(t, base);
    }

    #[test]
    fn theme_downgrades_under_256() {
        let t = Theme::load_builtin("sepia", ColorDepth::Ansi256);
        // 256 色下不该出现 Rgb 变体。
        assert!(
            !matches!(t.fg, Color::Rgb(..)),
            "256 色终端不该下发 RGB：{:?}",
            t.fg
        );
        assert!(!matches!(t.accent, Color::Rgb(..)));
    }
}
