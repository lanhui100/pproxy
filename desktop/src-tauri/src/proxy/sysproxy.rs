//! 系统代理注册（M6 spec §5，F10 细节）：Windows WinINET 注册表写入/还原。
//!
//! 两种模式：
//! - PAC 模式：AutoConfigURL = http://127.0.0.1:18900/pac
//! - 手动模式（实验性）：ProxyServer=127.0.0.1:18900，Override=<local>
//!
//! 安全细节：保存用户原值快照，关闭时还原原值（非清零）；写后广播
//! WM_SETTINGCHANGE 使运行中应用感知。非 Windows 平台为 no-op。
//! T4 追加：快照落盘 data_dir/sysproxy_snapshot.json（持久化），崩溃后下次启动按快照还原。

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum Mode {
    Pac,
    Manual,
}

#[allow(dead_code)]
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
#[allow(dead_code)]
const PAC_URL: &str = "http://127.0.0.1:18900/pac";
#[allow(dead_code)]
const SNAPSHOT_FILE: &str = "sysproxy_snapshot.json";

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub proxy_enable: Option<u32>,
    pub proxy_server: Option<String>,
    pub proxy_override: Option<String>,
    pub autoconfig_url: Option<String>,
    /// 持久化扩展：写入时 pid/ts，仅用于诊断；还原时忽略
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
}

#[allow(dead_code)]
fn data_dir_for_sysproxy() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("pony-desktop")
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".pony-desktop"))
            .unwrap_or_else(|_| std::env::temp_dir())
    }
}

#[allow(dead_code)]
fn snapshot_path() -> std::path::PathBuf {
    data_dir_for_sysproxy().join(SNAPSHOT_FILE)
}

