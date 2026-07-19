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
    /// 排版规则开关（§6.5 规则表、§8 `[format]`）。
    ///
    /// 直接复用 mj-text 的 `FormatOptions`：规则表就是它的字段，
    /// 再包一层只会让「配置里写的」与「排版真正用的」有机会走偏。
    #[serde(default)]
    pub format: mj_text::format::FormatOptions,
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub appearance: Appearance,
    /// 输入设备（§13：鼠标可选支持）。
    #[serde(default)]
    pub input: Input,
    /// 校对规则开关与阈值（§6.8、§8 `[proof]`）。
    #[serde(default)]
    pub proof: Proof,
    /// 键位重绑定（§7.3 `[MUST]`）：命令 id → 键位串，如 `proof = "F6"`。
    ///
    /// 存原始 table 而非强类型：键位的解析与冲突检测要用 ratatui 的按键类型，
    /// 那属 mj-tui（§4 分层，mj-core 不依赖 ratatui）。
    #[serde(default)]
    pub keymap: toml::Table,

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
    /// `thinned`（默认）| `fifo`。见 §6.9。
    ///
    /// 用枚举而非 String：M0 时类型还没有，先占了个 String；现在有了就该换过来——
    /// 否则配置里写错一个字母会被悄悄当成默认值，用户以为设上了。
    pub retention: crate::history::Retention,
    pub auto_interval_min: u64,
    pub auto_min_words: usize,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for History {
    fn default() -> Self {
        Self {
            max_per_chapter: 40,
            retention: crate::history::Retention::Thinned,
            auto_interval_min: 10,
            auto_min_words: 300,
            extra: toml::Table::new(),
        }
    }
}

/// 校对配置（§6.8）。默认对应「本地规则默认开、文风前两条开、的地得折叠」。
///
/// 落到 mj-text 的 `ProofOptions`/`StyleParams` 由 `to_options` 完成——
/// 配置里写的与校对真正用的是同一套值，不再包一层就走不偏。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Proof {
    pub confusion: bool,
    pub punct: bool,
    pub consistency: bool,
    pub de_di_de: bool,
    pub comma_chain: bool,
    pub comma_chain_max: usize,
    pub long_sentence: bool,
    pub long_sentence_max: usize,
    pub word_repeat: bool,
    pub short_burst: bool,
    /// 低于此置信度的问题 UI 默认折叠（§12.3：的地得 <0.6 折叠）。
    pub fold_below: f32,
    /// 外部校对命令（§6.8 的 ExternalProofreader，默认关）。
    #[serde(default)]
    pub external: ExternalProof,
    /// 大模型校对后端（§6.8 的 LlmProofreader，默认关）。
    #[serde(default)]
    pub llm: LlmProof,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// 外部校对后端配置（§6.8）。
///
/// 默认关且命令为空——这东西会**在用户机器上起进程**，不能因为装了个软件
/// 就默认去跑点什么。必须由用户显式写出要跑的命令。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalProof {
    pub enabled: bool,
    /// 要跑的命令，如 `["python", "-m", "pycorrector_server"]`。
    pub command: Vec<String>,
    /// 超时毫秒数（§6.8 默认 30s）。
    pub timeout_ms: u64,
    /// 送给外部程序的语言标记。
    pub lang: String,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for ExternalProof {
    fn default() -> Self {
        Self {
            enabled: false,
            command: Vec::new(),
            timeout_ms: 30_000,
            lang: "zh".into(),
            extra: toml::Table::new(),
        }
    }
}

/// 大模型校对后端配置（§6.8）。
///
/// # 两条 `[MUST]` 直接长在类型上
///
/// 1. **key 只存环境变量名**。这里根本没有放 key 的字段：想写也写不进来。
///    真有人写了 `api_key = "sk-..."`，它会被 `extra` 兜住并原样回写（§8 前向兼容），
///    等于把密钥永久留在 config.toml 里——所以 `LlmProofreader` 启动前会**拒绝**
///    这种配置并提示改用环境变量，见 `plaintext_secret_field`。
/// 2. **首次开启要明确同意**。`consented` 默认 false，且 `enabled` 单独一个是不够的：
///    正文要发给第三方，这事必须是用户点过头的，不能靠改一个 bool 顺带发生。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmProof {
    pub enabled: bool,
    /// 用户已看过「正文将发送到第三方服务」的说明并同意（§6.8 `[MUST]`）。
    pub consented: bool,
    pub endpoint: String,
    /// 模型 id。doc.md §8 的示例写的是 `claude-sonnet-4-6`，那一代已经过时；
    /// 默认取当前的 Opus，用户按自己的成本/延迟偏好改。
    pub model: String,
    /// **环境变量名**，不是 key 本身。
    pub api_key_env: String,
    /// 每批段落数上限（§6.8 默认 8）。
    pub batch_paragraphs: usize,
    /// 每批字数上限（§6.8 默认 2000）。
    pub batch_chars: usize,
    pub timeout_ms: u64,
    /// 单次回复的 token 上限。要留足——思考与 JSON 共用这个预算，
    /// 给少了就是「思考占满、JSON 截断」，那一批白花钱。
    pub max_tokens: u32,
    /// `low` | `medium` | `high` | `xhigh` | `max`。**空串 = 不下发**。
    ///
    /// 默认 `medium` 而非 API 默认的 `high`：这是用户按下 F7 后干等的交互路径，
    /// 一批才 8 段，medium 已足够；要更细的病句就往上调。
    /// 用老模型（如 Haiku 4.5）时它不认 effort，置空即可。
    pub effort: String,
    /// 是否开自适应思考。病句判断吃推理，默认开。
    ///
    /// 同样是给老模型留的退路：4.6 之前的模型不认 `thinking: adaptive`，
    /// 发过去直接 400。换那种模型时连同 `effort` 一起关掉。
    pub thinking: bool,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for LlmProof {
    fn default() -> Self {
        Self {
            enabled: false,
            consented: false,
            endpoint: "https://api.anthropic.com/v1/messages".into(),
            model: "claude-opus-4-8".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            batch_paragraphs: 8,
            batch_chars: 2000,
            timeout_ms: 60_000,
            max_tokens: 16_000,
            effort: "medium".into(),
            thinking: true,
            extra: toml::Table::new(),
        }
    }
}

