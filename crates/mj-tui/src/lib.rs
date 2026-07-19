//! ratatui 界面层。见 doc.md §6.10 / §7。

pub mod app;
pub mod batch;
pub mod clipboard;
pub mod commands;
pub mod doctor;
pub mod editor;
pub mod event;
pub mod font;
pub mod keyboard;
pub mod keymap;
pub mod panic;
pub mod screens;
pub mod theme;

pub use app::run;
pub use panic::CrashDump;
