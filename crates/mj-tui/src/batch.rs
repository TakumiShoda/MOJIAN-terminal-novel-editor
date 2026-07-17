//! 批量作业：全卷 / 全书的排版与替换。见 doc.md §6.5、§6.6、§7.4。
//!
//! 三条硬要求：
//! - `[MUST]` 显示进度条且**可中断**；中断时已完成的章保留（§6.5）；
//! - `[MUST]` 执行前每章各打一条快照（§6.6）；
//! - `[MUST]` 提供「撤销本次批量替换」——一次性回滚所有受影响章节到快照（§6.6）。
//!
//! **每章独立事务**：快照 → 改 → 存，一章一轮。中途中断（或某章出错），
//! 已经做完的那些章原样保留——它们各自都是完整的。

use mj_core::history::SnapshotId;
use mj_core::id::ChapterId;

/// 作业范围（§6.5、§6.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Chapter,
    Volume,
    Book,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chapter => "当前章",
            Self::Volume => "当前卷",
            Self::Book => "全书",
        }
    }

    /// F4 循环切换。
    pub fn next(self) -> Self {
        match self {
            Self::Chapter => Self::Volume,
            Self::Volume => Self::Book,
            Self::Book => Self::Chapter,
        }
    }

    /// 是否会动到当前章以外——这种范围值得多问一句。
    pub fn is_wide(self) -> bool {
        !matches!(self, Self::Chapter)
    }
}

/// 作业类型。
#[derive(Debug, Clone, PartialEq)]
pub enum BatchKind {
    Format(mj_text::format::FormatOptions),
    Replace {
        query: mj_text::search::Query,
        to: String,
    },
}

impl BatchKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Format(_) => "排版",
            Self::Replace { .. } => "替换",
        }
    }

    pub fn trigger(&self) -> mj_core::history::Trigger {
        match self {
            Self::Format(_) => mj_core::history::Trigger::BeforeFormat,
            Self::Replace { .. } => mj_core::history::Trigger::BeforeReplace,
        }
    }
}

/// 一个正在跑的批量作业。
#[derive(Debug)]
pub struct BatchJob {
    pub kind: BatchKind,
    pub scope: Scope,
    /// 待处理的章。从尾部弹出，故按倒序压入。
    queue: Vec<ChapterId>,
    total: usize,
    /// 已改动的章 + 改动前的快照 —— 「撤销本次批量」要靠它（§6.6）。
    touched: Vec<(ChapterId, SnapshotId)>,
    /// 扫过但没有改动的章数。
    unchanged: usize,
    /// 出错跳过的章。**不是致命错**：一章坏了不该让整本书的操作前功尽弃。
    failed: Vec<(ChapterId, String)>,
    cancelled: bool,
}

