//! 各屏幕：shelf / tree / editor / diff / proof / character / settings。见 doc.md §7。

pub mod character_form;
pub mod character_panel;
pub mod command_palette;
pub mod completion;
pub mod confirm;
pub mod format_preview;
pub mod help;
pub mod history_panel;
pub mod modal;
pub mod proof_panel;
pub mod search_panel;
pub mod settings;
pub mod shelf;
pub mod stats;
pub mod tree;

pub use character_form::CharacterForm;
pub use character_panel::CharacterPanel;
pub use command_palette::CommandPalette;
pub use completion::Completion;
pub use confirm::Confirm;
pub use format_preview::FormatPreview;
pub use help::Help;
pub use history_panel::{DiffView, HistoryPanel};
pub use modal::{Modal, ModalKind, ModalStack};
pub use proof_panel::ProofPanel;
pub use search_panel::SearchPanel;
pub use settings::Settings;
pub use shelf::Shelf;
pub use stats::Stats;
pub use tree::Tree;
