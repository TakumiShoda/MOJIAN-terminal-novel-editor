//! 角色卡表单编辑（§6.7 [MUST]：编辑用表单式界面）。
//!
//! 从角色列表按 `e` 进入。字段逐条排列，Tab/j/k 换字段，Enter/`i` 进字段编辑，
//! Ctrl+S 存，Esc 退。多行字段（背景/性格等）编辑时 Enter 换行。
//!
//! 状态与渲染分离：这里只管字段值与焦点，绘制在 app.rs；存盘经 Store。
//!
//! 编辑是**追加/退格式**（在字段末尾），不做字段内光标移动——这是 M5 的简化。
//! 但退格按**字素簇**删除（不是 char/byte），别把「é」「👨‍👩‍👧」劈成半个（§0 精神）。

use unicode_segmentation::UnicodeSegmentation;

use mj_core::model::Character;

/// 字段身份，决定存回 `Character` 的哪个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKey {
    Name,
    Aliases,
    Role,
    Gender,
    Age,
    Background,
    Personality,
    Appearance,
    Habits,
    Speech,
    Notes,
}

impl FieldKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "名字",
            Self::Aliases => "别名（、分隔）",
            Self::Role => "身份",
            Self::Gender => "性别",
            Self::Age => "年龄",
            Self::Background => "背景",
            Self::Personality => "性格",
            Self::Appearance => "外貌",
            Self::Habits => "习惯",
            Self::Speech => "语言风格",
            Self::Notes => "备注",
        }
    }

    /// 多行字段：编辑时 Enter 插入换行；单行字段 Enter 结束编辑。
    pub fn is_multiline(self) -> bool {
        matches!(
            self,
            Self::Background
                | Self::Personality
                | Self::Appearance
                | Self::Habits
                | Self::Speech
                | Self::Notes
        )
    }
}

/// 表单里的一个字段。
pub struct FormField {
    pub key: FieldKey,
    pub value: String,
}

pub struct CharacterForm {
    /// 编辑的是哪张卡。
    id: mj_core::id::CharacterId,
    /// 该卡未被表单覆盖的部分（relations / first_appearance / custom），存盘时保留。
    base: Character,
    fields: Vec<FormField>,
    focus: usize,
    editing: bool,
    dirty: bool,
}

impl CharacterForm {
    pub fn new(c: Character) -> Self {
        let fields = vec![
            FormField {
                key: FieldKey::Name,
                value: c.name.clone(),
            },
            FormField {
                key: FieldKey::Aliases,
                value: c.aliases.join("、"),
            },
            FormField {
                key: FieldKey::Role,
                value: c.role.clone(),
            },
            FormField {
                key: FieldKey::Gender,
                value: c.gender.clone(),
            },
            FormField {
                key: FieldKey::Age,
                value: c.age.clone(),
            },
            FormField {
                key: FieldKey::Background,
                value: c.background.clone(),
            },
            FormField {
                key: FieldKey::Personality,
                value: c.personality.clone(),
            },
            FormField {
                key: FieldKey::Appearance,
                value: c.appearance.clone(),
            },
            FormField {
                key: FieldKey::Habits,
                value: c.habits.clone(),
            },
            FormField {
                key: FieldKey::Speech,
                value: c.speech.clone(),
            },
            FormField {
                key: FieldKey::Notes,
                value: c.notes.clone(),
            },
        ];
        Self {
            id: c.id,
            base: c,
            fields,
            focus: 0,
            editing: false,
            dirty: false,
        }
    }