impl LlmProof {
    /// 配置里是否被塞了明文密钥。返回那个字段名。
    ///
    /// §6.8 `[MUST]`「不得明文写进 config.toml」。这里不是提醒而是**闸门**：
    /// 检出就拒跑（见 `proof_llm`），否则 `extra` 的原样回写会让这个 key
    /// 在 config.toml 里长住，用户还以为自己只是试了一下。
    pub fn plaintext_secret_field(&self) -> Option<&str> {
        const BANNED: [&str; 6] = ["api_key", "apikey", "key", "token", "secret", "auth"];
        self.extra
            .keys()
            .map(String::as_str)
            .find(|k| BANNED.contains(&k.to_ascii_lowercase().as_str()))
    }
}

impl Default for Proof {
    fn default() -> Self {
        Self {
            confusion: true,
            punct: true,
            consistency: true,
            de_di_de: true,
            comma_chain: true,
            comma_chain_max: 6,
            long_sentence: true,
            long_sentence_max: 60,
            word_repeat: false,
            short_burst: false,
            fold_below: 0.6,
            external: ExternalProof::default(),
            llm: LlmProof::default(),
            extra: toml::Table::new(),
        }
    }
}

impl Proof {
    /// 映射到 mj-text 的规则选项。
    pub fn to_options(&self) -> mj_text::proof::ProofOptions {
        let mut style = mj_text::proof::style::StyleParams {
            comma_chain_on: self.comma_chain,
            comma_chain_max: self.comma_chain_max,
            long_sentence_on: self.long_sentence,
            long_sentence_max: self.long_sentence_max,
            word_repeat_on: self.word_repeat,
            short_burst_on: self.short_burst,
            ..mj_text::proof::style::StyleParams::default()
        };
        // 阈值为 0 会把「> 0」变成对所有句子都报，几乎必是配置笔误；退回默认。
        if style.comma_chain_max == 0 {
            style.comma_chain_max = 6;
        }
        if style.long_sentence_max == 0 {
            style.long_sentence_max = 60;
        }
        mj_text::proof::ProofOptions {
            confusion_on: self.confusion,
            punct_on: self.punct,
            consistency_on: self.consistency,
            de_di_de_on: self.de_di_de,
            style,
        }
    }
}

/// 输入设备（§13）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// 鼠标支持（§13 `[SHOULD]`「鼠标可选支持（点击树、拖分隔条、滚轮）」）。
    ///
    /// **默认关**，而且这不是保守，是因为开它有代价：一旦程序捕获鼠标，
    /// 终端自己的「拖选一段文字复制」就没了——那是很多人每天都在用的东西。
    /// §13 同时写着「`[MUST]` 所有功能不依赖鼠标」，可见鼠标本就是添头；
    /// 为一个添头默认拿掉用户已有的能力，不划算。想要的人写一行开。
    pub mouse: bool,
    #[serde(flatten)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: String,
    pub column_width: u16,
    pub font_family: String,
    pub font_size: f32,
    /// 段间额外空行（§6.10）。
    pub paragraph_spacing: u16,
    /// 正文左右留白列数（§6.10）。
    pub margin: u16,
    /// 是否显示行号（§6.10）。
    pub line_number: bool,
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
            paragraph_spacing: 0,
            margin: 4,
            line_number: false,
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

    /// 写回配置（原子写）。
    ///
    /// 各层的 `extra`（`#[serde(flatten)]`）会把读进来时不认识的字段一并写回去，
    /// 故用新版本的墨简改一个主题，不会顺手删掉旧版本或未来版本写的配置
    /// （§8 前向兼容：多余字段保留不报错）。
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| Error::ChapterParse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        crate::atomic::write(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// 写回再读出应当一模一样。
    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.appearance.theme = "high_contrast".into();
        c.appearance.margin = 8;
        c.save(&path).unwrap();

        let back = Config::load(&path).unwrap();
        assert_eq!(back.appearance.theme, "high_contrast");
        assert_eq!(back.appearance.margin, 8);
    }

    /// §8 前向兼容：读进来时不认识的字段，写回去不能丢——
    /// 否则用新版墨简改一次主题，就把旧版/未来版写的配置顺手删了。
    #[test]
    fn save_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[appearance]\ntheme = \"sepia\"\nfuture_option = 42\n",
        )
        .unwrap();

        let mut c = Config::load(&path).unwrap();
        c.appearance.theme = "dark".into();
        c.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("future_option"), "未知字段被写没了：{text}");
        assert!(text.contains("dark"));
    }

    #[test]
    fn missing_file_yields_defaults() {
        let c = Config::load(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(c.general.day_starts_at, 4);
        assert_eq!(c.editor.undo_depth, 500);
        assert_eq!(c.history.retention, crate::history::Retention::Thinned);
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
