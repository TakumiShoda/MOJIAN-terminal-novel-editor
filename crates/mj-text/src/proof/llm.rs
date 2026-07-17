//! 大模型校对器（`LlmProofreader`，病句主力）。见 doc.md §6.8。
//!
//! **M7**：默认关，需用户显式配 endpoint + key（key 不得明文入 config），
//! 首次开启弹「正文将发往第三方」的告知。走网络、按段分批、带缓存——全是 IO，
//! 实现属于 mj-core，本 crate 只占位。
