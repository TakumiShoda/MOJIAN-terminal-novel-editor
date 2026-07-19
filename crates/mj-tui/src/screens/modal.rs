//! 浮层栈。见 doc.md §7.1。
//!
//! `[MUST]` 用显式的 `Vec<Modal>` 管理浮层，不要用一堆 bool 标志位——
//! 后者在第三个浮层出现时必然失控。这话是对的：M1–M5 期间这里长到了九个
//! `Option<面板>` 字段，外加一条**手写死的优先级链**（先查 confirm、再 diff、
//! 再 history……）。那条链就是隐式 z 序，加一个浮层就得记得插对位置。
//!
//! 换成栈之后：
//! - **谁在最上面谁吃键**，不必再维护优先级链；
//! - **Esc 逐层弹出**是 `pop()`，语义天然一致；
//! - 「确认框叠在查找面板上」这种真·两层场景不再是特例。
//!
//! 不进栈的两个：`batch`（正在跑的批量作业，不是用户可关的浮层，Esc 是「中断」
//! 不是「关闭」）与 `completion`（正文里的内联补全，键仍然打进缓冲，不夺焦点）。
//! §7.1 列的浮层里也没有它们。

use super::{
    CharacterForm, CharacterPanel, CommandPalette, Confirm, DiffView, FormatPreview, Help,
    HistoryPanel, ProofPanel, SearchPanel, Settings, Stats,
};

/// 一层浮层。
///
/// 各面板体量差得远（`Stats` 只有一个滚动位，`CharacterPanel` 拎着整册角色），
/// 直接内联会让每个栈元素都按最大的那个算。一律装箱：栈元素统一是一个指针，
/// 而浮层本就开得少、开一次多一次分配可以忽略。
pub enum Modal {
    Stats(Box<Stats>),
    FormatPreview(Box<FormatPreview>),
    Search(Box<SearchPanel>),
    History(Box<HistoryPanel>),
    Diff(Box<DiffView>),
    Confirm(Box<Confirm>),
    /// 「正文将发送到第三方服务」的同意框（§6.8 [MUST]）。
    Consent(Box<super::Consent>),
    Proof(Box<ProofPanel>),
    Characters(Box<CharacterPanel>),
    CharacterForm(Box<CharacterForm>),
    /// 命令面板（Ctrl+P，§7.3「最重要的一条」）。
    Palette(Box<CommandPalette>),
    /// 帮助页（F1）。
    Help(Box<Help>),
    /// 外观设置（§6.10）。
    Settings(Box<Settings>),
}

/// 浮层种类，供日志/测试断言「现在栈上是什么」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    Stats,
    FormatPreview,
    Search,
    History,
    Diff,
    Confirm,
    Consent,
    Proof,
    Characters,
    CharacterForm,
    Palette,
    Help,
    Settings,
}

impl Modal {
    /// 是否铺满正文区。
    ///
    /// 铺满的那些在绘制前要先 `Clear`——否则底下的正文会从空白处透上来
    /// （浮层栈之前每层都是「替换整个正文区」，不存在这个问题；改成分层叠画后
    /// 就有了）。确认框是居中小窗，自己会 Clear，不算铺满。
    pub fn is_fullscreen(&self) -> bool {
        // 确认框与命令面板是居中小窗，自己会 Clear，压在正文上正是它们该有的样子。
        !matches!(self, Self::Confirm(_) | Self::Palette(_))
    }

    pub fn kind(&self) -> ModalKind {
        match self {
            Self::Stats(_) => ModalKind::Stats,
            Self::FormatPreview(_) => ModalKind::FormatPreview,
            Self::Search(_) => ModalKind::Search,
            Self::History(_) => ModalKind::History,
            Self::Diff(_) => ModalKind::Diff,
            Self::Confirm(_) => ModalKind::Confirm,
            Self::Consent(_) => ModalKind::Consent,
            Self::Proof(_) => ModalKind::Proof,
            Self::Characters(_) => ModalKind::Characters,
            Self::CharacterForm(_) => ModalKind::CharacterForm,
            Self::Palette(_) => ModalKind::Palette,
            Self::Help(_) => ModalKind::Help,
            Self::Settings(_) => ModalKind::Settings,
        }
    }
}

/// 浮层栈。压栈打开、弹栈关闭；最上面那层吃键。
#[derive(Default)]
pub struct ModalStack {
    layers: Vec<Modal>,
}

/// 生成「取栈顶起最近的某类浮层」的存取器。
///
/// 取「最近的一个」而非严格栈顶：确认框叠在查找面板上时，确认框的处理逻辑
/// 仍需读到底下那层查找面板的状态。
macro_rules! accessor {
    ($get:ident, $get_mut:ident, $variant:ident, $ty:ty) => {
        pub fn $get(&self) -> Option<&$ty> {
            self.layers.iter().rev().find_map(|m| match m {
                Modal::$variant(x) => Some(x.as_ref()),
                _ => None,
            })
        }

        pub fn $get_mut(&mut self) -> Option<&mut $ty> {
            self.layers.iter_mut().rev().find_map(|m| match m {
                Modal::$variant(x) => Some(x.as_mut()),
                _ => None,
            })
        }
    };
}

impl ModalStack {
    pub fn push(&mut self, m: Modal) {
        self.layers.push(m);
    }

    /// Esc 逐层弹出（§7.1）。
    pub fn pop(&mut self) -> Option<Modal> {
        self.layers.pop()
    }

