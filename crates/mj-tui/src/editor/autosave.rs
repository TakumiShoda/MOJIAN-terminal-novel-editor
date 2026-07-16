//! 自动保存策略。见 doc.md §6.3。
//!
//! `[MUST]` 默认空闲 3 秒**或**累计变更 200 字触发。
//!
//! 策略与执行分离：这里只回答「现在该不该存」，不碰磁盘——
//! 于是可以用假时间完整测试，不必真等 3 秒。

use std::time::{Duration, Instant};

/// 该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 什么都不做。
    Idle,
    /// 只写 swp（正文尚未到保存时机，但要防断电）。
    WriteSwap,
    /// 保存正文（并清理 swp）。
    Save,
}

/// swp 的写入节流。
///
/// 比自动保存频繁得多：自动保存要序列化 front matter + 原子写 + fsync，
/// 每次都干太重；而 swp 只写正文一个文件，代价小。
/// 500ms 意味着断电最多丢半秒的字——而自动保存的窗口是 3 秒。
const SWAP_INTERVAL: Duration = Duration::from_millis(500);

pub struct AutoSave {
    idle: Duration,
    words_threshold: usize,
    /// 上次编辑的时刻。用于判断「空闲了多久」。
    last_edit: Option<Instant>,
    last_swap: Option<Instant>,
}

impl AutoSave {
    pub fn new(idle_ms: u64, words_threshold: usize) -> Self {
        Self {
            idle: Duration::from_millis(idle_ms),
            words_threshold,
            last_edit: None,
            last_swap: None,
        }
    }

    /// 缓冲发生了编辑。
    pub fn on_edit(&mut self, now: Instant) {
        self.last_edit = Some(now);
    }

    /// 正文已保存，重置计时。
    pub fn on_saved(&mut self) {
        self.last_edit = None;
        self.last_swap = None;
    }

    /// 现在该做什么。
    ///
    /// `dirty`/`changed_chars` 来自 `Buffer`。
    pub fn poll(&mut self, now: Instant, dirty: bool, changed_chars: usize) -> Action {
        if !dirty {
            return Action::Idle;
        }
        let Some(last_edit) = self.last_edit else {
            return Action::Idle;
        };

        // 累计变更够多 → 立刻存，不等空闲。
        // 「或」而非「且」：一个下午不停敲的人永远不会空闲 3 秒，
        // 只靠空闲触发的话，他的稿子从头到尾没存过盘。
        if changed_chars >= self.words_threshold {
            return Action::Save;
        }

        // 空闲够久 → 存。
        if now.duration_since(last_edit) >= self.idle {
            return Action::Save;
        }

        // 还在打字：退而求其次写 swp，把断电的损失限制在 SWAP_INTERVAL 内。
        let due = match self.last_swap {
            None => true,
            Some(t) => now.duration_since(t) >= SWAP_INTERVAL,
        };
        if due {
            self.last_swap = Some(now);
            return Action::WriteSwap;
        }
        Action::Idle
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn clean_buffer_does_nothing() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_eq!(
            a.poll(at(t, 5000), false, 0),
            Action::Idle,
            "未改动就不该存"
        );
    }

    #[test]
    fn saves_after_idle_timeout() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_ne!(
            a.poll(at(t, 2999), true, 5),
            Action::Save,
            "未到 3 秒不该存"
        );
        assert_eq!(a.poll(at(t, 3000), true, 5), Action::Save, "满 3 秒应存");
    }

    /// 累计够 200 字立刻存，不等空闲——否则不停敲的人一次都存不上。
    #[test]
    fn saves_on_word_threshold_without_idling() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_eq!(
            a.poll(at(t, 100), true, 200),
            Action::Save,
            "满 200 字应立刻存"
        );
    }

    #[test]
    fn below_threshold_and_still_typing_writes_swap() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_eq!(
            a.poll(at(t, 100), true, 10),
            Action::WriteSwap,
            "打字中应写 swp"
        );
    }

    /// swp 也要节流，否则每个按键都写盘。
    #[test]
    fn swap_is_throttled() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_eq!(a.poll(at(t, 10), true, 1), Action::WriteSwap);
        assert_eq!(a.poll(at(t, 20), true, 2), Action::Idle, "500ms 内不重复写");
        assert_eq!(
            a.poll(at(t, 520), true, 3),
            Action::WriteSwap,
            "过了间隔再写"
        );
    }

    #[test]
    fn saving_resets_the_clock() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        assert_eq!(a.poll(at(t, 3000), true, 5), Action::Save);
        a.on_saved();
        assert_eq!(
            a.poll(at(t, 9000), false, 0),
            Action::Idle,
            "存过之后应安静"
        );
    }

    /// 保存后继续打字，计时应重新开始。
    #[test]
    fn resumes_after_save() {
        let mut a = AutoSave::new(3000, 200);
        let t = Instant::now();
        a.on_edit(t);
        a.poll(at(t, 3000), true, 5);
        a.on_saved();

        a.on_edit(at(t, 4000));
        assert_ne!(a.poll(at(t, 5000), true, 1), Action::Save, "新一轮未到时限");
        assert_eq!(a.poll(at(t, 7000), true, 1), Action::Save, "新一轮满 3 秒");
    }

    /// 配置为 0 时不该退化成每帧狂存。
    #[test]
    fn zero_idle_is_not_pathological() {
        let mut a = AutoSave::new(0, 200);
        let t = Instant::now();
        a.on_edit(t);
        // idle=0 意味着立刻存，这是用户显式配置的，尊重它——
        // 但只在真的有改动时。
        assert_eq!(a.poll(t, true, 1), Action::Save);
        assert_eq!(a.poll(t, false, 0), Action::Idle);
    }
}
