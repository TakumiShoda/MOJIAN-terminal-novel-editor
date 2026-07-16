//! 配置加载。见 doc.md §8。
//!
//! 两条契约：
//! - 缺失字段用默认值（`#[serde(default)]` 全覆盖）；
//! - 多余字段保留不报错——用 `extra: toml::Table` 兜住未知键，回写时原样带回。
//!   这是前向兼容的关键：老版本读到新版本写的配置，不能把新字段吃掉。

use std::path::Path;

use mj_text::eol::LineEnding;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub editor: Editor,
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub appearance: Appearance,

    /// 未知的顶层表，原样透传（doc.md §8 前向兼容）。
    #[serde(flatten)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// 码字量按凌晨 N 点切日（写作者常见作息，doc.md §6.4）。
    pub day_starts_at: u8,
    pub keymap: String,
    /// 正文写出时的行尾（doc.md §9）。读入永远归一化为 LF，故此项只影响写出。
    ///
    /// 默认 `lf` 而非 `native`：正文要对 git 友好，且同一份稿子在不同平台间
    /// 传递不应产生「整文件都变了」的假 diff。Windows 用户需要 CRLF 时显式设 `native`。
    pub line_ending: LineEnding,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for General {
    fn default() -> Self {
        Self {
            day_starts_at: 4,
            keymap: "modeless".into(),
            line_ending: LineEnding::Lf,
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    pub autosave_idle_ms: u64,
    pub autosave_words: usize,
    pub undo_depth: usize,
    pub auto_pair: bool,
    pub word_nav: String,
    pub focus_column_width: u16,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            autosave_idle_ms: 3000,
            autosave_words: 200,
            undo_depth: 500,
            auto_pair: true,
            word_nav: "jieba".into(),
            focus_column_width: 40,
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub max_per_chapter: usize,
    pub retention: String,
    pub auto_interval_min: u64,
    pub auto_min_words: usize,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for History {
    fn default() -> Self {
        Self {
            max_per_chapter: 40,
            retention: "thinned".into(),
            auto_interval_min: 10,
            auto_min_words: 300,
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: String,
    pub column_width: u16,
    pub font_family: String,
    pub font_size: f32,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "sepia".into(),
            column_width: 40,
            font_family: "Source Han Serif".into(),
            font_size: 14.0,
            extra: toml::Table::new(),
        }
    }
}

impl Config {
    /// 读取配置。文件不存在 → 全默认值（首次启动的正常路径，不是错误）。
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(Error::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.to_owned(),
            source: Box::new(source),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let c = Config::load(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(c.general.day_starts_at, 4);
        assert_eq!(c.editor.undo_depth, 500);
        assert_eq!(c.history.retention, "thinned");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[editor]\nundo_depth = 99\n").unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.editor.undo_depth, 99, "显式值应生效");
        assert_eq!(c.editor.autosave_words, 200, "未写的字段应取默认值");
        assert_eq!(c.general.keymap, "modeless", "未写的表应取默认值");
    }

    /// doc.md §8：多余字段保留不报错。老版本读到新版本的配置，不能把新字段吃掉。
    #[test]
    fn preserves_unknown_fields_on_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "[general]\nday_starts_at = 5\nfuture_knob = \"x\"\n\n[future_section]\nk = 1\n",
        )
        .unwrap();

        let c = Config::load(&p).unwrap();
        assert_eq!(c.general.day_starts_at, 5);

        let back = toml::to_string(&c).unwrap();
        assert!(back.contains("future_knob"), "未知字段被吃掉了:\n{back}");
        assert!(back.contains("future_section"), "未知表被吃掉了:\n{back}");
    }

    /// doc.md §9：`line_ending = "lf" | "native"`。默认 lf。
    #[test]
    fn parses_line_ending_knob() {
        let dir = tempfile::tempdir().unwrap();

        let default = Config::load(&dir.path().join("missing.toml")).unwrap();
        assert_eq!(default.general.line_ending, LineEnding::Lf, "默认应为 lf");

        for (text, want) in [
            ("[general]\nline_ending = \"lf\"\n", LineEnding::Lf),
            ("[general]\nline_ending = \"native\"\n", LineEnding::Native),
        ] {
            let p = dir.path().join("c.toml");
            std::fs::write(&p, text).unwrap();
            assert_eq!(
                Config::load(&p).unwrap().general.line_ending,
                want,
                "解析 {text:?}"
            );
        }
    }

    /// 非法取值必须报错，不能静默退回默认——用户会以为设置生效了。
    #[test]
    fn rejects_invalid_line_ending() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "[general]\nline_ending = \"crlf\"\n").unwrap();
        assert!(
            matches!(Config::load(&p), Err(Error::ConfigParse { .. })),
            "非法的 line_ending 应报错"
        );
    }

    #[test]
    fn malformed_file_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "this is not toml {{{").unwrap();
        assert!(matches!(Config::load(&p), Err(Error::ConfigParse { .. })));
    }
}
