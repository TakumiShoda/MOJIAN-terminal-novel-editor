//! 外部命令校对后端（`ExternalProofreader`）。见 doc.md §6.8。
//!
//! 起一个用户配置的进程，stdin 送 JSON、stdout 读 JSON，契约带版本号。
//! 实现在 mj-core 而非 mj-text：起进程读写管道是 IO，按 §4 分层铁律归这里。
//!
//! # 一条铁律：绝不影响编辑
//!
//! §6.8 明写「超时、非零退出、非法 JSON → 记日志 + UI 提示，**绝不影响编辑**」。
//! 故本模块**不返回 Err**：任何失败都退化成「没有问题 + 一句提示」。
//! 校对是锦上添花，外部程序装没装好、跑不跑得动，都不该拦着人写字。
//!
//! # `start`/`end` 是字符偏移，不是字节偏移
//!
//! §6.8 的契约没写清这一点，而它对中文性命攸关：`start:12,end:14` 若按字节解，
//! 会砍在汉字中间。这里定为**字符偏移**，理由：
//! - 文档自己举的实现例子是 `python -m pycorrector_server`，而 Python（以及 JS）
//!   的字符串下标就是码点，不是字节。外部工具最自然的产出就是字符偏移。
//! - 例子里 `12..14` 跨 2 个单位表示一个两字词，也符合字符计数。
//!
//! 但**绝不轻信**：越界、start≥end、落不到字符边界的，一律丢弃并记日志，
//! 而不是硬切——切错一刀就是毁稿（§0）。

use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};

use mj_text::proof::{Category, Issue, Paragraph, Severity, Source};
use serde::{Deserialize, Serialize};

use crate::config::ExternalProof;

/// 契约版本（§6.8：`"v": 1`）。
const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct Request<'a> {
    v: u32,
    paragraphs: Vec<&'a str>,
    lang: &'a str,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    issues: Vec<ExtIssue>,
}

#[derive(Debug, Deserialize)]
struct ExtIssue {
    para: usize,
    /// **字符**偏移（见模块注释）。
    start: usize,
    end: usize,
    #[serde(default)]
    category: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    suggestions: Vec<String>,
    #[serde(default = "default_confidence")]
    confidence: f32,
}

fn default_confidence() -> f32 {
    0.6
}

/// 跑一趟的结果。**没有 Err**——见模块注释。
#[derive(Debug, Default)]
pub struct Outcome {
    pub issues: Vec<Issue>,
    /// 给用户看的一句话（超时/退出码/解析失败）。None = 一切正常。
    pub warning: Option<String>,
}

impl Outcome {
    fn warn(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        tracing::warn!("外部校对：{msg}");
        Self {
            issues: Vec::new(),
            warning: Some(msg),
        }
    }
}

/// 外部校对后端。
pub struct ExternalProofreader {
    cfg: ExternalProof,
}

impl ExternalProofreader {
    pub fn new(cfg: ExternalProof) -> Self {
        Self { cfg }
    }

    pub fn is_enabled(&self) -> bool {
        self.cfg.enabled && !self.cfg.command.is_empty()
    }

