//! 测试专用：串行化进程级 `HOME` 改写。
//!
//! Rust 测试在同一进程内并行执行，而 `HOME` 是进程级全局状态。任一测试用
//! `set_var("HOME", ...)` 切到临时目录后，并发测试（config / store / sync 的落盘路径）
//! 会读到不一致的路径，表现为随机失败——`sync_roundtrip_url_safe_and_replay_protection`
//! 就在 CI 上偶发 `res1.is_ok()` 断言失败（本地不复现，取决于线程交错）。
//!
//! 约定：凡改写 `HOME` 的测试必须经本模块加锁；需要「配置落盘但不污染真实 HOME」的测试
//! 直接用 [`with_temp_home`]。

use std::path::Path;

static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 在 `HOME` 独占窗口内执行闭包，结束后恢复原值（含原本无 `HOME` 的情形）。
pub fn with_home<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let orig = std::env::var_os("HOME");
    std::env::set_var("HOME", dir);
    let result = f();
    match orig {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    result
}

/// 临时 `HOME` + 独占窗口：闭包内的配置/数据库落盘全部落在临时目录，退出即弃。
pub fn with_temp_home<T>(f: impl FnOnce(&Path) -> T) -> T {
    let tmp = tempfile::tempdir().expect("创建临时 HOME 失败");
    with_home(tmp.path(), || f(tmp.path()))
}
