//! 外观设置页。见 doc.md §6.10、§2.1。
//!
//! `[MUST]`：字体不可用时显示**灰态 + 一句话原因**（「当前终端（Alacritty）不支持
//! 运行时更改字体」），并给「生成配置片段」——文档特意点了「不要省」，因为这是把
//! 「做不到」变成「帮你做到」的关键。
//!
//! 命名上叫「外观」而不是「字体」（§2.1）：字体只是其中一栏，而且多数终端里
//! 那一栏是灰的；用户真正能调、也真正感知得到的是主题与版面。
//!
//! 状态与渲染分离：这里只管「有哪些项、选中第几项、主题切到第几个」，绘制在 app.rs。

use crate::font::{FontCap, TerminalKind, config_snippet};

/// 设置页里的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// 主题，可当场切换（←/→）。
    Theme,
    ColumnWidth,
    Margin,
    ParagraphSpacing,
    LineNumber,
    /// 字体族——多数终端下是灰的。
    FontFamily,
    FontSize,
    /// 配置片段，`y` 复制。
    Snippet,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "主题",
            Self::ColumnWidth => "正文栏宽",
            Self::Margin => "左右留白",
            Self::ParagraphSpacing => "段间空行",
            Self::LineNumber => "行号",
            Self::FontFamily => "字体",
            Self::FontSize => "字号",
            Self::Snippet => "配置片段",
        }
    }
}

pub struct Settings {
    rows: Vec<Row>,
    cursor: usize,
    /// 可选主题：内置 + 用户自建，去重后排好。
    themes: Vec<String>,
    theme_idx: usize,
    /// 探测到的终端与字体能力。
    kind: TerminalKind,
    caps: FontCap,
    snippet: Option<String>,
    /// 版面数值，只读展示（改它们去 config.toml）。
    pub column_width: u16,
    pub margin: u16,
    pub paragraph_spacing: u16,
    pub line_number: bool,
    pub font_family: String,
    pub font_size: f32,
    /// 主题是否被改过——关页面时据此决定要不要写盘。
    dirty: bool,
}