#[allow(dead_code)]
fn save_snapshot(snapshot: &Snapshot) {
    let mut s = snapshot.clone();
    s.pid = Some(std::process::id());
    s.ts = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    let path = snapshot_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 原子写：tmp + rename
    let tmp = path.with_extension("json.tmp");
    if let Ok(txt) = serde_json::to_string_pretty(&s) {
        if std::fs::write(&tmp, txt).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[allow(dead_code)]
fn load_snapshot() -> Option<Snapshot> {
    let p = snapshot_path();
    let txt = std::fs::read_to_string(p).ok()?;
    serde_json::from_str::<Snapshot>(&txt).ok()
}

#[allow(dead_code)]
fn clear_snapshot() {
    let _ = std::fs::remove_file(snapshot_path());
}

#[allow(dead_code)]
pub fn current_pac_url() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{PAC_URL}?t={now}")
}

#[cfg(windows)]
pub fn flush_wininet_cache() {
    unsafe {
        use windows_sys::Win32::Networking::WinInet::{
            InternetSetOptionA, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
        };
        let r1 = InternetSetOptionA(std::ptr::null_mut(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null_mut(), 0);
        let r2 = InternetSetOptionA(std::ptr::null_mut(), INTERNET_OPTION_REFRESH, std::ptr::null_mut(), 0);
        if r1 == 0 || r2 == 0 {
            log::warn!("InternetSetOptionA returned 0 during flush_wininet_cache (settings: {r1}, refresh: {r2})");
        }
    }
}

#[cfg(not(windows))]
pub fn flush_wininet_cache() {}

/// 当白名单列表或模式改变时强刷 WinINET PAC 缓存
#[cfg(windows)]
pub fn update_pac_timestamp() -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)
        .map_err(|e| e.to_string())?;

    let cur: Option<String> = key.get_value("AutoConfigURL").ok();
    if let Some(url) = cur {
        if url.starts_with(PAC_URL) {
            let new_url = current_pac_url();
            key.set_value("AutoConfigURL", &new_url).map_err(|e| e.to_string())?;
            flush_wininet_cache();
            broadcast_change();
            log::info!("updated WinINET AutoConfigURL with new timestamp: {new_url}");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn update_pac_timestamp() -> Result<(), String> {
    Ok(())
}

/// 启用系统代理。返回启用前快照（供 disable 还原），并落盘持久化。
#[cfg(windows)]
pub fn enable(mode: Mode) -> Result<Snapshot, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)
        .map_err(|e| e.to_string())?;

    // 快照原值
    let snapshot = Snapshot {
        proxy_enable: key.get_value("ProxyEnable").ok(),
        proxy_server: key.get_value("ProxyServer").ok(),
        proxy_override: key.get_value("ProxyOverride").ok(),
        autoconfig_url: key.get_value("AutoConfigURL").ok(),
        pid: None,
        ts: None,
    };

    // 持久化先于注册表写入，保证崩溃后可还原
    save_snapshot(&snapshot);

    match mode {
        Mode::Pac => {
            let pac_url = current_pac_url();
            key.set_value("AutoConfigURL", &pac_url)
                .map_err(|e| e.to_string())?;
            key.delete_value("ProxyEnable").ok();
        }
        Mode::Manual => {
            key.set_value("ProxyEnable", &1u32)
                .map_err(|e| e.to_string())?;
            key.set_value("ProxyServer", &"127.0.0.1:18900")
                .map_err(|e| e.to_string())?;
            key.set_value("ProxyOverride", &"<local>")
                .map_err(|e| e.to_string())?;
            key.delete_value("AutoConfigURL").ok();
        }
    }
    flush_wininet_cache();
    let bc_ok = broadcast_change();
    if !bc_ok {
        log::warn!("broadcast_change SendMessageTimeoutA returned 0 (timeout/failed)");
    }
    // 二次校验：PAC URL 需 starts_with PAC 兼容 ?t= 指纹，回读校验并重试广播
    if mode == Mode::Pac {
        match key.get_value::<String, _>("AutoConfigURL") {
            Ok(v) if v.starts_with(PAC_URL) => {},
            Ok(v) => {
                log::warn!("PAC url mismatch after set: {v}, retrying broadcast");
                flush_wininet_cache();
                let _ = broadcast_change();
                // second read
                if let Ok(v2) = key.get_value::<String, _>("AutoConfigURL") {
                    if !v2.starts_with(PAC_URL) {
                        return Err(format!("PAC url verification failed after enable: {v2}"));
                    }
                } else {
                    return Err("PAC url missing after enable".into());
                }
            },
            Err(e) => return Err(format!("PAC url readback failed after enable: {e}")),
        }
    }
    Ok(snapshot)
}

/// 关闭并还原快照。优先使用传入快照；若为空则尝试读盘还原（T3 协同）。
#[cfg(windows)]
pub fn disable(snapshot: &Snapshot) -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;

    // 若传入为 default 空且存在持久化快照，则用持久化快照（崩溃后退出路径）
    let snap = if snapshot.proxy_enable.is_none()
        && snapshot.proxy_server.is_none()
        && snapshot.proxy_override.is_none()
        && snapshot.autoconfig_url.is_none()
    {
        if let Some(persisted) = load_snapshot() {
            persisted
        } else {
            snapshot.clone()
        }
    } else {
        snapshot.clone()
    };

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE)
        .map_err(|e| e.to_string())?;
    if let Some(v) = snap.proxy_enable {
        key.set_value("ProxyEnable", &v)
            .map_err(|e| e.to_string())?;
    } else {
        key.delete_value("ProxyEnable").ok();
    }
    match &snap.proxy_server {
        Some(v) => key.set_value("ProxyServer", v).map_err(|e| e.to_string())?,
        None => {
            key.delete_value("ProxyServer").ok();
        }
    }
    match &snap.proxy_override {
        Some(v) => key.set_value("ProxyOverride", v).map_err(|e| e.to_string())?,
        None => {
            key.delete_value("ProxyOverride").ok();
        }
    }
    match &snap.autoconfig_url {
        Some(v) => key.set_value("AutoConfigURL", v).map_err(|e| e.to_string())?,
        None => {
            key.delete_value("AutoConfigURL").ok();
        }
    }
    flush_wininet_cache();
    let ok = broadcast_change();
    if !ok {
        log::warn!("broadcast_change after disable returned failure");
    }
    clear_snapshot();
    Ok(())
}

