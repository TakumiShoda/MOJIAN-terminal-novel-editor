//! 「正文将发送到第三方服务」的同意框。见 doc.md §6.8 第 3 条 `[MUST]`。
//!
//! # 为什么不复用 `Confirm`
//!
//! 那个是批量作业的确认框，措辞和字段都长在 `BatchKind` 上（改哪些章、
//! 把什么换成什么）。这里要回答的是完全不同的一个问题：**我的稿子要发到哪去、
//! 发多少、发给谁**。这种事得当面把话说全，塞进一个泛化的 y/n 框里等于没说。
//!
//! 默认停在「不同意」。手滑连按两下回车不该把稿子发出去。

/// 同意框。展示的都是从配置里真实读出来的值——不能只写「第三方服务」四个字，
/// 用户有权知道具体是哪个地址、哪个模型、用的哪个环境变量。
pub struct Consent {
    endpoint: String,
    model: String,
    key_env: String,
    /// 默认 false = 停在「不同意」。
    yes: bool,
}

impl Consent {
    pub fn new(endpoint: String, model: String, key_env: String) -> Self {
        Self {
            endpoint,
            model,
            key_env,
            yes: false,
        }
    }

    pub fn is_yes(&self) -> bool {
        self.yes
    }

    /// ←/→ 或 Tab 切换。
    pub fn toggle(&mut self) {
        self.yes = !self.yes;
    }

    pub fn title(&self) -> &'static str {
        "开启模型校对前，请先确认"
    }

    pub fn lines(&self) -> Vec<String> {
        vec![
            "开启后，你**当前章的正文**会被发送到下面这个地址：".into(),
            String::new(),
            format!("  地址：{}", self.endpoint),
            format!("  模型：{}", self.model),
            format!("  密钥：从环境变量 {} 读取", self.key_env),
            String::new(),
            "要知道的几件事：".into(),
            "  · 只在你手动触发时发送，不会自动扫描全书。".into(),
            "  · 一次只发当前这一章，不含角色卡与其他章节。".into(),
            "  · 对方如何存储与使用这些文字，由对方的条款决定，墨简管不着。".into(),
            "  · 随时可在 config.toml 里把 [proof.llm] 的 enabled 改回 false。".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consent() -> Consent {
        Consent::new(
            "https://api.anthropic.com/v1/messages".into(),
            "claude-opus-4-8".into(),
            "ANTHROPIC_API_KEY".into(),
        )
    }

    /// 手滑连按两下回车不该把稿子发出去。
    #[test]
    fn defaults_to_no() {
        assert!(!consent().is_yes());
    }

    #[test]
    fn toggles() {
        let mut c = consent();
        c.toggle();
        assert!(c.is_yes());
        c.toggle();
        assert!(!c.is_yes());
    }

    /// 不能只说「第三方服务」——用户有权看到具体发去哪。
    #[test]
    fn spells_out_where_the_text_goes() {
        let text = consent().lines().join("\n");
        assert!(text.contains("api.anthropic.com"), "要写明地址：{text}");
        assert!(text.contains("claude-opus-4-8"), "要写明模型：{text}");
        assert!(text.contains("ANTHROPIC_API_KEY"), "要写明密钥来源：{text}");
        assert!(text.contains("不会自动扫描全书"), "要说清发送范围：{text}");
        assert!(text.contains("enabled"), "要告诉用户怎么关掉：{text}");
    }
}
