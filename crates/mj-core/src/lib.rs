//! 领域模型、存储、索引、版本历史。见 doc.md §5。

pub mod atomic;
pub mod config;
pub mod error;
pub mod history;
pub mod id;
pub mod index;
pub mod lock;
pub mod model;
pub mod store;
pub mod workspace;

pub use error::{Error, Result};
pub use workspace::Workspace;
