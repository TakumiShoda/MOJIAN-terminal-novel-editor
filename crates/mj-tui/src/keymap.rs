//! 键位表与重绑定。见 doc.md §7.3。
//!
//! `[MUST]` 键位全部可在 `[keymap]` 里重绑定；
//! `[MUST]` 冲突检测——启动时校验，冲突则报警并用默认值。
//!
//! 默认键位**不在这里另存一份**，而是从 `commands::COMMANDS` 的 `keys` 字段解析出来。
//! 那张表已经是命令面板与帮助页的唯一真相，键位表再抄一遍就又多一处会分叉的地方。
//!
//! 配置形如：
//! ```toml
//! [keymap]
//! proof = "F6"          # 把校对从 F7 挪到 F6
//! toggle_tree = "Alt+T"
//! ```
//! 键名是命令的稳定 id（`Command::id()`）。

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::commands::{COMMANDS, Command};

/// 一个键位组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Binding {
    /// 解析 `"Ctrl+S"` / `"F7"` / `"Alt+C"` / `"Ctrl+Shift+S"`。大小写不敏感。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let mut last = s;
        // 逐个剥前缀修饰键。
        while let Some((head, rest)) = last.split_once('+') {
            match head.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "option" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                // 不认识的修饰键：整条作废，别猜。
                _ => return None,
            }
            last = rest.trim();
        }

        let code = parse_code(last)?;
        // `Ctrl+Shift+S` 这类：终端发的是大写字母，统一按大写记，
        // 好与 crossterm 实际给的事件对上。
        let code = match (code, mods.contains(KeyModifiers::SHIFT)) {
            (KeyCode::Char(c), true) => KeyCode::Char(c.to_ascii_uppercase()),
            (other, _) => other,
        };
        Some(Self { code, mods })
    }

    /// 回显成人能读的形式，供帮助页与报错。
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => s.push_str("Space"),
            KeyCode::Char(c) => s.push(c.to_ascii_uppercase()),
            KeyCode::F(n) => s.push_str(&format!("F{n}")),
            KeyCode::Esc => s.push_str("Esc"),
            KeyCode::Tab => s.push_str("Tab"),
            KeyCode::Enter => s.push_str("Enter"),
            other => s.push_str(&format!("{other:?}")),
        }
        s
    }

    /// 事件是否命中本键位。
    ///
    /// 只比对我们关心的三个修饰键——终端还会带 KEYPAD 之类的杂位，
    /// 整体相等比较会漏掉本该命中的按键。
    pub fn matches(&self, code: KeyCode, mods: KeyModifiers) -> bool {
        const CARE: KeyModifiers = KeyModifiers::CONTROL
            .union(KeyModifiers::ALT)
            .union(KeyModifiers::SHIFT);
        self.code == code && (mods & CARE) == (self.mods & CARE)
    }
}

fn parse_code(s: &str) -> Option<KeyCode> {
    let lower = s.to_ascii_lowercase();
    // 功能键 F1..F24
    if let Some(n) = lower.strip_prefix('f')
        && let Ok(n) = n.parse::<u8>()
        && (1..=24).contains(&n)
    {
        return Some(KeyCode::F(n));
    }
    Some(match lower.as_str() {
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        _ => {
            let mut it = s.chars();
            let c = it.next()?;
            if it.next().is_some() {
                return None; // 多字符且不是已知名字
            }
            // 单字母一律存小写；SHIFT 的处理在 parse 里统一做。
            KeyCode::Char(c.to_ascii_lowercase())
        }
    })
}