    /// 关掉栈里最近的某一类（不一定在栈顶）。
    pub fn close_kind(&mut self, kind: ModalKind) -> Option<Modal> {
        let pos = self.layers.iter().rposition(|m| m.kind() == kind)?;
        Some(self.layers.remove(pos))
    }

    pub fn clear(&mut self) {
        self.layers.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn top(&self) -> Option<&Modal> {
        self.layers.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut Modal> {
        self.layers.last_mut()
    }

    pub fn top_kind(&self) -> Option<ModalKind> {
        self.top().map(|m| m.kind())
    }

    /// 栈顶是否为某类——键分发用。
    pub fn top_is(&self, kind: ModalKind) -> bool {
        self.top_kind() == Some(kind)
    }

    /// 栈里是否有某类（不论层次）。
    pub fn contains(&self, kind: ModalKind) -> bool {
        self.layers.iter().any(|m| m.kind() == kind)
    }

    /// 自底向上遍历，供渲染分层绘制。
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Modal> {
        self.layers.iter_mut()
    }

    /// 自底向上的种类列表，供日志与测试断言。
    pub fn kinds(&self) -> Vec<ModalKind> {
        self.layers.iter().map(|m| m.kind()).collect()
    }

    /// 取走确认框（关闭并拿到它）——确认后要用里面记的作业参数。
    pub fn take_confirm(&mut self) -> Option<Confirm> {
        match self.close_kind(ModalKind::Confirm)? {
            Modal::Confirm(c) => Some(*c),
            _ => None,
        }
    }

    /// 取走同意框——同意后要落盘 consented 并接着跑那趟校对。
    pub fn take_consent(&mut self) -> Option<super::Consent> {
        match self.close_kind(ModalKind::Consent)? {
            Modal::Consent(c) => Some(*c),
            _ => None,
        }
    }

    /// 取走排版预览——应用时要消费里面的编辑列表。
    pub fn take_format_preview(&mut self) -> Option<FormatPreview> {
        match self.close_kind(ModalKind::FormatPreview)? {
            Modal::FormatPreview(p) => Some(*p),
            _ => None,
        }
    }

    accessor!(stats, stats_mut, Stats, Stats);
    accessor!(
        format_preview,
        format_preview_mut,
        FormatPreview,
        FormatPreview
    );
    accessor!(search, search_mut, Search, SearchPanel);
    accessor!(history, history_mut, History, HistoryPanel);
    accessor!(diff, diff_mut, Diff, DiffView);
    accessor!(confirm, confirm_mut, Confirm, Confirm);
    accessor!(proof, proof_mut, Proof, ProofPanel);
    accessor!(characters, characters_mut, Characters, CharacterPanel);
    accessor!(
        character_form,
        character_form_mut,
        CharacterForm,
        CharacterForm
    );
    accessor!(palette, palette_mut, Palette, CommandPalette);
    accessor!(help, help_mut, Help, Help);
    accessor!(settings, settings_mut, Settings, Settings);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn search() -> Modal {
        Modal::Search(Box::new(SearchPanel::new(false)))
    }

    fn confirm() -> Modal {
        Modal::Confirm(Box::new(Confirm::new(
            crate::batch::BatchKind::Format(Default::default()),
            crate::batch::Scope::Book,
            10,
        )))
    }

    #[test]
    fn empty_stack_has_no_top() {
        let s = ModalStack::default();
        assert!(s.is_empty());
        assert!(s.top().is_none());
        assert!(s.top_kind().is_none());
    }

    #[test]
    fn top_is_the_last_pushed() {
        let mut s = ModalStack::default();
        s.push(search());
        assert!(s.top_is(ModalKind::Search));
        s.push(confirm());
        assert!(s.top_is(ModalKind::Confirm), "确认框叠上来后由它吃键");
        assert_eq!(s.len(), 2);
    }

    /// Esc 逐层弹出（§7.1）：关掉确认框应露出底下的查找面板。
    #[test]
    fn pop_reveals_the_layer_below() {
        let mut s = ModalStack::default();
        s.push(search());
        s.push(confirm());
        s.pop();
        assert!(s.top_is(ModalKind::Search), "弹掉确认框后回到查找面板");
        s.pop();
        assert!(s.is_empty());
    }

    /// 叠着的时候仍能读到下层面板的状态。
    #[test]
    fn accessor_finds_layer_below_the_top() {
        let mut s = ModalStack::default();
        s.push(search());
        s.push(confirm());
        assert!(s.search().is_some(), "确认框之下的查找面板仍可读");
        assert!(s.confirm().is_some());
    }

    #[test]
    fn close_kind_removes_from_middle() {
        let mut s = ModalStack::default();
        s.push(search());
        s.push(confirm());
        s.close_kind(ModalKind::Search);
        assert_eq!(s.len(), 1);
        assert!(s.top_is(ModalKind::Confirm), "上层不受影响");
        assert!(s.search().is_none());
    }

    #[test]
    fn contains_checks_whole_stack() {
        let mut s = ModalStack::default();
        s.push(search());
        s.push(confirm());
        assert!(s.contains(ModalKind::Search));
        assert!(!s.contains(ModalKind::Diff));
    }

    #[test]
    fn clear_drops_everything() {
        let mut s = ModalStack::default();
        s.push(search());
        s.push(confirm());
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn pop_on_empty_is_safe() {
        let mut s = ModalStack::default();
        assert!(s.pop().is_none());
    }
}
