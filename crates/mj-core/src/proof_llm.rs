//! 大模型校对后端（`LlmProofreader`）。见 doc.md §6.8 第 3 条。
//!
//! 本地规则查错别字和标点，外部命令查它会查的；病句主要靠这里。
//! 默认关，开之前用户得点过头——正文要发到第三方去。
//!
//! # 为什么不要模型给偏移量
//!
//! §6.8 给外部命令定的契约是 `{"para":0,"start":12,"end":14}`。对**程序**来说
//! 这没问题，对**模型**来说这是最不该问的东西：数汉字是它最不擅长的一件事，
//! 偏一个字就把刀切在词中间，而 §0 明写切错就是毁稿。
//!
//! 所以这里的契约要的是 `quote`——把它认为有问题的原文**一字不差地抄回来**，
//! 由我们 `find` 回去定位。抄错了就找不到，找不到就丢掉并记日志。
//! 这样模型的失败模式从「悄悄给错坐标」变成「这条不算数」，前者毁稿，后者只是漏报。
//!
//! # 请求形状上两个容易凭记忆写错的地方
//!
//! - **不能用 assistant 预填**。「预填一个 `{` 逼模型只吐 JSON」是老办法，
//!   在 Opus 4.6 及以后的模型上直接 400。现在用 `output_config.format`
//!   下发 JSON Schema，由服务端保证形状。
//! - **`content` 是块数组，不是一段文本**。开着思考时 `content[0]` 是 thinking 块，
//!   照着 `content[0].text` 取会拿到空。必须挑出 `type == "text"` 的块。
//!
//! # 一条铁律：绝不影响编辑
//!
//! 同 `proof_external`：本模块**不返回 Err**。网络断了、key 错了、模型抽风，
//! 一律退化成「没有额外的问题 + 一句提示」。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mj_text::proof::{CancelToken, Category, Issue, Paragraph, ProofContext, Severity, Source};
use serde::{Deserialize, Serialize};

use crate::config::LlmProof;
use crate::proof_external::Outcome;

/// prompt 版本。改了 prompt 或 schema 就得加一，否则缓存会拿旧 prompt 的结果
/// 冒充新 prompt 的（§6.8：缓存键含 prompt 版本）。
const PROMPT_VERSION: u32 = 1;

/// Anthropic Messages API 的版本头，与模型无关。
const API_VERSION: &str = "2023-06-01";

/// 缓存条目上限。超了就只留本轮用到的段落——它是缓存，扔了只是下次多花几次调用。
const CACHE_MAX_ENTRIES: usize = 4096;

const SYSTEM_PROMPT: &str = "\
你是中文小说的校对助手。你要找的是**读者会绊一下的地方**：病句、成分残缺、\
搭配不当、指代不明、逻辑不通、时态或人称前后矛盾、明显的错别字。

不要做的事：
- 不要点评文风、节奏、用词雅俗，也不要提「可以更生动」这类建议。作者的文风是作者的。
- 不要动人物名、地名、功法名、自造词。给你的专名表里的词一律当作正确。
- 拿不准的不报。漏一条没关系，误报一条会让作者关掉这个功能。

对每一处问题，`quote` 必须是该段原文里**一字不差**的一小段（连标点一起抄），\
且尽量短——只框住出问题的地方，不要整段抄。抄不准的那条会被丢弃。
`suggestion` 是替换 `quote` 的改写；给不出就留空字符串。";

// ---------- 与模型之间的 JSON 契约 ----------

#[derive(Debug, Deserialize)]
struct ModelOutput {
    #[serde(default)]
    issues: Vec<ModelIssue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ModelIssue {
    /// 批内段号（就是 prompt 里 `[n]` 的 n）。
    #[serde(default)]
    para: usize,
    /// 原文片段，必须能在该段里原样找到。
    #[serde(default)]
    quote: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    suggestion: String,
    #[serde(default = "half")]
    confidence: f32,
}

fn half() -> f32 {
    0.5
}

/// 下发给服务端的 JSON Schema。
///
/// 三条硬要求：每个 object 都要 `additionalProperties: false`、`required` 要列全
/// 所有字段、不能用 `minimum`/`maxLength` 这类约束（不支持）。`confidence` 因此
/// 没有区间约束，由我们自己 clamp。
fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "issues": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "para": {"type": "integer", "description": "段号，即输入里 [n] 的 n"},
                        "quote": {"type": "string", "description": "该段原文里一字不差的片段，尽量短"},
                        "category": {
                            "type": "string",
                            "enum": ["Typo", "Grammar", "Punct", "Style", "Consistency"]
                        },
                        "message": {"type": "string", "description": "一句话说清问题，中文"},
                        "suggestion": {"type": "string", "description": "替换 quote 的改写；给不出就空串"},
                        "confidence": {"type": "number", "description": "0 到 1"}
                    },
                    "required": ["para", "quote", "category", "message", "suggestion", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["issues"],
        "additionalProperties": false
    })
}

// ---------- 后端 ----------

pub struct LlmProofreader {
    cfg: LlmProof,
    cache_path: Option<PathBuf>,
}

impl LlmProofreader {
    pub fn new(cfg: LlmProof) -> Self {
        Self {
            cfg,
            cache_path: None,
        }
    }

