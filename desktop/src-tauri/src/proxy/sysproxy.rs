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
    let key = hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE | KEY_QUERY_VALUE)?;

    // 快照原值
    let snapshot = Snapshot {
        proxy_enable: key.get_value("ProxyEnable").ok(),
        proxy_server: key.get_value("ProxyServer").ok(),
        autoconfig_url: key.get_value("AutoConfigURL").ok(),
    };

    match mode {
        Mode::Pac => {
            key.set_value("AutoConfigURL", &PAC_URL)?;
            key.delete_value("ProxyEnable").ok();
        }
        Mode::Manual => {
            key.set_value("ProxyEnable", &1u32)?;
            key.set_value("ProxyServer", &"127.0.0.1:18900")?;
            key.set_value("ProxyOverride", &"<local>")?;
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
    let key = hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE)?;
    if let Some(v) = snapshot.proxy_enable {
        key.set_value("ProxyEnable", &v)?;
    } else {
        key.delete_value("ProxyEnable").ok();
    }
    match &snapshot.proxy_server {
        Some(v) => key.set_value("ProxyServer", v)?,
        None => key.delete_value("ProxyServer").ok(),
    }
    match &snapshot.autoconfig_url {
        Some(v) => key.set_value("AutoConfigURL", v)?,
        None => key.delete_value("AutoConfigURL").ok(),
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
            None,
        );
    }
}

// ---- 非 Windows 平台：no-op（开发期占位；生产目标仅 Windows）----


#[cfg(not(windows))]
pub fn enable(_mode: Mode) -> Result<Snapshot, String> {
    Err("system proxy not supported on this platform".into())
}

#[cfg(not(windows))]
pub fn disable(_: &Snapshot) -> Result<(), String> {
    Err("system proxy not supported on this platform".into())
}
