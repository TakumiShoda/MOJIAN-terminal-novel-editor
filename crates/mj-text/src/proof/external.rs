//! 外部命令校对器（`ExternalProofreader`）。见 doc.md §6.8。
//!
//! **M7**：默认关，调用用户配置的命令，stdin/stdout 走版本化 JSON 契约。
//! 它要起进程、读写管道，是 IO——按 §4 分层铁律实现属于 mj-core，本 crate 只占位。
