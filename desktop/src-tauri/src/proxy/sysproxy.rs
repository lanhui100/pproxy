//! 系统代理注册（M6 spec §5，F10 细节）：Windows WinINET 注册表写入/还原。
//!
//! 两种模式：
//! - PAC 模式：AutoConfigURL = http://127.0.0.1:18900/pac
//! - 手动模式（实验性）：ProxyServer=127.0.0.1:18900，Override=<local>
//!
//! 安全细节：保存用户原值快照，关闭时还原原值（非清零）；写后广播
//! WM_SETTINGCHANGE 使运行中应用感知。非 Windows 平台为 no-op。

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Pac,
    Manual,
}

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const PAC_URL: &str = "http://127.0.0.1:18900/pac";

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub proxy_enable: Option<u32>,
    pub proxy_server: Option<String>,
    pub autoconfig_url: Option<String>,
}

/// 启用系统代理。返回启用前快照（供 disable 还原）。
#[cfg(windows)]
pub fn enable(mode: Mode) -> Result<Snapshot, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE).map_err(|e| e.to_string())?;

    // 快照原值
    let snapshot = Snapshot {
        proxy_enable: key.get_value("ProxyEnable").ok(),
        proxy_server: key.get_value("ProxyServer").ok(),
        autoconfig_url: key.get_value("AutoConfigURL").ok(),
    };

    match mode {
        Mode::Pac => {
            key.set_value("AutoConfigURL", &PAC_URL).map_err(|e| e.to_string())?;
            key.delete_value("ProxyEnable").ok();
        }
        Mode::Manual => {
            key.set_value("ProxyEnable", &1u32).map_err(|e| e.to_string())?;
            key.set_value("ProxyServer", &"127.0.0.1:18900").map_err(|e| e.to_string())?;
            key.set_value("ProxyOverride", &"<local>").map_err(|e| e.to_string())?;
            key.delete_value("AutoConfigURL").ok();
        }
    }
    broadcast_change();
    Ok(snapshot)
}

/// 关闭并还原快照。
#[cfg(windows)]
pub fn disable(snapshot: &Snapshot) -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE).map_err(|e| e.to_string())?;
    if let Some(v) = snapshot.proxy_enable {
        key.set_value("ProxyEnable", &v).map_err(|e| e.to_string())?;
    } else {
        key.delete_value("ProxyEnable").ok();
    }
    match &snapshot.proxy_server {
        Some(v) => key.set_value("ProxyServer", v).map_err(|e| e.to_string())?,
        None => { key.delete_value("ProxyServer").ok(); }
    }
    match &snapshot.autoconfig_url {
        Some(v) => key.set_value("AutoConfigURL", v).map_err(|e| e.to_string())?,
        None => { key.delete_value("AutoConfigURL").ok(); }
    }
    broadcast_change();
    Ok(())
}

/// 广播设置变更（F10）：已运行应用感知代理切换。
#[cfg(windows)]
fn broadcast_change() {
    // HWND_BROADCAST；SMTO_ABORTIFHUSH 避免挂起窗口阻塞
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutA, HWND_BROADCAST, SMTO_ABORTIFHUNG,
        };
        const WM_SETTINGCHANGE: u32 = 0x001A;
        SendMessageTimeoutA(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            "Internet Settings\0".as_ptr() as _,
            SMTO_ABORTIFHUNG,
            1000,
            std::ptr::null_mut(),
        );
    }
}

/// 启动自愈：上次进程崩溃/强杀时 AutoConfigURL 残留指向已死的本地 PAC 端口，
/// 白名单站点会全部失败（PAC 无 DIRECT 兜底）。启动时若发现仍指向本应用 PAC，
/// 说明引擎必然未运行，直接清除并广播还原直连。
#[cfg(windows)]
pub fn cleanup_stale() {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)
    else {
        return;
    };
    if key.get_value("AutoConfigURL").ok().as_deref() == Some(PAC_URL) {
        key.delete_value("AutoConfigURL").ok();
        broadcast_change();
        log::info!("cleaned stale AutoConfigURL left by previous crashed run");
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