    /// 跑一趟。段落偏移会被还原成整章坐标。
    pub fn check(&self, paragraphs: &[Paragraph<'_>]) -> Outcome {
        if !self.is_enabled() {
            return Outcome::default();
        }
        let texts: Vec<&str> = paragraphs.iter().map(|p| p.text).collect();
        let req = Request {
            v: PROTOCOL_VERSION,
            paragraphs: texts,
            lang: &self.cfg.lang,
        };
        let Ok(payload) = serde_json::to_vec(&req) else {
            return Outcome::warn("请求序列化失败");
        };

        let stdout = match self.run(&payload) {
            Ok(s) => s,
            Err(e) => return Outcome::warn(e),
        };

        let resp: Response = match serde_json::from_slice(&stdout) {
            Ok(r) => r,
            Err(e) => {
                // 把外部程序吐的头几十个字节带上——排查时最有用的就是这个。
                let head: String = String::from_utf8_lossy(&stdout).chars().take(60).collect();
                return Outcome::warn(format!("返回的不是合法 JSON（{e}）：{head}"));
            }
        };
        if resp.v != PROTOCOL_VERSION {
            return Outcome::warn(format!(
                "契约版本不匹配：期望 {PROTOCOL_VERSION}，收到 {}",
                resp.v
            ));
        }

        Outcome {
            issues: map_issues(&resp.issues, paragraphs),
            warning: None,
        }
    }

    /// 起进程、喂 stdin、限时收 stdout。
    fn run(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let (program, args) = self
            .cfg
            .command
            .split_first()
            .ok_or_else(|| "没有配置命令".to_string())?;

        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("起不来命令 {program}：{e}"))?;

        // 先把 stdin 写完并关掉——不关的话对方可能一直等输入，我们就等到超时。
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload);
            let _ = stdin.flush();
            // drop 即关闭，给对方 EOF。
        }

        // std 没有「带超时的 wait」。开一个线程去收，主线程限时等它。
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("mj-external-proof".into())
            .spawn(move || {
                let mut out = Vec::new();
                if let Some(s) = &mut stdout {
                    let _ = s.read_to_end(&mut out);
                }
                let mut err = String::new();
                if let Some(s) = &mut stderr {
                    let _ = s.read_to_string(&mut err);
                }
                let _ = tx.send((out, err));
            })
            .map_err(|e| format!("起不来收集线程：{e}"))?;

        let timeout = std::time::Duration::from_millis(self.cfg.timeout_ms.max(100));
        let (out, err) = match rx.recv_timeout(timeout) {
            Ok(v) => v,
            Err(_) => {
                // 超时：**必须杀掉**，否则一个卡死的外部程序会一直挂在那里，
                // 用户每次校对都多一个僵尸进程。
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("超时（{} 毫秒）已中止", self.cfg.timeout_ms));
            }
        };

        match child.wait() {
            Ok(status) if status.success() => Ok(out),
            Ok(status) => Err(format!(
                "退出码 {}：{}",
                status.code().unwrap_or(-1),
                err.trim().chars().take(80).collect::<String>()
            )),
            Err(e) => Err(format!("等不到进程结束：{e}")),
        }
    }
}

