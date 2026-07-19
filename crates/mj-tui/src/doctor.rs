//! `mj doctor`：探测终端能力并打印报告。见 doc.md §12.2。
//!
//! §12.2 原话：「`mj doctor` 是排查用户环境问题的第一手段，别省。」
//!
//! 它同时承担另一件事：§2.1 的终端能力表被标了 `[VERIFY]`——**不得照抄文档**。
//! 我们没法在开发机上把 kitty/alacritty/WT 都验一遍，但用户可以：doctor 在
//! **他自己的终端里**跑，报告的是实际探测结果。凡是我们只能推断、没法确证的，
//! 报告里明说是「推断」，不冒充事实。

use crate::font::{EnvProbe, FontCap, TerminalKind, config_snippet};
use crate::theme::ColorDepth;

/// 一条探测结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub item: String,
    pub value: String,
    /// 补充说明：依据是什么、拿不准的地方在哪。
    pub note: String,
}

/// 完整报告。
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// 字体改不了时给出的配置片段（§6.10 [MUST]）。
    pub snippet: Option<String>,
}

impl Report {
    /// 生成报告。纯函数：吃探测到的环境，吐结论，故可整测。
    pub fn build(env: &EnvProbe, font_family: &str, font_size: f32) -> Self {
        let kind = TerminalKind::from_env(env);
        let depth = ColorDepth::from_env(
            env.colorterm.as_deref(),
            env.term.as_deref(),
            env.term_program.as_deref(),
        );
        let caps = kind.expected_caps();

        let mut findings = Vec::new();

        findings.push(Finding {
            item: "终端".into(),
            value: kind.label().into(),
            note: describe_evidence(env),
        });

        findings.push(Finding {
            item: "真彩色".into(),
            value: match depth {
                ColorDepth::TrueColor => "支持".into(),
                ColorDepth::Ansi256 => "按 256 色处理".into(),
            },
            note: match depth {
                ColorDepth::TrueColor => "主题颜色按 RGB 原样下发".into(),
                ColorDepth::Ansi256 => "没探到 COLORTERM=truecolor；主题会自动取 256 色近似。\
                     终端其实支持的话，设 COLORTERM=truecolor 即可"
                    .into(),
            },
        });

        findings.push(Finding {
            item: "字体·字体族".into(),
            value: yes_no(caps.contains(FontCap::SET_FAMILY)),
            note: if caps.contains(FontCap::SET_FAMILY) {
                "推断可用（OSC 50）。本项未经真机验证，以实际效果为准".into()
            } else {
                kind.unsupported_reason()
            },
        });

        findings.push(Finding {
            item: "字体·字号".into(),
            value: yes_no(caps.contains(FontCap::SET_SIZE)),
            note: if caps.contains(FontCap::SET_SIZE) {
                "推断可用（kitty 远程控制）。需终端侧 allow_remote_control，否则会失败".into()
            } else {
                kind.unsupported_reason()
            },
        });

        findings.push(Finding {
            item: "剪贴板".into(),
            value: "OSC 52（尽力而为）".into(),
            note: "终端不回话，无从确认是否真放进了剪贴板；ssh 场景下也能用".into(),
        });

        findings.push(Finding {
            item: "键盘协议".into(),
            value: if matches!(kind, TerminalKind::Kitty | TerminalKind::WezTerm) {
                "可能支持 kitty 协议".into()
            } else {
                "传统模式".into()
            },
            note: "传统模式下 Ctrl+Shift+S、Ctrl+Tab 到不了程序（终端不编码 Shift），\
                   故打快照用 F9"
                .into(),
        });

        let snippet = if caps.is_empty() {
            config_snippet(kind, font_family, font_size)
        } else {
            None
        };

        Self { findings, snippet }
    }

    /// 渲染成可打印的文本。
    pub fn render(&self) -> String {
        let mut s = String::from("墨简 · 终端能力报告\n\n");
        let w = self
            .findings
            .iter()
            .map(|f| display_width(&f.item))
            .max()
            .unwrap_or(8);
        for f in &self.findings {
            let pad = " ".repeat(w.saturating_sub(display_width(&f.item)));
            s.push_str(&format!("{}{}  {}\n", f.item, pad, f.value));
            if !f.note.is_empty() {
                s.push_str(&format!("{}  └ {}\n", " ".repeat(w), f.note));
            }
        }
        if let Some(snip) = &self.snippet {
            s.push_str("\n改不了字体，但可以把下面这段贴进终端配置，重启终端后生效：\n\n");
            for line in snip.lines() {
                s.push_str(&format!("    {line}\n"));
            }
        }
        s
    }
}

fn yes_no(b: bool) -> String {
    if b {
        "可改".into()
    } else {
        "不可改".into()
    }
}

fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// 说清判定依据——用户要能自己核对我们凭什么这么认。
fn describe_evidence(e: &EnvProbe) -> String {
    let mut bits = Vec::new();
    if e.kitty_window_id.is_some() {
        bits.push("KITTY_WINDOW_ID".to_string());
    }
    if e.wezterm_pane.is_some() {
        bits.push("WEZTERM_PANE".to_string());
    }
    if e.wt_session.is_some() {
        bits.push("WT_SESSION".to_string());
    }
    if let Some(tp) = &e.term_program {
        bits.push(format!("TERM_PROGRAM={tp}"));
    }
    if let Some(t) = &e.term {
        bits.push(format!("TERM={t}"));
    }
    if bits.is_empty() {
        "没有可用于判定的环境变量".into()
    } else {
        format!("依据：{}", bits.join("，"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn env(term: Option<&str>, prog: Option<&str>, colorterm: Option<&str>) -> EnvProbe {
        EnvProbe {
            term: term.map(|s| s.into()),
            term_program: prog.map(|s| s.into()),
            colorterm: colorterm.map(|s| s.into()),
            ..EnvProbe::default()
        }
    }

    #[test]
    fn reports_the_detected_terminal() {
        let e = EnvProbe {
            kitty_window_id: Some("1".into()),
            ..EnvProbe::default()
        };
        let r = Report::build(&e, "Source Han Serif", 14.0);
        let text = r.render();
        assert!(text.contains("kitty"), "{text}");
        assert!(text.contains("KITTY_WINDOW_ID"), "要说清判定依据：{text}");
    }

    #[test]
    fn truecolor_reported_from_colorterm() {
        let r = Report::build(&env(None, None, Some("truecolor")), "X", 14.0);
        let text = r.render();
        assert!(text.contains("支持"), "{text}");
    }

    /// 没探到真彩时要给出**可操作**的建议，而不是只说「不支持」。
    #[test]
    fn ansi256_tells_user_how_to_fix_it() {
        let r = Report::build(&env(Some("xterm-256color"), None, None), "X", 14.0);
        let text = r.render();
        assert!(text.contains("256"), "{text}");
        assert!(
            text.contains("COLORTERM=truecolor"),
            "要告诉用户怎么开：{text}"
        );
    }

    /// §6.10 [MUST]：字体改不了时给配置片段。
    #[test]
    fn unsupported_font_yields_a_config_snippet() {
        let e = EnvProbe {
            wt_session: Some("x".into()),
            ..EnvProbe::default()
        };
        let r = Report::build(&e, "等距更纱黑体", 14.0);
        assert!(r.snippet.is_some(), "Windows Terminal 该给配置片段");
        let text = r.render();
        assert!(text.contains("等距更纱黑体"), "{text}");
        assert!(text.contains("重启终端"), "要说明贴完需要重启：{text}");
    }

    /// 能改字体的终端就不必给片段——那是给「做不到」的场景准备的。
    #[test]
    fn capable_terminal_gets_no_snippet() {
        let e = EnvProbe {
            kitty_window_id: Some("1".into()),
            ..EnvProbe::default()
        };
        let r = Report::build(&e, "X", 14.0);
        assert!(r.snippet.is_none());
    }

    /// 拿不准的地方必须写明是推断，不能冒充事实（§2.1 的 [VERIFY]）。
    #[test]
    fn hedges_where_we_cannot_be_sure() {
        let e = EnvProbe {
            kitty_window_id: Some("1".into()),
            ..EnvProbe::default()
        };
        let text = Report::build(&e, "X", 14.0).render();
        assert!(
            text.contains("推断"),
            "字体能力是推断出来的，要说清：{text}"
        );
        assert!(
            text.contains("allow_remote_control"),
            "要点出前置条件：{text}"
        );
    }

    /// 传统键盘模式下够不到的键要写明，免得用户以为功能坏了。
    #[test]
    fn explains_the_unreachable_keys() {
        let text = Report::build(&EnvProbe::default(), "X", 14.0).render();
        assert!(text.contains("Ctrl+Shift+S"), "{text}");
        assert!(text.contains("F9"), "要给出实际可用的替代键：{text}");
    }

    #[test]
    fn report_covers_all_four_areas() {
        // §12.2：探测 truecolor / 字体 / 键盘协议 / 剪贴板。
        let text = Report::build(&EnvProbe::default(), "X", 14.0).render();
        for item in ["真彩色", "字体", "键盘协议", "剪贴板"] {
            assert!(text.contains(item), "报告漏了「{item}」：{text}");
        }
    }

    #[test]
    fn renders_without_env() {
        // 什么变量都没有也要能出报告，不 panic。
        let text = Report::build(&EnvProbe::default(), "X", 14.0).render();
        assert!(text.contains("未知终端"), "{text}");
    }
}