impl BatchJob {
    pub fn new(kind: BatchKind, scope: Scope, mut chapters: Vec<ChapterId>) -> Self {
        let total = chapters.len();
        chapters.reverse(); // 从尾部弹出 = 按原顺序处理
        Self {
            kind,
            scope,
            queue: chapters,
            total,
            touched: Vec::new(),
            unchanged: 0,
            failed: Vec::new(),
            cancelled: false,
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn done(&self) -> usize {
        self.total - self.queue.len()
    }

    pub fn changed_count(&self) -> usize {
        self.touched.len()
    }

    pub fn failed(&self) -> &[(ChapterId, String)] {
        &self.failed
    }

    pub fn touched(&self) -> &[(ChapterId, SnapshotId)] {
        &self.touched
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn is_done(&self) -> bool {
        self.queue.is_empty() || self.cancelled
    }

    /// Esc 取消。已完成的章保留（§6.5）。
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn next_chapter(&mut self) -> Option<ChapterId> {
        if self.cancelled {
            return None;
        }
        self.queue.pop()
    }

    pub fn record_change(&mut self, ch: ChapterId, before: SnapshotId) {
        self.touched.push((ch, before));
    }

    pub fn record_unchanged(&mut self) {
        self.unchanged += 1;
    }

    pub fn record_failure(&mut self, ch: ChapterId, why: String) {
        self.failed.push((ch, why));
    }

    /// 进度（0..1）。
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.done() as f64 / self.total as f64
    }

    /// 进度条文本。
    pub fn progress_line(&self, width: usize) -> String {
        let filled = ((self.progress() * width as f64) as usize).min(width);
        format!(
            "[{}{}] {}/{} 章",
            "█".repeat(filled),
            "░".repeat(width - filled),
            self.done(),
            self.total
        )
    }

    /// 收工时的一句话。
    pub fn summary(&self) -> String {
        let verb = self.kind.label();
        let mut s = if self.cancelled {
            format!("已中断：{verb}了 {} 章（已完成的保留）", self.touched.len())
        } else {
            format!("{verb}完成：改动 {} 章", self.touched.len())
        };
        if self.unchanged > 0 {
            s.push_str(&format!("，{} 章无需改动", self.unchanged));
        }
        if !self.failed.is_empty() {
            s.push_str(&format!("，{} 章出错已跳过", self.failed.len()));
        }
        if !self.touched.is_empty() {
            s.push_str("。Alt+U 可整体撤销");
        }
        s
    }
}

/// 一次做完的批量操作，供「撤销本次批量替换」（§6.6 `[MUST]`）。
///
/// 只记在内存里：§6.6 说的是「撤销**本次**」，即当前这一趟操作。
/// 关掉程序之后，回退的入口是历史面板（每章都有快照），
/// 那才是跨会话的后路——它已经在了。
#[derive(Debug, Clone)]
pub struct BatchUndo {
    pub kind_label: &'static str,
    pub scope: Scope,
    /// 每章 + 操作前的快照。
    pub entries: Vec<(ChapterId, SnapshotId)>,
}

impl BatchUndo {
    pub fn from_job(job: &BatchJob) -> Option<Self> {
        if job.touched.is_empty() {
            return None;
        }
        Some(Self {
            kind_label: job.kind.label(),
            scope: job.scope,
            entries: job.touched.clone(),
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "撤销本次{}（{}，{} 章）",
            self.kind_label,
            self.scope.label(),
            self.entries.len()
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn job(n: usize) -> BatchJob {
        let chapters: Vec<ChapterId> = (0..n).map(|_| ChapterId::generate()).collect();
        BatchJob::new(
            BatchKind::Format(mj_text::format::FormatOptions::default()),
            Scope::Book,
            chapters,
        )
    }

    #[test]
    fn processes_chapters_in_order() {
        let ids: Vec<ChapterId> = (0..3).map(|_| ChapterId::generate()).collect();
        let mut j = BatchJob::new(
            BatchKind::Format(Default::default()),
            Scope::Book,
            ids.clone(),
        );
        for want in &ids {
            assert_eq!(j.next_chapter().as_ref(), Some(want), "应按原顺序处理");
        }
        assert!(j.next_chapter().is_none());
        assert!(j.is_done());
    }

    #[test]
    fn progress_advances() {
        let mut j = job(4);
        assert_eq!(j.done(), 0);
        assert_eq!(j.progress(), 0.0);
        j.next_chapter();
        j.next_chapter();
        assert_eq!(j.done(), 2);
        assert_eq!(j.progress(), 0.5);
    }

    /// §6.5：中断时已完成的章保留。
    #[test]
    fn cancel_stops_dispatch_but_keeps_done_work() {
        let mut j = job(10);
        let a = j.next_chapter().unwrap();
        j.record_change(a, SnapshotId::of("旧"));
        let b = j.next_chapter().unwrap();
        j.record_change(b, SnapshotId::of("旧2"));

        j.cancel();
        assert!(j.next_chapter().is_none(), "取消后不再派发");
        assert!(j.is_done());
        assert_eq!(j.changed_count(), 2, "已完成的两章必须保留");
        assert!(j.summary().contains("已中断"), "{}", j.summary());
        assert!(j.summary().contains("保留"), "{}", j.summary());
    }

    #[test]
    fn empty_job_is_immediately_done() {
        let j = job(0);
        assert!(j.is_done());
        assert_eq!(j.progress(), 1.0, "空作业算 100%，别显示成 0%");
    }

    /// 一章出错不该让整本书的操作前功尽弃。
    #[test]
    fn failures_are_recorded_not_fatal() {
        let mut j = job(3);
        let a = j.next_chapter().unwrap();
        j.record_failure(a, "front matter 损坏".into());
        let b = j.next_chapter().unwrap();
        j.record_change(b, SnapshotId::of("x"));

        assert_eq!(j.failed().len(), 1);
        assert_eq!(j.changed_count(), 1, "别的章照常处理");
        assert!(j.summary().contains("出错已跳过"), "{}", j.summary());
    }

    #[test]
    fn summary_mentions_unchanged_chapters() {
        let mut j = job(2);
        j.next_chapter();
        j.record_unchanged();
        j.next_chapter();
        j.record_unchanged();
        assert!(j.summary().contains("无需改动"), "{}", j.summary());
    }

    #[test]
    fn progress_bar_is_proportional() {
        let mut j = job(4);
        assert_eq!(j.progress_line(4), "[░░░░] 0/4 章");
        j.next_chapter();
        j.next_chapter();
        assert_eq!(j.progress_line(4), "[██░░] 2/4 章");
        j.next_chapter();
        j.next_chapter();
        assert_eq!(j.progress_line(4), "[████] 4/4 章");
    }

    // ---- 批量撤销（§6.6 [MUST]）----

    #[test]
    fn undo_record_captures_touched_chapters() {
        let mut j = job(3);
        let a = j.next_chapter().unwrap();
        j.record_change(a, SnapshotId::of("旧A"));
        let b = j.next_chapter().unwrap();
        j.record_change(b, SnapshotId::of("旧B"));

        let u = BatchUndo::from_job(&j).unwrap();
        assert_eq!(u.entries.len(), 2);
        assert_eq!(u.entries[0].0, a);
        assert_eq!(u.entries[0].1, SnapshotId::of("旧A"));
        assert!(u.describe().contains("2 章"), "{}", u.describe());
    }

    /// 没改动任何章就不该留下「可撤销」的记录——那会让用户以为改了什么。
    #[test]
    fn no_undo_record_when_nothing_changed() {
        let mut j = job(2);
        j.next_chapter();
        j.record_unchanged();
        assert!(BatchUndo::from_job(&j).is_none());
    }

    /// 被中断的作业同样可以撤销——已完成的那部分。
    #[test]
    fn cancelled_job_is_still_undoable() {
        let mut j = job(10);
        let a = j.next_chapter().unwrap();
        j.record_change(a, SnapshotId::of("旧"));
        j.cancel();

        let u = BatchUndo::from_job(&j).unwrap();
        assert_eq!(u.entries.len(), 1, "中断前改的那章也该能退回去");
    }

    #[test]
    fn scope_cycles_and_flags_wide_scopes() {
        assert_eq!(Scope::Chapter.next(), Scope::Volume);
        assert_eq!(Scope::Volume.next(), Scope::Book);
        assert_eq!(Scope::Book.next(), Scope::Chapter);

        assert!(!Scope::Chapter.is_wide());
        assert!(Scope::Volume.is_wide(), "动到当前章以外就该多问一句");
        assert!(Scope::Book.is_wide());
    }
}