    /// 指定段落级缓存的落盘位置（`Workspace::llm_cache_file`）。
    pub fn with_cache(mut self, path: PathBuf) -> Self {
        self.cache_path = Some(path);
        self
    }

    /// 配置齐了、用户同意了、且没把密钥明文写进配置，才算开着。
    pub fn is_enabled(&self) -> bool {
        self.cfg.enabled
            && self.cfg.consented
            && !self.cfg.endpoint.is_empty()
            && !self.cfg.model.is_empty()
            && !self.cfg.api_key_env.is_empty()
            && self.cfg.plaintext_secret_field().is_none()
    }

    /// 开着但还差点什么时，说清差的是什么。开关本身是关的就返回 None——
    /// 没开的功能不该在用户眼前唠叨。
    pub fn setup_problem(&self) -> Option<String> {
        if !self.cfg.enabled {
            return None;
        }
        if let Some(field) = self.cfg.plaintext_secret_field() {
            return Some(format!(
                "config.toml 的 [proof.llm] 里有明文密钥字段 `{field}`。\
                 请删掉它，把密钥放进环境变量 {}（§6.8 不允许密钥明文入配置）",
                self.cfg.api_key_env
            ));
        }
        if !self.cfg.consented {
            return Some("尚未确认「正文将发送到第三方服务」。请在设置页开启以确认".into());
        }
        if self.cfg.api_key_env.is_empty() || self.cfg.endpoint.is_empty() {
            return Some("[proof.llm] 缺 endpoint 或 api_key_env".into());
        }
        if std::env::var(&self.cfg.api_key_env).is_err() {
            return Some(format!("环境变量 {} 没有设置", self.cfg.api_key_env));
        }
        None
    }

