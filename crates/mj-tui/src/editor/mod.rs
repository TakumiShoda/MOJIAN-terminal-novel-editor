//! 编辑器组件：视口、光标、软换行、undo。见 doc.md §6.3。

pub mod buffer;
pub mod view;

pub use buffer::Buffer;
pub use view::Viewport;
