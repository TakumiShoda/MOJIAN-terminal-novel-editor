//! 错误类型的大小上限。
//!
//! 缘由：CI 在 windows-latest 上以 `result_large_err` 报错，而本机（macOS）不报——
//! Windows 的 `PathBuf` 更大，把 `Error` 顶过了 clippy 的 128 字节阈值。
//!
//! `Result<T>` 至少和 `Error` 一样大，意味着**每一次成功返回**都要搬运这些字节。
//! 这不是 lint 的洁癖，是真实成本；且这个成本在哪个平台上都存在，
//! 只是 Windows 先报出来而已。
//!
//! 故用测试把上限钉死：本机就能发现越界，不必等 CI 跑到 Windows 才知道。

/// clippy `result_large_err` 的默认阈值。
const CLIPPY_LARGE_ERR_THRESHOLD: usize = 128;

/// 本机上限，留出跨平台余量（Windows 的 PathBuf 比 Unix 大）。
const MAX_ERROR_SIZE: usize = 96;

#[test]
fn error_stays_small_enough_for_all_platforms() {
    let size = std::mem::size_of::<mj_core::Error>();

    assert!(
        size <= MAX_ERROR_SIZE,
        "Error 膨胀到 {size} 字节（本机上限 {MAX_ERROR_SIZE}，clippy 阈值 {CLIPPY_LARGE_ERR_THRESHOLD}）。\n\
         Result<T> 至少这么大，每次成功返回都要搬运它。\n\
         请把大字段装箱（如 Box<toml::de::Error>），而不是抬高这个上限。\n\
         注意：Windows 的 PathBuf 更大，本机不越界不代表 CI 不越界。"
    );
}

#[test]
fn result_is_no_larger_than_error() {
    // 这条是前一条的前提：若不成立，上面的上限就管不住 Result。
    assert_eq!(
        std::mem::size_of::<mj_core::Result<()>>(),
        std::mem::size_of::<mj_core::Error>()
    );
}