impl Settings {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        themes: Vec<String>,
        current_theme: &str,
        kind: TerminalKind,
        column_width: u16,
        margin: u16,
        paragraph_spacing: u16,
        line_number: bool,
        font_family: String,
        font_size: f32,
    ) -> Self {
        let caps = kind.expected_caps();
        let snippet = config_snippet(kind, &font_family, font_size);
        let theme_idx = themes.iter().position(|t| t == current_theme).unwrap_or(0);
        let mut rows = vec![
            Row::Theme,
            Row::ColumnWidth,
            Row::Margin,
            Row::ParagraphSpacing,
            Row::LineNumber,
            Row::FontFamily,
            Row::FontSize,
        ];
        if snippet.is_some() {
            rows.push(Row::Snippet);
        }
        Self {
            rows,
            cursor: 0,
            themes,
            theme_idx,
            kind,
            caps,
            snippet,
            column_width,
            margin,
            paragraph_spacing,
            line_number,
            font_family,
            font_size,
            dirty: false,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn current_row(&self) -> Row {
        self.rows[self.cursor.min(self.rows.len() - 1)]
    }

    pub fn theme(&self) -> &str {
        self.themes.get(self.theme_idx).map_or("sepia", |s| s)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn snippet(&self) -> Option<&str> {
        self.snippet.as_deref()
    }

    pub fn terminal(&self) -> TerminalKind {
        self.kind
    }

    pub fn move_down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// ←/→ 换主题。返回 true 表示主题变了，调用方要立刻重算配色（当场看到效果）。
    pub fn cycle_theme(&mut self, forward: bool) -> bool {
        if self.current_row() != Row::Theme || self.themes.is_empty() {
            return false;
        }
        let n = self.themes.len();
        self.theme_idx = if forward {
            (self.theme_idx + 1) % n
        } else {
            (self.theme_idx + n - 1) % n
        };
        self.dirty = true;
        true
    }

    /// 某一行是否处于灰态（不可用）。
    pub fn is_disabled(&self, row: Row) -> bool {
        match row {
            Row::FontFamily => !self.caps.contains(FontCap::SET_FAMILY),
            Row::FontSize => !self.caps.contains(FontCap::SET_SIZE),
            // 版面数值这里只读——不是「不可用」，故不置灰。
            _ => false,
        }
    }

    /// 该行的取值文本。
    pub fn value_of(&self, row: Row) -> String {
        match row {
            Row::Theme => format!("‹ {} ›", self.theme()),
            Row::ColumnWidth => {
                if self.column_width == 0 {
                    "撑满".into()
                } else {
                    format!("{} 全角字", self.column_width)
                }
            }
            Row::Margin => format!("{} 列", self.margin),
            Row::ParagraphSpacing => format!("{} 行", self.paragraph_spacing),
            Row::LineNumber => if self.line_number { "显示" } else { "隐藏" }.into(),
            Row::FontFamily => self.font_family.clone(),
            Row::FontSize => format!("{}", self.font_size),
            Row::Snippet => "按 y 复制".into(),
        }
    }

    /// 该行的补充说明。灰态行必须给出**一句话原因**（§6.10 [MUST]）。
    pub fn note_of(&self, row: Row) -> Option<String> {
        match row {
            Row::FontFamily | Row::FontSize if self.is_disabled(row) => {
                Some(self.kind.unsupported_reason())
            }
            Row::Snippet => self
                .kind
                .config_file_hint()
                .map(|f| format!("贴进 {f}，重启终端后生效")),
            Row::ColumnWidth | Row::Margin | Row::ParagraphSpacing | Row::LineNumber => {
                Some("在 config.toml 的 [appearance] 里改".into())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn settings(kind: TerminalKind) -> Settings {
        Settings::new(
            vec!["dark".into(), "light".into(), "sepia".into()],
            "sepia",
            kind,
            40,
            4,
            0,
            false,
            "Source Han Serif".into(),
            14.0,
        )
    }

    #[test]
    fn starts_on_the_current_theme() {
        let s = settings(TerminalKind::Alacritty);
        assert_eq!(s.theme(), "sepia");
        assert!(!s.is_dirty());
    }

    #[test]
    fn cycles_theme_both_ways() {
        let mut s = settings(TerminalKind::Alacritty);
        assert!(s.cycle_theme(true));
        assert_eq!(s.theme(), "dark", "sepia 之后绕回开头");
        assert!(s.cycle_theme(false));
        assert_eq!(s.theme(), "sepia");
        assert!(s.is_dirty(), "改过就该标脏，关页面时写盘");
    }

    /// 光标不在主题行时，←/→ 不该动主题。
    #[test]
    fn cycle_only_works_on_theme_row() {
        let mut s = settings(TerminalKind::Alacritty);
        s.move_down(); // 栏宽
        assert_eq!(s.current_row(), Row::ColumnWidth);
        assert!(!s.cycle_theme(true));
        assert_eq!(s.theme(), "sepia", "主题不该被改");
    }

    /// §6.10 [MUST]：字体不可用时灰态 + 一句话原因，且原因要点出终端名。
    #[test]
    fn font_rows_disabled_with_reason_when_unsupported() {
        let s = settings(TerminalKind::Alacritty);
        assert!(s.is_disabled(Row::FontFamily));
        assert!(s.is_disabled(Row::FontSize));
        let note = s.note_of(Row::FontFamily).unwrap();
        assert!(note.contains("Alacritty"), "原因要点出是哪个终端：{note}");
    }

    /// kitty 能改字号、改不了字体族——两栏应分别灰/不灰。
    #[test]
    fn kitty_disables_family_but_not_size() {
        let s = settings(TerminalKind::Kitty);
        assert!(s.is_disabled(Row::FontFamily), "kitty 改不了字体族");
        assert!(!s.is_disabled(Row::FontSize), "kitty 能改字号");
    }

    /// 不可用时要有「生成配置片段」那一行；能改的终端不需要。
    #[test]
    fn snippet_row_appears_only_when_needed() {
        let s = settings(TerminalKind::Alacritty);
        assert!(s.rows().contains(&Row::Snippet));
        assert!(s.snippet().unwrap().contains("alacritty.toml"));
        let note = s.note_of(Row::Snippet).unwrap();
        assert!(note.contains("重启终端"), "{note}");
    }

    #[test]
    fn no_snippet_row_when_format_unknown() {
        let s = settings(TerminalKind::Unknown);
        assert!(
            !s.rows().contains(&Row::Snippet),
            "说不准格式就不给片段，也就不该有这一行"
        );
    }

    #[test]
    fn navigation_clamps() {
        let mut s = settings(TerminalKind::Alacritty);
        s.move_up();
        assert_eq!(s.cursor(), 0);
        for _ in 0..50 {
            s.move_down();
        }
        assert_eq!(s.cursor(), s.rows().len() - 1);
    }

    #[test]
    fn column_width_zero_reads_as_full_width() {
        let mut s = settings(TerminalKind::Alacritty);
        s.column_width = 0;
        assert_eq!(s.value_of(Row::ColumnWidth), "撑满");
    }

    /// 只读的版面项要告诉用户去哪儿改，别让人对着它按半天。
    #[test]
    fn readonly_rows_say_where_to_edit() {
        let s = settings(TerminalKind::Alacritty);
        let note = s.note_of(Row::Margin).unwrap();
        assert!(note.contains("config.toml"), "{note}");
    }
}
