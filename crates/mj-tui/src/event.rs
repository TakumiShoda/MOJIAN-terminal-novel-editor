//! 事件循环。见 doc.md §7.4。
//!
//! 并发用 `std::thread` + `std::sync::mpsc`，不引入 tokio（doc.md §3）。
//! 终端事件与 tick 各跑一个线程，汇入同一个 channel 由主循环取。

use std::sync::mpsc::{Receiver, RecvError, Sender, channel};
use std::time::Duration;

use ratatui::crossterm::event::Event;

/// M0 只有终端事件与 tick。Proof / Index / Font 等工作线程回传见后续里程碑。
#[derive(Debug)]
pub enum AppEvent {
    Term(Event),
    /// 100ms，驱动自动保存计时与动画。
    Tick,
}

pub struct EventLoop {
    rx: Receiver<AppEvent>,
    /// 持有 sender 让 channel 保持存活，即使采集线程意外退出。
    _tx: Sender<AppEvent>,
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

        Self { rx, _tx: tx }
    }

    /// 取下一个事件，阻塞直到有事件到达。
    pub fn next(&self) -> Result<AppEvent, RecvError> {
        self.rx.recv()
    }
}