    /// 校对给定段落。**手动触发专用**——§6.8 `[MUST]` 不做全书自动扫描。
    ///
    /// 按 §6.8 的策略：分批、串行、退避重试、段落级缓存。任何一批失败都只丢那一批。
    pub fn check(
        &self,
        paragraphs: &[Paragraph<'_>],
        ctx: &ProofContext,
        cancel: &CancelToken,
    ) -> Outcome {
        if !self.is_enabled() {
            return match self.setup_problem() {
                Some(p) => Outcome::warn(p),
                None => Outcome::default(),
            };
        }
        let key = match std::env::var(&self.cfg.api_key_env) {
            Ok(k) if !k.trim().is_empty() => k,
            _ => {
                return Outcome::warn(format!("环境变量 {} 没有设置或为空", self.cfg.api_key_env));
            }
        };
        self.check_with_key(&key, paragraphs, ctx, cancel)
    }

    /// `check` 的实体。密钥从参数进而非现读环境变量——测试要能在不碰
    /// 进程环境的前提下把整条 HTTP 路径跑通（改 env 在并行测试里是数据竞争）。
    fn check_with_key(
        &self,
        key: &str,
        paragraphs: &[Paragraph<'_>],
        ctx: &ProofContext,
        cancel: &CancelToken,
    ) -> Outcome {
        let mut cache = Cache::load(self.cache_path.as_deref());
        let mut found: Vec<Issue> = Vec::new();
        let mut failures = 0usize;
        let mut last_error: Option<String> = None;
        // 命中缓存的段落不再送出去（§6.8：未改动的段落不重复请求）。
        let mut pending: Vec<usize> = Vec::new();
        for (i, p) in paragraphs.iter().enumerate() {
            match cache.get(&self.cache_key(p.text)) {
                Some(hits) => found.extend(locate(hits, p, 0)),
                None => pending.push(i),
            }
        }

        for batch in self.batches(paragraphs, &pending) {
            if cancel.is_cancelled() {
                break;
            }
            let slice: Vec<&Paragraph<'_>> = batch.iter().map(|&i| &paragraphs[i]).collect();
            match self.run_batch(key, &slice, ctx) {
                Ok(issues) => {
                    // 按批内段号归位，顺手写缓存。
                    for (local, &global) in batch.iter().enumerate() {
                        let p = &paragraphs[global];
                        let mine: Vec<ModelIssue> =
                            issues.iter().filter(|m| m.para == local).cloned().collect();
                        found.extend(locate(&mine, p, 0));
                        cache.put(self.cache_key(p.text), mine);
                    }
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!(error = %e, "模型校对：这一批丢弃");
                    last_error = Some(e);
                }
            }
        }

        cache.save(self.cache_path.as_deref(), paragraphs, |t| {
            self.cache_key(t)
        });
        found.sort_by_key(|i| i.range.start);

        let warning = last_error.map(|e| {
            if failures > 1 {
                format!("{failures} 批未完成（{e}）")
            } else {
                e
            }
        });
        Outcome {
            issues: found,
            warning,
        }
    }

    /// 缓存键：`blake3(prompt版本 + 模型 + 段落)`（§6.8）。
    ///
    /// 模型和 prompt 版本都得进去——换了模型或改了 prompt，旧结果就不作数了。
    fn cache_key(&self, text: &str) -> String {
        let material = format!("{PROMPT_VERSION}\0{}\0{text}", self.cfg.model);
        // 取前 16 字节手写成 hex，而不是切 to_hex() 的字符串——后者要靠
        // 「blake3 的 hex 全是 ASCII」这个额外前提才安全（同 `ignore_key`）。
        let h = blake3::hash(material.as_bytes());
        let mut s = String::with_capacity(32);
        for b in &h.as_bytes()[..16] {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// 按 §6.8 的「8 段/批 或 2000 字上限」切批。`pending` 是待查段落的下标。
    fn batches(&self, paragraphs: &[Paragraph<'_>], pending: &[usize]) -> Vec<Vec<usize>> {
        let max_paras = self.cfg.batch_paragraphs.max(1);
        let max_chars = self.cfg.batch_chars.max(200);
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        let mut chars = 0usize;
        for &i in pending {
            let n = paragraphs[i].text.chars().count();
            // 单段就超上限时不切它——切开会让模型看不全句子。自成一批送出去。
            if !cur.is_empty() && (cur.len() >= max_paras || chars + n > max_chars) {
                out.push(std::mem::take(&mut cur));
                chars = 0;
            }
            cur.push(i);
            chars += n;
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// 送一批，带退避重试。返回批内相对的结果。
    fn run_batch(
        &self,
        key: &str,
        batch: &[&Paragraph<'_>],
        ctx: &ProofContext,
    ) -> Result<Vec<ModelIssue>, String> {
        let body = self.request_body(batch, ctx);
        let payload = serde_json::to_vec(&body).map_err(|e| format!("请求序列化失败：{e}"))?;

        // §6.8：串行 + 退避重试；解析失败重试一次后丢弃该批。
        let mut wait = std::time::Duration::from_millis(500);
        let mut last = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(wait);
                wait *= 3;
            }
            match self.post(key, &payload) {
                Ok(text) => match parse_output(&text) {
                    Ok(issues) => return Ok(issues),
                    Err(e) => last = e,
                },
                // 配置/鉴权类错误重试也没用，直接抬走。
                Err(Fail::Fatal(e)) => return Err(e),
                Err(Fail::Retryable(e)) => last = e,
            }
        }
        Err(last)
    }

    fn request_body(&self, batch: &[&Paragraph<'_>], ctx: &ProofContext) -> serde_json::Value {
        let mut output_config = serde_json::Map::new();
        output_config.insert(
            "format".into(),
            serde_json::json!({
                "type": "json_schema",
                "schema": output_schema(),
            }),
        );
        // 空字符串 = 不下发。老模型（如 Haiku 4.5）不认 effort，留个退路。
        if !self.cfg.effort.is_empty() {
            output_config.insert("effort".into(), self.cfg.effort.clone().into());
        }

        let mut body = serde_json::json!({
            "model": self.cfg.model,
            "max_tokens": self.cfg.max_tokens,
            "system": SYSTEM_PROMPT,
            "output_config": output_config,
            "messages": [{"role": "user", "content": user_prompt(batch, ctx)}],
        });
        if self.cfg.thinking {
            // 4.6 之后只有 adaptive 这一种开法；budget_tokens 已被移除，发了就 400。
            body["thinking"] = serde_json::json!({"type": "adaptive"});
        }
        body
    }

    fn post(&self, key: &str, payload: &[u8]) -> Result<String, Fail> {
        let resp = ureq::post(&self.cfg.endpoint)
            .set("x-api-key", key)
            .set("anthropic-version", API_VERSION)
            .set("content-type", "application/json")
            .timeout(std::time::Duration::from_millis(
                self.cfg.timeout_ms.max(1000),
            ))
            .send_bytes(payload);

        match resp {
            Ok(r) => r
                .into_string()
                .map_err(|e| Fail::Retryable(format!("读响应失败：{e}"))),
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                let detail = api_error_message(&body);
                match code {
                    // 429/5xx 是暂时的，值得退避重试。
                    429 | 500..=599 => Err(Fail::Retryable(format!("HTTP {code}：{detail}"))),
                    401 | 403 => Err(Fail::Fatal(format!(
                        "HTTP {code}：环境变量 {} 里的密钥被拒（{detail}）",
                        self.cfg.api_key_env
                    ))),
                    // 400 多半是配置对不上模型（如老模型不认 thinking/effort）。
                    // 把服务端自己的话原样带上——它比我猜的准。
                    _ => Err(Fail::Fatal(format!("HTTP {code}：{detail}"))),
                }
            }
            Err(e) => Err(Fail::Retryable(format!("连不上：{e}"))),
        }
    }
}

enum Fail {
    /// 重试可能好转（限流、5xx、网络）。
    Retryable(String),
    /// 重试也一样（鉴权、请求形状不对）。
    Fatal(String),
}

/// 拼给模型看的正文。段落编号就是它要回填的 `para`。
fn user_prompt(batch: &[&Paragraph<'_>], ctx: &ProofContext) -> String {
    let mut s = String::new();
    if !ctx.names.is_empty() {
        // 专名表挡住绝大多数误报：不给的话「沈砚」「玄铁令」全会被当错别字。
        let names: Vec<&str> = ctx.names.iter().take(200).map(String::as_str).collect();
        s.push_str("专名表（一律视为正确，不要报错）：");
        s.push_str(&names.join("、"));
        s.push_str("\n\n");
    }
    s.push_str("以下是待校对的段落，逐段编号：\n\n");
    for (i, p) in batch.iter().enumerate() {
        s.push_str(&format!("[{i}] {}\n\n", p.text));
    }
    s
}

/// 从响应里取出模型给的 JSON。
///
/// `content` 是**块数组**：开着思考时第一块是 thinking，所以只挑 `type == "text"` 的。
fn parse_output(raw: &str) -> Result<Vec<ModelIssue>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("响应不是合法 JSON：{e}"))?;

    match v.get("stop_reason").and_then(|s| s.as_str()) {
        // 服务端拒答：重试同样的正文也是拒，当作这批没有结果。
        Some("refusal") => return Err("模型拒绝处理这一批".into()),
        // 截断的 JSON 解出来只会是垃圾。多半是 max_tokens 给少了。
        Some("max_tokens") => {
            return Err("回复被 max_tokens 截断，请调大 [proof.llm] max_tokens".into());
        }
        _ => {}
    }

    let text: String = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(format!("响应里没有文本块：{}", head(raw, 80)));
    }
    let out: ModelOutput = serde_json::from_str(&text)
        .map_err(|e| format!("模型没按 schema 返回（{e}）：{}", head(&text, 80)))?;
    Ok(out.issues)
}

/// 服务端错误体里的 `error.message`；取不到就退回原文头部。
fn api_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")?
                .get("message")?
                .as_str()
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| head(body, 120))
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 把模型抄回来的 `quote` 在段落里找回去，换成整章坐标。
///
/// 找不到就丢——见模块注释。`base` 是段落 offset 之外的额外位移（当前恒为 0，
/// 留给将来按句切分用）。
fn locate(hits: &[ModelIssue], p: &Paragraph<'_>, base: usize) -> Vec<Issue> {
    let mut out = Vec::new();
    for m in hits {
        if m.quote.is_empty() {
            continue;
        }
        let Some(at) = p.text.find(&m.quote) else {
            // 最常见的失败：模型「顺手改了个字」再抄回来。宁可漏报也不硬定位。
            tracing::warn!(quote = %m.quote, "模型校对：quote 不在原文里，丢弃该条");
            continue;
        };
        let start = p.offset + base + at;
        out.push(Issue {
            range: start..start + m.quote.len(),
            severity: Severity::Warning,
            category: parse_category(&m.category),
            rule_id: "llm.review".into(),
            message: if m.message.is_empty() {
                "模型指出此处可能有问题".into()
            } else {
                m.message.clone()
            },
            suggestions: if m.suggestion.is_empty() {
                Vec::new()
            } else {
                vec![m.suggestion.clone()]
            },
            // §6.8 [MUST]：UI 必须区分「本地规则」与「模型建议」。
            source: Source::Llm,
            confidence: m.confidence.clamp(0.0, 1.0),
        });
    }
    out
}

fn parse_category(s: &str) -> Category {
    match s.to_ascii_lowercase().as_str() {
        "typo" => Category::Typo,
        "punct" => Category::Punct,
        "style" => Category::Style,
        "consistency" => Category::Consistency,
        // 认不出就归病句——这个后端本来就是来查病句的。
        _ => Category::Grammar,
    }
}

// ---------- 段落级缓存 ----------

#[derive(Default)]
struct Cache {
    map: HashMap<String, Vec<ModelIssue>>,
    dirty: bool,
}

impl Cache {
    fn load(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(map) => Self { map, dirty: false },
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "模型校对缓存损坏，视作空");
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "读模型校对缓存失败");
                Self::default()
            }
        }
    }

    fn get(&self, key: &str) -> Option<&Vec<ModelIssue>> {
        self.map.get(key)
    }

    fn put(&mut self, key: String, hits: Vec<ModelIssue>) {
        self.map.insert(key, hits);
        self.dirty = true;
    }

    /// 写回。超过上限就只留本轮这一章的段落——缓存丢了只是下次多花几次调用。
    fn save(
        mut self,
        path: Option<&Path>,
        paragraphs: &[Paragraph<'_>],
        key_of: impl Fn(&str) -> String,
    ) {
        let (Some(path), true) = (path, self.dirty) else {
            return;
        };
        if self.map.len() > CACHE_MAX_ENTRIES {
            let keep: std::collections::HashSet<String> =
                paragraphs.iter().map(|p| key_of(p.text)).collect();
            self.map.retain(|k, _| keep.contains(k));
        }
        let Ok(json) = serde_json::to_vec(&self.map) else {
            return;
        };
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!(path = %dir.display(), error = %e, "建不了缓存目录");
            return;
        }
        if let Err(e) = crate::atomic::write(path, &json) {
            tracing::warn!(error = %e, "写模型校对缓存失败");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    #![allow(clippy::string_slice)] // 用 locate 出来的区间切原文，正是要验证它落在字符边界上

    use super::*;
    use mj_text::proof::split_paragraphs;

    fn hit(quote: &str) -> ModelIssue {
        ModelIssue {
            para: 0,
            quote: quote.into(),
            category: "Grammar".into(),
            message: "读着别扭".into(),
            suggestion: String::new(),
            confidence: 0.8,
        }
    }

    // ---- 定位：模型抄回来的 quote 要能切回原文 ----

    #[test]
    fn quote_maps_to_chapter_offsets() {
        let text = "第一段。\n\n他推开门，风雪扑面而来。";
        let paras = split_paragraphs(text);
        let issues = locate(&[hit("风雪扑面")], &paras[1], 0);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            text.get(issues[0].range.clone()),
            Some("风雪扑面"),
            "整章坐标要能切回原文"
        );
        assert_eq!(issues[0].source, Source::Llm, "来源必须标成模型");
        assert_eq!(issues[0].rule_id, "llm.review");
    }

    /// 模型抄错一个字 → 丢掉，**绝不**猜位置。切错一刀就是毁稿（§0）。
    #[test]
    fn quote_not_in_text_is_dropped() {
        let paras = split_paragraphs("他推开门，风雪扑面而来。");
        assert!(
            locate(&[hit("风雪扑脸")], &paras[0], 0).is_empty(),
            "抄不准的条目必须丢弃"
        );
        assert!(locate(&[hit("")], &paras[0], 0).is_empty(), "空 quote 丢弃");
    }

    #[test]
    fn suggestion_becomes_a_single_suggestion() {
        let paras = split_paragraphs("他跑的很快。");
        let mut m = hit("跑的");
        m.suggestion = "跑得".into();
        let issues = locate(&[m], &paras[0], 0);
        assert_eq!(issues[0].suggestions, vec!["跑得".to_string()]);
        // 空建议不该变成一条空字符串建议——UI 会画出个空条目。
        let empty = locate(&[hit("跑的")], &paras[0], 0);
        assert!(empty[0].suggestions.is_empty());
    }

    #[test]
    fn confidence_is_clamped() {
        let paras = split_paragraphs("正文。");
        let mut m = hit("正文");
        m.confidence = 9.9;
        assert_eq!(locate(&[m], &paras[0], 0)[0].confidence, 1.0);
    }

    // ---- 响应解析：真实形状 ----

    /// `content` 是块数组，开着思考时第一块是 thinking——不能照着 content[0] 取。
    #[test]
    fn reads_text_block_past_the_thinking_block() {
        let raw = serde_json::json!({
            "type": "message",
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": r#"{"issues":[{"para":0,"quote":"跑的","category":"Typo","message":"的地得","suggestion":"跑得","confidence":0.9}]}"#}
            ]
        })
        .to_string();
        let issues = parse_output(&raw).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].quote, "跑的");
        assert_eq!(issues[0].suggestion, "跑得");
    }

    #[test]
    fn empty_issue_list_is_success_not_failure() {
        let raw = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": r#"{"issues":[]}"#}]
        })
        .to_string();
        assert!(parse_output(&raw).unwrap().is_empty());
    }

    /// 截断的回复解出来只会是垃圾坐标，必须当失败并说清原因。
    #[test]
    fn truncated_reply_is_an_error_with_an_actionable_message() {
        let raw = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": r#"{"issues":[{"para":0,"quo"#}]
        })
        .to_string();
        let e = parse_output(&raw).unwrap_err();
        assert!(e.contains("max_tokens"), "要指出是被截断了：{e}");
    }

    #[test]
    fn refusal_is_reported_not_parsed() {
        let raw = serde_json::json!({"stop_reason": "refusal", "content": []}).to_string();
        assert!(parse_output(&raw).unwrap_err().contains("拒绝"));
    }

    #[test]
    fn non_json_response_errors_with_a_sample() {
        assert!(parse_output("<html>502 Bad Gateway</html>").is_err());
        let raw = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "抱歉，我无法完成"}]
        })
        .to_string();
        let e = parse_output(&raw).unwrap_err();
        assert!(e.contains("抱歉"), "要把模型实际吐的东西带上：{e}");
    }

    #[test]
    fn extracts_api_error_message() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"thinking: unsupported"}}"#;
        assert_eq!(api_error_message(body), "thinking: unsupported");
        assert!(api_error_message("plain text boom").contains("boom"));
    }

    // ---- 请求形状 ----

    #[test]
    fn request_uses_structured_output_not_prefill() {
        let cfg = LlmProof::default();
        let r = LlmProofreader::new(cfg);
        let paras = split_paragraphs("正文。");
        let refs: Vec<&Paragraph<'_>> = paras.iter().collect();
        let body = r.request_body(&refs, &ProofContext::default());

        assert_eq!(
            body["output_config"]["format"]["type"], "json_schema",
            "要靠 output_config 保证 JSON，而不是预填"
        );
        // 预填（最后一条是 assistant）在 4.6 之后的模型上直接 400。
        let msgs = body["messages"].as_array().unwrap();
        assert!(
            msgs.iter().all(|m| m["role"] != "assistant"),
            "不能有 assistant 预填：{msgs:?}"
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(
            body["thinking"].get("budget_tokens").is_none(),
            "budget_tokens 已被移除，发了就 400"
        );
    }

    /// schema 的三条硬要求：object 都要 additionalProperties:false、required 列全、
    /// 不能带 minimum/maximum 这类不支持的约束。
    #[test]
    fn schema_meets_structured_output_rules() {
        let s = output_schema();
        assert_eq!(s["additionalProperties"], false);
        let item = &s["properties"]["issues"]["items"];
        assert_eq!(item["additionalProperties"], false);
        let props: Vec<&String> = item["properties"].as_object().unwrap().keys().collect();
        let required: Vec<&str> = item["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(props.len(), required.len(), "required 要列全所有字段");
        for p in props {
            assert!(required.contains(&p.as_str()), "{p} 不在 required 里");
        }
        let text = s.to_string();
        for banned in ["minimum", "maximum", "minLength", "maxLength", "multipleOf"] {
            assert!(!text.contains(banned), "schema 不支持 {banned}");
        }
    }

    /// 老模型不认 effort，留了「空串 = 不下发」这条退路。
    #[test]
    fn empty_effort_is_omitted() {
        let mut cfg = LlmProof {
            effort: String::new(),
            thinking: false,
            ..LlmProof::default()
        };
        cfg.model = "claude-haiku-4-5".into();
        let r = LlmProofreader::new(cfg);
        let paras = split_paragraphs("正文。");
        let refs: Vec<&Paragraph<'_>> = paras.iter().collect();
        let body = r.request_body(&refs, &ProofContext::default());
        assert!(body["output_config"].get("effort").is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn prompt_carries_names_and_numbers_paragraphs() {
        let paras = split_paragraphs("第一段。\n\n第二段。");
        let refs: Vec<&Paragraph<'_>> = paras.iter().collect();
        let ctx = ProofContext::new(vec!["沈砚".to_string()]);
        let p = user_prompt(&refs, &ctx);
        assert!(p.contains("沈砚"), "专名要给模型，否则人名全被当错别字");
        assert!(p.contains("[0] 第一段。"));
        assert!(p.contains("[1] 第二段。"));
    }

    // ---- 开关与 [MUST] ----

    #[test]
    fn disabled_by_default_and_silent() {
        let r = LlmProofreader::new(LlmProof::default());
        assert!(!r.is_enabled());
        assert!(r.setup_problem().is_none(), "没开的功能不该唠叨");
        let out = r.check(
            &split_paragraphs("正文"),
            &ProofContext::default(),
            &CancelToken::new(),
        );
        assert!(out.issues.is_empty() && out.warning.is_none());
    }

    /// §6.8 [MUST]：首次开启必须明确同意。只把 enabled 打开是不够的。
    #[test]
    fn enabled_without_consent_is_off() {
        let r = LlmProofreader::new(LlmProof {
            enabled: true,
            consented: false,
            ..LlmProof::default()
        });
        assert!(!r.is_enabled(), "没同意就不该发正文出去");
        assert!(r.setup_problem().unwrap().contains("第三方"));
    }

    /// §6.8 [MUST]：密钥不得明文写进 config.toml。
    ///
    /// 这是闸门不是提醒：`extra` 会把未知字段原样回写（§8 前向兼容），
    /// 放过去等于让密钥在 config.toml 里长住。
    #[test]
    fn plaintext_key_in_config_blocks_the_backend() {
        for field in ["api_key", "API_KEY", "token", "secret"] {
            let mut extra = toml::Table::new();
            extra.insert(field.into(), toml::Value::String("sk-ant-xxx".into()));
            let cfg = LlmProof {
                enabled: true,
                consented: true,
                extra,
                ..LlmProof::default()
            };
            assert!(cfg.plaintext_secret_field().is_some(), "{field} 该被认出来");
            let r = LlmProofreader::new(cfg);
            assert!(!r.is_enabled(), "{field}：配置里有明文密钥时必须拒跑");
            let p = r.setup_problem().unwrap();
            assert!(p.contains("环境变量"), "要告诉用户怎么改：{p}");
            assert!(!p.contains("sk-ant-xxx"), "提示里不能把密钥再抄一遍：{p}");
        }
    }

    /// 配置里正常的字段不能被误判成密钥。
    #[test]
    fn ordinary_unknown_fields_are_not_secrets() {
        let mut extra = toml::Table::new();
        extra.insert("future_knob".into(), toml::Value::Integer(1));
        let cfg = LlmProof {
            extra,
            ..LlmProof::default()
        };
        assert!(cfg.plaintext_secret_field().is_none());
    }

    // ---- 分批 ----

    #[test]
    fn batches_split_by_paragraph_count_and_chars() {
        let text = (0..10)
            .map(|i| format!("第{i}段。"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let paras = split_paragraphs(&text);
        let pending: Vec<usize> = (0..paras.len()).collect();

        let r = LlmProofreader::new(LlmProof {
            batch_paragraphs: 3,
            ..LlmProof::default()
        });
        let b = r.batches(&paras, &pending);
        assert_eq!(b.len(), 4, "10 段按 3 段/批 → 4 批：{b:?}");
        assert_eq!(b[0], vec![0, 1, 2]);
        assert_eq!(b[3], vec![9]);

        // 段数没到上限，也要能被字数上限切开（batch_chars 下限 200）。
        let long = "啊".repeat(150);
        let text = format!("{long}\n\n{long}\n\n{long}");
        let paras = split_paragraphs(&text);
        let pending: Vec<usize> = (0..paras.len()).collect();
        let r = LlmProofreader::new(LlmProof {
            batch_chars: 200,
            ..LlmProof::default()
        });
        let b = r.batches(&paras, &pending);
        assert_eq!(
            b,
            vec![vec![0], vec![1], vec![2]],
            "每段 150 字，200 字一批装不下两段"
        );
    }

    /// 单段就超上限时不能切它——切开模型就看不全句子了。
    #[test]
    fn oversized_paragraph_goes_out_alone() {
        let huge = "啊".repeat(5000);
        let paras = split_paragraphs(&huge);
        let r = LlmProofreader::new(LlmProof::default());
        let b = r.batches(&paras, &[0]);
        assert_eq!(b, vec![vec![0]]);
    }

    #[test]
    fn only_pending_paragraphs_are_batched() {
        let text = "一。\n\n二。\n\n三。";
        let paras = split_paragraphs(text);
        let r = LlmProofreader::new(LlmProof::default());
        assert_eq!(r.batches(&paras, &[1]), vec![vec![1]], "命中缓存的不该再送");
        assert!(r.batches(&paras, &[]).is_empty());
    }

    // ---- 缓存 ----

    #[test]
    fn cache_key_changes_with_model_and_text() {
        let a = LlmProofreader::new(LlmProof::default());
        let b = LlmProofreader::new(LlmProof {
            model: "claude-haiku-4-5".into(),
            ..LlmProof::default()
        });
        assert_ne!(a.cache_key("同一段"), b.cache_key("同一段"), "换模型要失效");
        assert_ne!(a.cache_key("甲"), a.cache_key("乙"), "换正文要失效");
        assert_eq!(a.cache_key("甲"), a.cache_key("甲"), "同输入要稳定");
    }

    #[test]
    fn cache_roundtrips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache").join("proof_llm.json");
        let paras = split_paragraphs("正文。");

        let mut c = Cache::load(Some(&path));
        assert!(c.get("k").is_none());
        c.put("k".into(), vec![hit("正文")]);
        c.save(Some(&path), &paras, |t| t.to_string());

        let back = Cache::load(Some(&path));
        assert_eq!(back.get("k").unwrap()[0].quote, "正文");
    }

    #[test]
    fn corrupt_cache_is_empty_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proof_llm.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Cache::load(Some(&path)).get("whatever").is_none());
    }

    /// 没改动的缓存不写盘——F7 一次就重写一遍文件是白费的 IO。
    #[test]
    fn unchanged_cache_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proof_llm.json");
        Cache::load(Some(&path)).save(Some(&path), &[], |t| t.to_string());
        assert!(!path.exists(), "没改动就不该建文件");
    }

    // ---- 真跑一趟 HTTP ----
    //
    // 上面那些断言都只看形状，一个字节都没发出去过。这里起一个本地桩服务，
    // 把 check → 建请求 → ureq → 解响应 → 定位 这条链整个走通：
    // 请求头对不对、body 送没送到、响应块挑得对不对，只有真发一次才知道。
    // 不碰网络也不要密钥。

    /// 一个只会说 HTTP/1.1 的最小桩服务。按 `replies` 顺序应答，收下的请求原样留存。
    struct Stub {
        url: String,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Stub {
        /// `replies` 每项是 `(状态码, body)`，用完为止（多出来的请求收不到应答）。
        fn start(replies: Vec<(u16, String)>) -> Self {
            use std::io::{BufRead as _, Read as _, Write as _};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = std::sync::Arc::clone(&seen);

            let handle = std::thread::spawn(move || {
                for (code, body) in replies {
                    let Ok((sock, _)) = listener.accept() else {
                        return;
                    };
                    let mut reader = std::io::BufReader::new(&sock);
                    // 请求头 + Content-Length。
                    let mut head = String::new();
                    let mut len = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                        let done = line == "\r\n" || line == "\n";
                        head.push_str(&line);
                        if done {
                            break;
                        }
                    }
                    let mut payload = vec![0u8; len];
                    let _ = reader.read_exact(&mut payload);
                    head.push_str(&String::from_utf8_lossy(&payload));
                    sink.lock().unwrap().push(head);

                    let mut sock = &sock;
                    let _ = write!(
                        sock,
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.flush();
                }
            });
            Self {
                url,
                seen,
                handle: Some(handle),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn ok_body(issues: &str) -> String {
        serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": format!(r#"{{"issues":[{issues}]}}"#)}
            ]
        })
        .to_string()
    }

    fn reader_at(url: &str) -> LlmProofreader {
        LlmProofreader::new(LlmProof {
            enabled: true,
            consented: true,
            endpoint: url.into(),
            timeout_ms: 5_000,
            ..LlmProof::default()
        })
    }

    #[test]
    fn end_to_end_against_a_stub_server() {
        let issue = r#"{"para":0,"quote":"跑的很快","category":"Typo","message":"该用「得」","suggestion":"跑得很快","confidence":0.9}"#;
        let stub = Stub::start(vec![(200, ok_body(issue))]);
        let text = "他跑的很快，一路奔到城门。";
        let paras = split_paragraphs(text);

        let out = reader_at(&stub.url).check_with_key(
            "sk-test",
            &paras,
            &ProofContext::default(),
            &CancelToken::new(),
        );

        assert!(out.warning.is_none(), "{:?}", out.warning);
        assert_eq!(out.issues.len(), 1);
        assert_eq!(
            text.get(out.issues[0].range.clone()),
            Some("跑的很快"),
            "区间要能切回原文"
        );
        assert_eq!(out.issues[0].suggestions, vec!["跑得很快".to_string()]);
        assert_eq!(out.issues[0].source, Source::Llm);

        // 请求本身：鉴权头、版本头、以及 body 里确实带着正文和 schema。
        let req = &stub.requests()[0];
        let low = req.to_ascii_lowercase();
        assert!(low.contains("post /v1/messages"), "{req}");
        assert!(low.contains("x-api-key: sk-test"), "缺鉴权头：{req}");
        assert!(
            low.contains(&format!("anthropic-version: {API_VERSION}")),
            "缺版本头：{req}"
        );
        assert!(low.contains("content-type: application/json"), "{req}");
        assert!(req.contains("跑的很快"), "正文没送到：{req}");
        assert!(req.contains("json_schema"), "没下发 schema：{req}");
    }

    /// 429 要退避重试，不是当场判死。
    #[test]
    fn rate_limit_is_retried() {
        let busy = r#"{"error":{"message":"rate limited"}}"#.to_string();
        let stub = Stub::start(vec![(429, busy), (200, ok_body(""))]);
        let paras = split_paragraphs("正文。");
        let out = reader_at(&stub.url).check_with_key(
            "k",
            &paras,
            &ProofContext::default(),
            &CancelToken::new(),
        );
        assert!(
            out.warning.is_none(),
            "重试后成功就不该报警：{:?}",
            out.warning
        );
        assert_eq!(stub.requests().len(), 2, "429 之后应重试一次");
    }

    /// 401 是配置问题，重试一百次也是 401——要立刻停，并说清是哪个环境变量。
    #[test]
    fn auth_failure_does_not_retry() {
        let denied = r#"{"error":{"message":"invalid x-api-key"}}"#.to_string();
        let stub = Stub::start(vec![(401, denied)]);
        let paras = split_paragraphs("正文。");
        let out = reader_at(&stub.url).check_with_key(
            "bad",
            &paras,
            &ProofContext::default(),
            &CancelToken::new(),
        );
        assert_eq!(stub.requests().len(), 1, "鉴权失败不该重试");
        let w = out.warning.unwrap();
        assert!(w.contains("ANTHROPIC_API_KEY"), "要指出改哪个环境变量：{w}");
        assert!(out.issues.is_empty());
    }

    /// §6.8「绝不影响编辑」：后端全挂也只是没有额外的问题 + 一句提示。
    #[test]
    fn dead_endpoint_warns_but_never_errors() {
        // 起了就关，端口上没人听。
        let url = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/v1/messages", l.local_addr().unwrap())
        };
        let mut cfg = LlmProof {
            enabled: true,
            consented: true,
            endpoint: url,
            timeout_ms: 1_000,
            ..LlmProof::default()
        };
        cfg.batch_paragraphs = 1;
        let paras = split_paragraphs("正文。");
        let out = LlmProofreader::new(cfg).check_with_key(
            "k",
            &paras,
            &ProofContext::default(),
            &CancelToken::new(),
        );
        assert!(out.issues.is_empty());
        assert!(out.warning.is_some(), "连不上要说一声");
    }

    /// 命中缓存的段落不再发请求（§6.8：未改动的段落不重复请求）。
    #[test]
    fn cached_paragraphs_are_not_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("proof_llm.json");
        let text = "他跑的很快。";
        let paras = split_paragraphs(text);
        let issue = r#"{"para":0,"quote":"跑的","category":"Typo","message":"的地得","suggestion":"跑得","confidence":0.9}"#;
        // 只备一次应答：第二趟若还发请求，就会拿不到响应而报警。
        let stub = Stub::start(vec![(200, ok_body(issue))]);

        let first = reader_at(&stub.url)
            .with_cache(cache.clone())
            .check_with_key("k", &paras, &ProofContext::default(), &CancelToken::new());
        assert_eq!(first.issues.len(), 1);

        let second = reader_at(&stub.url).with_cache(cache).check_with_key(
            "k",
            &paras,
            &ProofContext::default(),
            &CancelToken::new(),
        );
        assert_eq!(stub.requests().len(), 1, "第二趟不该再发请求");
        assert!(second.warning.is_none());
        assert_eq!(second.issues.len(), 1, "缓存里的结果要能重新定位出来");
        assert_eq!(text.get(second.issues[0].range.clone()), Some("跑的"));
    }

    /// 取消后不再开新批（§7：长任务要能 Esc 掉）。
    #[test]
    fn cancellation_stops_before_sending() {
        let stub = Stub::start(vec![]);
        let cancel = CancelToken::new();
        cancel.cancel();
        let paras = split_paragraphs("一。\n\n二。");
        let out =
            reader_at(&stub.url).check_with_key("k", &paras, &ProofContext::default(), &cancel);
        assert!(stub.requests().is_empty(), "已取消就不该发请求");
        assert!(out.issues.is_empty());
    }

    /// 缓存爆了只留本轮的段落——它是派生数据，扔了只是下次多花几次调用。
    #[test]
    fn oversized_cache_is_trimmed_to_the_current_chapter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proof_llm.json");
        let paras = split_paragraphs("留下我。");

        let mut c = Cache::load(Some(&path));
        for i in 0..=CACHE_MAX_ENTRIES {
            c.put(format!("old-{i}"), vec![]);
        }
        c.put("留下我。".into(), vec![hit("留下我")]);
        c.save(Some(&path), &paras, |t| t.to_string());

        let back = Cache::load(Some(&path));
        assert_eq!(back.map.len(), 1, "该只剩本轮这一章的段落");
        assert!(back.get("留下我。").is_some());
    }
}
