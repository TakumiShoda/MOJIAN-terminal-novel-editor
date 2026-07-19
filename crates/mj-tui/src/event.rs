//! 事件循环。见 doc.md §7.4。
//!
//! 并发用 `std::thread` + `std::sync::mpsc`，不引入 tokio（doc.md §3）。
//! 终端事件与 tick 各跑一个线程，汇入同一个 channel 由主循环取。

use std::sync::mpsc::{Receiver, RecvError, Sender, channel};
use std::time::Duration;

use ratatui::crossterm::event::Event;

#[derive(Debug)]
pub enum AppEvent {
    Term(Event),
    /// 100ms，驱动自动保存计时与动画。
    Tick,
    /// 模型校对跑完了，工作线程回传（§6.8、§7「长任务一律进工作线程」）。
    LlmProof(Box<LlmProofDone>),
}

/// 一趟模型校对的结果。
///
/// `chapter` + `text_hash` 是**指纹**，不是附赠信息：请求要跑好几秒，这期间用户
/// 完全可能改了正文或换了章，而 `issues` 里的是当时那份文本的字节偏移。指纹对不上
/// 就必须整份丢掉——拿旧坐标往新正文上画，下划线会落在毫不相干的字上。
#[derive(Debug)]
pub struct LlmProofDone {
    pub chapter: mj_core::id::ChapterId,
    pub text_hash: String,
    pub issues: Vec<mj_text::proof::Issue>,
    pub warning: Option<String>,
}

pub struct EventLoop {
    rx: Receiver<AppEvent>,
    /// 持有 sender 让 channel 保持存活，即使采集线程意外退出。
    tx: Sender<AppEvent>,
}

impl EventLoop {
    /// 启动采集线程。
    ///
    /// 两个线程都是 detached：进程退出时随之终止。它们只读终端、不碰正文，
    /// 没有需要清理的状态。
    pub fn spawn() -> Self {
        let (tx, rx) = channel();

        // 终端事件：阻塞读，来一个送一个。
        let term_tx = tx.clone();
        std::thread::Builder::new()
            .name("mj-term-events".into())
            .spawn(move || {
                loop {
                    match ratatui::crossterm::event::read() {
                        Ok(ev) => {
                            if term_tx.send(AppEvent::Term(ev)).is_err() {
                                break; // 主循环已退出
                            }
                        }
                        Err(e) => {
                            // 读终端失败通常意味着 stdin 已关闭；记日志后退出线程，
                            // 不能 panic——那会连累整个进程。
                            tracing::warn!(error = %e, "读取终端事件失败，采集线程退出");
                            break;
                        }
                    }
                }
            })
            .ok();

        let tick_tx = tx.clone();
        std::thread::Builder::new()
            .name("mj-tick".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    if tick_tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
            })
            .ok();

        Self { rx, tx }
    }

    /// 给工作线程用的回传端。
    pub fn sender(&self) -> Sender<AppEvent> {
        self.tx.clone()
    }

    /// 取下一个事件，阻塞直到有事件到达。
    pub fn next(&self) -> Result<AppEvent, RecvError> {
        self.rx.recv()
    }

    /// 取事件，没有就立刻返回 None。
    ///
    /// 批量作业（全书排版/替换）跑的时候用它：主循环不能停在 `recv` 上
    /// 干等，否则进度条不动、Esc 也按不了；但也不能不理按键——
    /// §6.5 要求「可中断」。故干一小块活、瞄一眼有没有按键，如此往复。
    pub fn try_next(&self) -> Option<AppEvent> {
        self.rx.try_recv().ok()
    }
}