/// 对外暴露的 disable 读取持久化版本（tray 退出路径确保读盘还原，即使内存快照丢失）
#[cfg(windows)]
pub fn disable_with_persisted_fallback(snapshot_opt: Option<Snapshot>) -> Result<(), String> {
    if let Some(s) = snapshot_opt {
        return disable(&s);
    }
    if let Some(persisted) = load_snapshot() {
        return disable(&persisted);
    }
    // 无快照时兜底：若残留 PAC 指向本应用则清除
    cleanup_stale();
    Ok(())
}

/// 广播设置变更（F10）：已运行应用感知代理切换。返回是否成功（SendMessageTimeoutA !=0）
#[cfg(windows)]
pub fn broadcast_change() -> bool {
    unsafe {
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutA, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        const TIMEOUT_MS: u32 = 1000;
        let mut result: usize = 0;
        let ret: LRESULT = SendMessageTimeoutA(
            HWND_BROADCAST as HWND,
            WM_SETTINGCHANGE,
            0 as WPARAM,
            c"InternetSettings".as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            &mut result as *mut usize,
        );
        if ret == 0 {
            log::warn!("SendMessageTimeoutA WM_SETTINGCHANGE failed (ret=0, timeout={TIMEOUT_MS}ms)");
            return false;
        }
        true
    }
}

#[cfg(not(windows))]
pub fn broadcast_change() -> bool {
    true
}

/// 启动自愈：优先按持久化快照还原；否则回落旧逻辑：若 AutoConfigURL 或 ProxyServer 指向本应用，则清除。
#[cfg(windows)]
pub fn cleanup_stale() {
    // 优先：若存在快照文件，按快照还原（崩溃后残留 PAC / Manual 代理的精确还原）
    if let Some(snap) = load_snapshot() {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) =
            hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)
        {
            let is_our_pac = key
                .get_value::<String, _>("AutoConfigURL")
                .ok()
                .map(|v| v.starts_with(PAC_URL))
                .unwrap_or(false);
            let is_our_manual = key
                .get_value::<String, _>("ProxyServer")
                .ok()
                .map(|v| v.contains("127.0.0.1:18900"))
                .unwrap_or(false);
            if is_our_pac || is_our_manual {
                let _ = disable(&snap);
                log::info!("restored proxy from persisted snapshot after stale settings detected");
                return;
            } else {
                // 已被外部改写，清理失效快照
                clear_snapshot();
            }
        }
    }

    // 回落：清除指向本应用 18900 的残留设置
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) =
        hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)
    else {
        return;
    };
    let is_our_pac = key
        .get_value::<String, _>("AutoConfigURL")
        .ok()
        .map(|v| v.starts_with(PAC_URL))
        .unwrap_or(false);
    let is_our_manual = key
        .get_value::<String, _>("ProxyServer")
        .ok()
        .map(|v| v.contains("127.0.0.1:18900"))
        .unwrap_or(false);

    if is_our_pac || is_our_manual {
        if is_our_pac {
            key.delete_value("AutoConfigURL").ok();
        }
        if is_our_manual {
            key.delete_value("ProxyEnable").ok();
            key.delete_value("ProxyServer").ok();
            key.delete_value("ProxyOverride").ok();
        }
        flush_wininet_cache();
        broadcast_change();
        clear_snapshot();
        log::info!("cleaned stale proxy settings left by previous crashed run");
    }
}

#[cfg(not(windows))]
pub fn cleanup_stale() {}

// ---- 非 Windows 平台：no-op（开发期占位；生产目标仅 Windows）----

#[cfg(not(windows))]
pub fn enable(_mode: Mode) -> Result<Snapshot, String> {
    Err("system proxy not supported on this platform".into())
}

#[cfg(not(windows))]
pub fn disable(_: &Snapshot) -> Result<(), String> {
    Err("system proxy not supported on this platform".into())
}

#[cfg(not(windows))]
pub fn disable_with_persisted_fallback(_: Option<Snapshot>) -> Result<(), String> {
    Err("system proxy not supported on this platform".into())
}
