//! 纯函数文本处理。
//!
//! 分层铁律（doc.md §4）：本 crate 输入 `&str`，输出结果，零 IO、零全局状态。
//! 这是为了让排版幂等性、统计一致性能被 proptest 大规模验证。

pub mod count;
pub mod eol;
pub mod format;
pub mod proof;
pub mod search;
pub mod width;