/// 配置有问题的地方。启动时报给用户——`[MUST]` 冲突要「报警」。
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// `[keymap]` 里写了不认识的命令 id。
    UnknownCommand(String),
    /// 键位串解析不了。
    BadBinding { id: String, value: String },
    /// 两条命令抢同一个键。
    Conflict {
        binding: String,
        first: String,
        second: String,
    },
    /// 用户占用了某命令的默认键，导致那条命令没键可用（仍可从命令面板触达）。
    DefaultShadowed { id: String, binding: String },
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownCommand(id) => format!("[keymap] 里有不认识的命令「{id}」，已忽略"),
            Self::BadBinding { id, value } => {
                format!("[keymap] {id} = \"{value}\" 解析不了，已用默认键位")
            }
            Self::Conflict {
                binding,
                first,
                second,
            } => format!("键位 {binding} 被「{first}」和「{second}」同时占用，两者都退回默认键位"),
            Self::DefaultShadowed { id, binding } => {
                format!("「{id}」的默认键位 {binding} 已被别的命令占用，只能从命令面板触达")
            }
        }
    }
}

/// 键位表。
#[derive(Debug, Clone)]
pub struct Keymap {
    entries: Vec<(Binding, Command)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Keymap {
    /// 默认键位：从命令表的 `keys` 字段解析。
    pub fn defaults() -> Self {
        let mut entries = Vec::new();
        for spec in COMMANDS {
            if !spec.cmd.has_global_key() {
                continue;
            }
            if let Some(b) = Binding::parse(spec.keys) {
                entries.push((b, spec.cmd));
            }
        }
        Self { entries }
    }

    /// 按用户配置构建。返回键位表与发现的问题。
    ///
    /// 冲突策略（§7.3「冲突则报警并用默认值」）：
    /// 1. 先收用户绑定，解析不了的报 `BadBinding` 并丢弃；
    /// 2. **用户绑定之间**若撞车，两条都丢弃、退回默认——只留一条是随机的，
    ///    而随机比全退更难排查；
    /// 3. 再补默认键位，但默认键若已被用户占用则跳过（报 `DefaultShadowed`）。
    ///
    /// 先收齐用户绑定、再补其余默认，是为了让「把 A 挪到 B 的老位置、同时把 B 挪走」
    /// 这种成对调换能成功，且与配置里的书写顺序无关。
    pub fn from_config(table: &toml::Table) -> (Self, Vec<Problem>) {
        let mut problems = Vec::new();
        let mut user: Vec<(Binding, Command)> = Vec::new();

        for (key, value) in table {
            let Some(cmd) = Command::from_id(key) else {
                problems.push(Problem::UnknownCommand(key.clone()));
                continue;
            };
            let Some(text) = value.as_str() else {
                problems.push(Problem::BadBinding {
                    id: key.clone(),
                    value: value.to_string(),
                });
                continue;
            };
            match Binding::parse(text) {
                Some(b) => user.push((b, cmd)),
                None => problems.push(Problem::BadBinding {
                    id: key.clone(),
                    value: text.to_string(),
                }),
            }
        }

        // 用户绑定内部撞车：涉事的全部丢弃。
        let mut rejected: Vec<Command> = Vec::new();
        for i in 0..user.len() {
            for j in (i + 1)..user.len() {
                if user[i].0 == user[j].0 {
                    problems.push(Problem::Conflict {
                        binding: user[i].0.display(),
                        first: user[i].1.id().to_string(),
                        second: user[j].1.id().to_string(),
                    });
                    rejected.push(user[i].1);
                    rejected.push(user[j].1);
                }
            }
        }
        user.retain(|(_, c)| !rejected.contains(c));

        let mut entries = user;
        // 补默认：跳过已被用户绑定的命令，以及键位已被占用的情况。
        for spec in COMMANDS {
            if !spec.cmd.has_global_key() {
                continue;
            }
            if entries.iter().any(|(_, c)| *c == spec.cmd) {
                continue;
            }
            let Some(b) = Binding::parse(spec.keys) else {
                continue;
            };
            if entries.iter().any(|(eb, _)| *eb == b) {
                problems.push(Problem::DefaultShadowed {
                    id: spec.cmd.id().to_string(),
                    binding: b.display(),
                });
                continue;
            }
            entries.push((b, spec.cmd));
        }

        (Self { entries }, problems)
    }

    /// 查按键对应的命令。
    pub fn lookup(&self, code: KeyCode, mods: KeyModifiers) -> Option<Command> {
        self.entries
            .iter()
            .find(|(b, _)| b.matches(code, mods))
            .map(|(_, c)| *c)
    }

    /// 某命令当前绑的键（供帮助页显示真实键位而非表里的默认值）。
    pub fn binding_of(&self, cmd: Command) -> Option<Binding> {
        self.entries
            .iter()
            .find(|(_, c)| *c == cmd)
            .map(|(b, _)| *b)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn table(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    // ---- 解析 ----

    #[test]
    fn parses_plain_and_modified_keys() {
        assert_eq!(
            Binding::parse("F7").unwrap(),
            Binding {
                code: KeyCode::F(7),
                mods: KeyModifiers::NONE
            }
        );
        assert_eq!(
            Binding::parse("Ctrl+S").unwrap(),
            Binding {
                code: KeyCode::Char('s'),
                mods: KeyModifiers::CONTROL
            }
        );
        assert_eq!(
            Binding::parse("Alt+C").unwrap(),
            Binding {
                code: KeyCode::Char('c'),
                mods: KeyModifiers::ALT
            }
        );
    }

    #[test]
    fn parsing_is_case_insensitive() {
        assert_eq!(Binding::parse("ctrl+s"), Binding::parse("Ctrl+S"));
        assert_eq!(Binding::parse("f7"), Binding::parse("F7"));
    }

    /// Ctrl+Shift+S 的字母要按大写记——终端发的就是大写。
    #[test]
    fn shift_letter_becomes_uppercase() {
        let b = Binding::parse("Ctrl+Shift+S").unwrap();
        assert_eq!(b.code, KeyCode::Char('S'));
        assert!(b.mods.contains(KeyModifiers::CONTROL));
        assert!(b.mods.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(Binding::parse("").is_none());
        assert!(
            Binding::parse("Hyper+X").is_none(),
            "不认识的修饰键要整条作废"
        );
        assert!(Binding::parse("F99").is_none());
        assert!(Binding::parse("NotAKey").is_none());
    }

    #[test]
    fn display_roundtrips() {
        for s in ["F7", "Ctrl+S", "Alt+C", "Esc"] {
            let b = Binding::parse(s).unwrap();
            assert_eq!(
                Binding::parse(&b.display()).unwrap(),
                b,
                "{s} 回显后应能再解析"
            );
        }
    }

    /// 终端会带上我们不关心的修饰位，不能整体相等比较。
    #[test]
    fn matching_ignores_irrelevant_modifiers() {
        let b = Binding::parse("Ctrl+S").unwrap();
        assert!(b.matches(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(
            b.matches(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL | KeyModifiers::SUPER
            ),
            "多出的 SUPER 位不该让匹配失败"
        );
        assert!(!b.matches(KeyCode::Char('s'), KeyModifiers::NONE));
    }

    // ---- 默认表 ----

    #[test]
    fn defaults_come_from_the_command_table() {
        let k = Keymap::defaults();
        assert_eq!(
            k.lookup(KeyCode::F(7), KeyModifiers::NONE),
            Some(Command::Proof)
        );
        assert_eq!(
            k.lookup(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Some(Command::Save)
        );
    }

    /// Esc 是上下文相关的，不该占全局键位。
    #[test]
    fn esc_is_not_a_global_binding() {
        let k = Keymap::defaults();
        assert_eq!(k.lookup(KeyCode::Esc, KeyModifiers::NONE), None);
    }

    /// 默认表自身不得有冲突——命令表里两条写了同一个键的话，这里会发现。
    #[test]
    fn defaults_have_no_conflicts() {
        let (_, problems) = Keymap::from_config(&toml::Table::new());
        let conflicts: Vec<_> = problems
            .iter()
            .filter(|p| {
                matches!(
                    p,
                    Problem::Conflict { .. } | Problem::DefaultShadowed { .. }
                )
            })
            .collect();
        assert!(conflicts.is_empty(), "默认键位表内部有冲突：{conflicts:?}");
    }

    // ---- 重绑定 ----

    #[test]
    fn user_binding_overrides_default() {
        let (k, problems) = Keymap::from_config(&table("proof = \"F6\""));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            k.lookup(KeyCode::F(6), KeyModifiers::NONE),
            Some(Command::Proof)
        );
        assert_eq!(
            k.lookup(KeyCode::F(7), KeyModifiers::NONE),
            None,
            "老键位应让出"
        );
    }

    /// 成对调换：把校对挪到排版的老位置、同时把排版挪走，应当都成功，
    /// 且与配置里的书写顺序无关。
    #[test]
    fn swapping_two_bindings_works_regardless_of_order() {
        for src in [
            "proof = \"F5\"\nformat = \"F7\"",
            "format = \"F7\"\nproof = \"F5\"",
        ] {
            let (k, problems) = Keymap::from_config(&table(src));
            assert!(problems.is_empty(), "{src} → {problems:?}");
            assert_eq!(
                k.lookup(KeyCode::F(5), KeyModifiers::NONE),
                Some(Command::Proof)
            );
            assert_eq!(
                k.lookup(KeyCode::F(7), KeyModifiers::NONE),
                Some(Command::Format)
            );
        }
    }

    /// §7.3 [MUST]：冲突要报警，并退回默认值。
    #[test]
    fn conflicting_user_bindings_are_reported_and_reverted() {
        let (k, problems) = Keymap::from_config(&table("proof = \"F6\"\nhistory = \"F6\""));
        let conflict = problems
            .iter()
            .find(|p| matches!(p, Problem::Conflict { .. }))
            .expect("该报冲突");
        assert!(conflict.message().contains("F6"), "{}", conflict.message());

        // 两条都退回默认。
        assert_eq!(
            k.lookup(KeyCode::F(7), KeyModifiers::NONE),
            Some(Command::Proof)
        );
        assert_eq!(
            k.lookup(KeyCode::F(8), KeyModifiers::NONE),
            Some(Command::History)
        );
        assert_eq!(k.lookup(KeyCode::F(6), KeyModifiers::NONE), None);
    }

    #[test]
    fn unknown_command_is_reported_not_fatal() {
        // 键要加引号——TOML 的裸键不收中文。
        let (k, problems) = Keymap::from_config(&table("\"查无此命令\" = \"F6\"\nproof = \"F6\""));
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::UnknownCommand(_)))
        );
        // 其余配置照常生效。
        assert_eq!(
            k.lookup(KeyCode::F(6), KeyModifiers::NONE),
            Some(Command::Proof)
        );
    }

    #[test]
    fn bad_binding_falls_back_to_default() {
        let (k, problems) = Keymap::from_config(&table("proof = \"Hyper+Q\""));
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::BadBinding { .. }))
        );
        assert_eq!(
            k.lookup(KeyCode::F(7), KeyModifiers::NONE),
            Some(Command::Proof),
            "解析不了就用默认"
        );
    }

    /// 用户占了别人的默认键：那条命令没键可用，但要报出来，且仍可从命令面板触达。
    #[test]
    fn shadowed_default_is_reported() {
        let (k, problems) = Keymap::from_config(&table("stats = \"F7\""));
        let p = problems
            .iter()
            .find(|p| matches!(p, Problem::DefaultShadowed { .. }))
            .expect("该报默认键位被占");
        assert!(p.message().contains("命令面板"), "{}", p.message());
        assert_eq!(
            k.lookup(KeyCode::F(7), KeyModifiers::NONE),
            Some(Command::Stats)
        );
        assert!(k.binding_of(Command::Proof).is_none(), "校对失去了键位");
    }

    #[test]
    fn binding_of_reports_current_not_default() {
        let (k, _) = Keymap::from_config(&table("proof = \"F6\""));
        assert_eq!(k.binding_of(Command::Proof).unwrap().display(), "F6");
    }
}