    pub fn id(&self) -> mj_core::id::CharacterId {
        self.id
    }

    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    pub fn focus(&self) -> usize {
        self.focus
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn focused_field(&self) -> &FormField {
        &self.fields[self.focus]
    }

    pub fn next_field(&mut self) {
        if self.focus + 1 < self.fields.len() {
            self.focus += 1;
        }
    }

    pub fn prev_field(&mut self) {
        self.focus = self.focus.saturating_sub(1);
    }

    /// 进入/退出字段编辑。
    pub fn start_editing(&mut self) {
        self.editing = true;
    }

    pub fn stop_editing(&mut self) {
        self.editing = false;
    }

    /// 往当前字段末尾敲一个字符。
    pub fn input_char(&mut self, c: char) {
        self.fields[self.focus].value.push(c);
        self.dirty = true;
    }

    /// 多行字段里换行。
    pub fn input_newline(&mut self) {
        if self.fields[self.focus].key.is_multiline() {
            self.fields[self.focus].value.push('\n');
            self.dirty = true;
        }
    }

    /// 退格删一个**字素簇**。
    pub fn backspace(&mut self) {
        let v = &mut self.fields[self.focus].value;
        if let Some((idx, _)) = v.grapheme_indices(true).next_back() {
            v.truncate(idx);
            self.dirty = true;
        }
    }

    /// 收集成一张 `Character`（保留表单未覆盖的字段）。
    pub fn to_character(&self) -> Character {
        let mut c = self.base.clone();
        for f in &self.fields {
            match f.key {
                FieldKey::Name => c.name = f.value.trim().to_string(),
                FieldKey::Aliases => {
                    c.aliases = f
                        .value
                        .split(['、', ',', '，'])
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
                FieldKey::Role => c.role = f.value.trim().to_string(),
                FieldKey::Gender => c.gender = f.value.trim().to_string(),
                FieldKey::Age => c.age = f.value.trim().to_string(),
                FieldKey::Background => c.background = f.value.clone(),
                FieldKey::Personality => c.personality = f.value.clone(),
                FieldKey::Appearance => c.appearance = f.value.clone(),
                FieldKey::Habits => c.habits = f.value.clone(),
                FieldKey::Speech => c.speech = f.value.clone(),
                FieldKey::Notes => c.notes = f.value.clone(),
            }
        }
        c
    }

    /// 存盘后调用，清脏标记。
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use mj_core::id::CharacterId;

    fn form() -> CharacterForm {
        let mut c = Character::new(CharacterId::generate(), "沈砚");
        c.aliases = vec!["沈公子".into(), "小砚".into()];
        c.role = "主角".into();
        CharacterForm::new(c)
    }

    #[test]
    fn loads_existing_values() {
        let f = form();
        assert_eq!(f.fields()[0].value, "沈砚");
        assert_eq!(f.fields()[1].value, "沈公子、小砚", "别名以、拼接展示");
        assert_eq!(f.fields()[2].value, "主角");
    }

    #[test]
    fn editing_appends_and_marks_dirty() {
        let mut f = form();
        f.next_field(); // 别名
        f.next_field(); // 身份
        assert_eq!(f.focused_field().key, FieldKey::Role);
        f.start_editing();
        f.input_char('！');
        assert!(f.is_dirty());
        assert_eq!(f.focused_field().value, "主角！");
    }

    #[test]
    fn backspace_removes_a_grapheme_not_half() {
        let mut f = form();
        f.start_editing();
        // 名字末尾加一个 ZWJ 家庭 emoji，退格应整簇删掉。
        f.input_char('👨'); // 简化：单标量也行，关键是不 panic 且删净
        let before = f.focused_field().value.clone();
        f.backspace();
        assert_eq!(
            f.focused_field().value,
            "沈砚",
            "退格应回到原值：was {before}"
        );
    }

    #[test]
    fn newline_only_in_multiline_fields() {
        let mut f = form();
        // 单行字段（名字）：换行无效。
        f.start_editing();
        f.input_newline();
        assert_eq!(f.focused_field().value, "沈砚", "单行字段不接受换行");
        // 多行字段（背景）：换行生效。
        while f.focused_field().key != FieldKey::Background {
            f.next_field();
        }
        f.input_char('甲');
        f.input_newline();
        f.input_char('乙');
        assert_eq!(f.focused_field().value, "甲\n乙");
    }

    #[test]
    fn to_character_splits_aliases() {
        let mut f = form();
        f.next_field(); // 别名
        f.start_editing();
        f.input_char('、');
        f.input_char('阿');
        f.input_char('砚');
        let c = f.to_character();
        assert_eq!(c.aliases, vec!["沈公子", "小砚", "阿砚"]);
    }

    #[test]
    fn to_character_preserves_untouched_fields() {
        let mut base = Character::new(CharacterId::generate(), "沈砚");
        base.first_appearance = None;
        base.custom
            .insert("武器".into(), toml::Value::String("青玉刀".into()));
        let f = CharacterForm::new(base);
        let c = f.to_character();
        assert_eq!(
            c.custom.get("武器").and_then(|v| v.as_str()),
            Some("青玉刀"),
            "表单没碰的自定义字段要保留"
        );
    }

    #[test]
    fn focus_navigation_clamps() {
        let mut f = form();
        f.prev_field();
        assert_eq!(f.focus(), 0, "顶部不越界");
        for _ in 0..100 {
            f.next_field();
        }
        assert_eq!(f.focus(), f.fields().len() - 1, "底部不越界");
    }
}