/// 把外部程序给的「段号 + 字符偏移」映射成整章字节偏移。
///
/// 任何一条对不上的都**丢掉并记日志**，不硬切——切在汉字中间就毁稿了（§0）。
fn map_issues(ext: &[ExtIssue], paragraphs: &[Paragraph<'_>]) -> Vec<Issue> {
    let mut out = Vec::new();
    for e in ext {
        let Some(p) = paragraphs.get(e.para) else {
            tracing::warn!(para = e.para, "外部校对：段号越界，丢弃该条");
            continue;
        };
        let Some(range) = char_range_to_bytes(p.text, e.start, e.end) else {
            tracing::warn!(
                para = e.para,
                start = e.start,
                end = e.end,
                "外部校对：字符区间非法，丢弃该条"
            );
            continue;
        };
        out.push(Issue {
            range: (p.offset + range.start)..(p.offset + range.end),
            severity: Severity::Warning,
            category: parse_category(&e.category),
            rule_id: "external".into(),
            message: if e.message.is_empty() {
                "外部校对报告了一处问题".into()
            } else {
                e.message.clone()
            },
            suggestions: e.suggestions.clone(),
            // §6.8 [MUST]：UI 必须区分「本地规则」与「外部/模型」。
            source: Source::External,
            confidence: e.confidence.clamp(0.0, 1.0),
        });
    }
    out.sort_by_key(|i| i.range.start);
    out
}

/// 字符区间 → 字节区间。越界或空区间返回 None。
fn char_range_to_bytes(s: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    if start >= end {
        return None;
    }
    let mut bs = None;
    let mut be = None;
    for (i, (b, _)) in s.char_indices().enumerate() {
        if i == start {
            bs = Some(b);
        }
        if i == end {
            be = Some(b);
            break;
        }
    }
    // end 正好等于总字符数时，落到字符串末尾。
    let total = s.chars().count();
    if end == total {
        be = Some(s.len());
    }
    Some(bs?..be?)
}

fn parse_category(s: &str) -> Category {
    match s.to_ascii_lowercase().as_str() {
        "typo" => Category::Typo,
        "grammar" => Category::Grammar,
        "punct" => Category::Punct,
        "style" => Category::Style,
        "consistency" => Category::Consistency,
        // 外部程序爱写什么写什么，认不出就归病句——它多半是来报这个的。
        _ => Category::Grammar,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    #![allow(clippy::string_slice)] // 用返回的区间切原文，正是要验证它落在字符边界上

    use super::*;
    use mj_text::proof::split_paragraphs;

    fn cfg(command: Vec<&str>) -> ExternalProof {
        ExternalProof {
            enabled: true,
            command: command.into_iter().map(|s| s.to_string()).collect(),
            timeout_ms: 5_000,
            ..ExternalProof::default()
        }
    }

    // ---- 字符偏移映射（与平台无关，总是跑）----

    #[test]
    fn char_offsets_map_to_bytes_for_cjk() {
        let s = "他推开门，风雪扑面。";
        // 第 5..7 个字符是「风雪」。
        let r = char_range_to_bytes(s, 5, 7).unwrap();
        assert_eq!(&s[r], "风雪", "字符偏移要按字符数，不是字节数");
    }

    #[test]
    fn char_range_at_end_of_string() {
        let s = "结尾";
        let r = char_range_to_bytes(s, 0, 2).unwrap();
        assert_eq!(&s[r], "结尾");
    }

    /// 越界/空区间一律丢弃——绝不硬切。
    #[test]
    fn rejects_bad_ranges() {
        let s = "短";
        assert!(char_range_to_bytes(s, 0, 0).is_none(), "空区间");
        assert!(char_range_to_bytes(s, 2, 1).is_none(), "倒置");
        assert!(char_range_to_bytes(s, 0, 99).is_none(), "越界");
        assert!(char_range_to_bytes(s, 5, 6).is_none(), "起点越界");
    }

    #[test]
    fn maps_paragraph_offsets_to_chapter_offsets() {
        let text = "第一段。\n\n他推开门，风雪扑面。";
        let paras = split_paragraphs(text);
        let ext = vec![ExtIssue {
            para: 1,
            start: 5,
            end: 7,
            category: "typo".into(),
            message: "试试".into(),
            suggestions: vec!["风霜".into()],
            confidence: 0.8,
        }];
        let issues = map_issues(&ext, &paras);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            text.get(issues[0].range.clone()),
            Some("风雪"),
            "整章坐标要能切回原文"
        );
        assert_eq!(issues[0].source, Source::External, "来源必须标成外部");
    }

    #[test]
    fn drops_issues_with_bad_paragraph_index() {
        let paras = split_paragraphs("只有一段。");
        let ext = vec![ExtIssue {
            para: 99,
            start: 0,
            end: 1,
            category: String::new(),
            message: String::new(),
            suggestions: vec![],
            confidence: 0.5,
        }];
        assert!(map_issues(&ext, &paras).is_empty(), "段号越界应丢弃");
    }

    #[test]
    fn unknown_category_falls_back_to_grammar() {
        assert_eq!(parse_category("Typo"), Category::Typo);
        assert_eq!(parse_category("什么鬼"), Category::Grammar);
    }

    #[test]
    fn disabled_backend_does_nothing() {
        let p = ExternalProofreader::new(ExternalProof::default());
        assert!(!p.is_enabled());
        let out = p.check(&split_paragraphs("随便"));
        assert!(out.issues.is_empty());
        assert!(out.warning.is_none(), "关着的后端不该报警");
    }

    /// 配了 enabled 但没给命令，也算没开——不能去跑一个空命令。
    #[test]
    fn enabled_without_command_is_still_off() {
        let p = ExternalProofreader::new(ExternalProof {
            enabled: true,
            ..ExternalProof::default()
        });
        assert!(!p.is_enabled());
    }

    // ---- 真起进程（要 shell，故只在 unix 上跑）----
    //
    // Windows 上没有 `sh`，而为此引一个跨平台的测试辅助程序不值当：
    // 契约解析与偏移映射这些真正容易错的部分，上面那些测试已经盖住了，
    // 且它们与平台无关。

    #[cfg(unix)]
    mod process {
        use super::*;

        #[test]
        fn reads_issues_from_a_real_process() {
            let json = r#"{"v":1,"issues":[{"para":0,"start":0,"end":2,"category":"Typo","message":"试","suggestions":["改"],"confidence":0.9}]}"#;
            let p = ExternalProofreader::new(cfg(vec![
                "sh",
                "-c",
                &format!("cat >/dev/null; printf '%s' '{json}'"),
            ]));
            let out = p.check(&split_paragraphs("错字在此。"));
            assert!(out.warning.is_none(), "{:?}", out.warning);
            assert_eq!(out.issues.len(), 1);
            assert_eq!(out.issues[0].message, "试");
        }

        /// 外部程序确实收到了我们送的 JSON。
        #[test]
        fn sends_the_request_on_stdin() {
            // 把 stdin 原样回显成一条 message，借此验证送出去的内容。
            let p = ExternalProofreader::new(cfg(vec![
                "sh",
                "-c",
                r#"IN=$(cat); case "$IN" in *'"v":1'*) printf '{"v":1,"issues":[]}';; *) printf '{"v":9,"issues":[]}';; esac"#,
            ]));
            let out = p.check(&split_paragraphs("随便写点。"));
            assert!(
                out.warning.is_none(),
                "请求里该带契约版本号，实际：{:?}",
                out.warning
            );
        }

        /// §6.8：非零退出 → 提示，但**绝不影响编辑**（不返回错误、不 panic）。
        #[test]
        fn nonzero_exit_warns_but_does_not_fail() {
            let p = ExternalProofreader::new(cfg(vec![
                "sh",
                "-c",
                "cat >/dev/null; echo 出错了 >&2; exit 3",
            ]));
            let out = p.check(&split_paragraphs("正文"));
            assert!(out.issues.is_empty());
            let w = out.warning.expect("该有提示");
            assert!(w.contains('3'), "提示里要带退出码：{w}");
        }

        /// 非法 JSON → 提示里要带上对方吐的内容，便于排查。
        #[test]
        fn invalid_json_warns_with_a_sample() {
            let p = ExternalProofreader::new(cfg(vec![
                "sh",
                "-c",
                "cat >/dev/null; printf 'not json at all'",
            ]));
            let out = p.check(&split_paragraphs("正文"));
            let w = out.warning.expect("该有提示");
            assert!(w.contains("not json"), "要带上对方吐的内容：{w}");
        }

        /// 契约版本不匹配要拒收——对方换了协议还硬解，只会解出垃圾坐标。
        #[test]
        fn version_mismatch_is_rejected() {
            let p = ExternalProofreader::new(cfg(vec![
                "sh",
                "-c",
                r#"cat >/dev/null; printf '{"v":99,"issues":[]}'"#,
            ]));
            let out = p.check(&split_paragraphs("正文"));
            assert!(out.warning.unwrap().contains("99"));
        }

        /// §6.8：超时要中止并提示；卡死的进程必须被杀掉。
        #[test]
        fn timeout_is_enforced() {
            let mut c = cfg(vec!["sh", "-c", "cat >/dev/null; sleep 30"]);
            c.timeout_ms = 300;
            let p = ExternalProofreader::new(c);
            let start = std::time::Instant::now();
            let out = p.check(&split_paragraphs("正文"));
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "该及时超时返回"
            );
            assert!(out.warning.unwrap().contains("超时"));
        }

        /// 命令根本不存在：提示，不崩。
        #[test]
        fn missing_command_warns() {
            let p = ExternalProofreader::new(cfg(vec!["这个命令肯定不存在-mj"]));
            let out = p.check(&split_paragraphs("正文"));
            assert!(out.issues.is_empty());
            assert!(out.warning.is_some());
        }
    }
}
