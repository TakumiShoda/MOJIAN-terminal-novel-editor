//! 各屏幕：shelf / tree / editor / diff / proof / character / settings。见 doc.md §7。

pub mod confirm;
pub mod format_preview;
pub mod history_panel;
pub mod proof_panel;
pub mod search_panel;
pub mod shelf;
pub mod stats;
pub mod tree;

pub use confirm::Confirm;
pub use format_preview::FormatPreview;
pub use history_panel::{DiffView, HistoryPanel};
pub use proof_panel::ProofPanel;
pub use search_panel::SearchPanel;
pub use shelf::Shelf;
pub use stats::Stats;
pub use tree::Tree;
