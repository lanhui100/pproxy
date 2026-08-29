mod proxy;

use std::sync::atomic::{AtomicBool, Ordering as AOrd};
use std::sync::OnceLock;
use tokio::sync::watch;

static TRAY_TOGGLE_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();
static TRAY_MODE_WL_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();
static TRAY_MODE_GB_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  // ---- 单实例保护（严格基于 PID 存活性，严禁基于运行时间自毁锁文件）----
  {
    let lock_path = data_dir().join("instance.lock");
    if let Some(parent) = lock_path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    {
      let is_stale = match std::fs::metadata(&lock_path) {
        Ok(_) => {
          let pid_alive = std::fs::read_to_string(&lock_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|pid| {
              if pid == std::process::id() {
                true
              } else {
                #[cfg(windows)]
                {
                  use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
                  use windows_sys::Win32::Foundation::CloseHandle;
                  unsafe {
                    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    if !handle.is_null() {
                      CloseHandle(handle);
                      true
                    } else {
                      false
                    }
                  }
                }
                #[cfg(unix)]
                {
                  std::process::Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
                }
                #[cfg(not(any(unix, windows)))]
                { true }
              }
            })
            .unwrap_or(false);
          !pid_alive
        }
        Err(_) => false,
      };
      if is_stale {
        let _ = std::fs::remove_file(&lock_path);
      }
      match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
        Ok(mut f) => { use std::io::Write as _; let _ = writeln!(f, "{}", std::process::id()); }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
          eprintln!("another instance is running, exiting");
          std::process::exit(0);
        }
        Err(_) => {}
      }
    }
  }
  let ctx = tauri::generate_context!();
  let app = tauri::Builder::default()
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_http::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .setup(|app| {
      use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
      use tauri::tray::TrayIconBuilder;
      use tauri::Emitter;
      // T3 重排：cleanup_stale(持久化还原) → init_watch_from_file → (engine.bind 成功后才 sysproxy::enable 在 proxy_enable 内) → broadcast → emit proxy-ready
      proxy::sysproxy::cleanup_stale();
      init_watch_from_file();
      let current_mode = load_proxy_mode_from_file();
      let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
      let toggle = CheckMenuItem::with_id(app, "proxy_toggle", "系统代理: 已启用", true, ENGINE_ON.load(AOrd::SeqCst), None::<&str>)?;
      let mode_wl = CheckMenuItem::with_id(app, "mode_whitelist", "  白名单模式 (智能分流)", true, current_mode == proxy::pac::ProxyMode::Whitelist, None::<&str>)?;
      let mode_gb = CheckMenuItem::with_id(app, "mode_global", "  全局模式 (全部流量)", true, current_mode == proxy::pac::ProxyMode::Global, None::<&str>)?;
      let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
      let sep1 = PredefinedMenuItem::separator(app)?;
      let sep2 = PredefinedMenuItem::separator(app)?;
      let menu = Menu::with_items(app, &[&show, &sep1, &toggle, &mode_wl, &mode_gb, &sep2, &quit])?;
      let _ = TRAY_TOGGLE_ITEM.set(toggle.clone());
      let _ = TRAY_MODE_WL_ITEM.set(mode_wl.clone());
      let _ = TRAY_MODE_GB_ITEM.set(mode_gb.clone());

      let icon = app.default_window_icon().cloned().expect("window icon");
      TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Pony Proxy")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
          "show" => {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus(); }
          }
          "proxy_toggle" => {
            let on = ENGINE_ON.load(AOrd::SeqCst);
            let res = if on { proxy_disable_inner(app.clone()) } else { proxy_enable_inner(app.clone()) };
            if let Err(e) = res { log::warn!("proxy_toggle failed: {e}"); }
          }
          "mode_whitelist" => {
            let _ = proxy_mode_set(app.clone(), "whitelist".into());
          }
          "mode_global" => {
            let _ = proxy_mode_set(app.clone(), "global".into());
          }
          "quit" => {
            if let Err(e) = proxy_disable_inner(app.clone()) { log::warn!("proxy_disable on quit failed: {e}"); }
            let _ = std::fs::remove_file(data_dir().join("instance.lock"));
            app.exit(0);
          }
          _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
          use tauri::tray::TrayIconEvent;
          use tauri::Manager;
          let is_click = matches!(ev, TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, .. } | TrayIconEvent::DoubleClick { button: tauri::tray::MouseButton::Left, .. });
          if is_click {
            let app = tray.app_handle();
            if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus(); }
          }
        })
        .build(app)?;
      if cfg!(debug_assertions) {
        app.handle().plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())?;
      }
      let handle = app.handle().clone();
      let _ = handle.emit("proxy-ready", serde_json::json!({"ready": true, "mode": match current_mode { proxy::pac::ProxyMode::Whitelist => "whitelist", proxy::pac::ProxyMode::Global => "global" }}));
      Ok(())
    })
    .on_window_event(|window, event| {
      if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        use tauri::Manager;
        let app_handle = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
          let cfg = app_config_get();
          let shown = cfg.get("balloon_shown").and_then(|v| v.as_bool()).unwrap_or(false);
          if !shown {
            use tauri_plugin_notification::NotificationExt;
            let _ = app_handle.notification().builder().title("Pony Proxy").body("已最小化到托盘，代理仍在运行。右键托盘可退出").show();
            let _ = app_config_set(serde_json::json!({"balloon_shown": true}));
          }
        });
      }
    })
    .invoke_handler(tauri::generate_handler![
      credential_get, credential_set, credential_delete,
      proxy_whitelist_get, proxy_whitelist_set, proxy_mode_get, proxy_mode_set,
      proxy_enable, proxy_disable, proxy_pac, proxy_status, proxy_test_sites,
      proxy_tunnel_get, proxy_tunnel_set_url, tunnel_token_save, tunnel_token_clear,
      proxy_auto_config_get, proxy_auto_config_set, app_config_get, app_config_set,
      proxy_bypass_hosts, api_bypass_fetch,
    ])
    .build(ctx)
    .expect("error while building tauri application");
  app.run(|app_handle, event| {
    match event {
      tauri::RunEvent::ExitRequested { .. } => { if let Err(e)=proxy_disable_inner(app_handle.clone()){log::warn!("ExitRequested disable failed: {e}");} let _=std::fs::remove_file(data_dir().join("instance.lock")); }
      tauri::RunEvent::Exit => { let _ = proxy_disable_inner(app_handle.clone()); let _=std::fs::remove_file(data_dir().join("instance.lock")); }
      _ => {}
    }
  });
}

const CREDENTIAL_SERVICE: &str = "pony-desktop";
const CREDENTIAL_USER: &str = "admin_token";
const CREDENTIAL_USER_TUNNEL: &str = "tunnel_token";

fn cred_entry(user: &str) -> Result<keyring::Entry, String> {
  keyring::Entry::new(CREDENTIAL_SERVICE, user).map_err(|e| format!("keyring entry error: {e}"))
}
#[cfg(debug_assertions)]
fn cred_dev_file(user: &str) -> Option<std::path::PathBuf> {
  std::env::var("PONY_DESKTOP_DEV_FILE_KEYRING").ok().map(|_| std::env::temp_dir().join("pony-desktop-dev-keyring").join(user))
}
fn cred_set_impl(user: &str, secret: String) -> Result<(), String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) {
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    return std::fs::write(p, secret).map_err(|e| e.to_string());
  }
  let ent = cred_entry(user)?;
  ent.set_password(&secret).map_err(|e| format!("credential set failed: {e}"))
}
fn cred_get_impl(user: &str) -> Result<Option<String>, String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { return Ok(std::fs::read_to_string(p).ok()); }
  match cred_entry(user)?.get_password() {
    Ok(v) => Ok(Some(v)),
    Err(keyring::Error::NoEntry) => Ok(None),
    Err(e) => Err(format!("credential get failed: {e}")),
  }
}
fn cred_delete_impl(user: &str) -> Result<(), String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { let _=std::fs::remove_file(p); return Ok(()); }
  match cred_entry(user)?.delete_credential() {
    Ok(()) => Ok(()),
    Err(keyring::Error::NoEntry) => Ok(()),
    Err(e) => Err(format!("credential delete failed: {e}")),
  }
}
#[tauri::command]
fn credential_set(secret: String) -> Result<(), String> { cred_set_impl(CREDENTIAL_USER, secret) }
#[tauri::command]
fn credential_get() -> Result<Option<String>, String> { cred_get_impl(CREDENTIAL_USER) }
#[tauri::command]
fn credential_delete() -> Result<(), String> { cred_delete_impl(CREDENTIAL_USER) }

// ---- M6 + T1 watch 通道 ----
static WHITELIST_TX: OnceLock<watch::Sender<Vec<String>>> = OnceLock::new();
static PROXY_MODE_TX: OnceLock<watch::Sender<proxy::pac::ProxyMode>> = OnceLock::new();
// 契约：所有读方必须 clone 后立即 drop guard，再做 matches/generate_pac，不持锁跨 await
// 已通过 tokio::sync::watch 实现 clone-then-drop；编译期 lint: #[deny(clippy::await_holding_lock)] 在 engine.rs

fn seed() -> Vec<String> {
    ["github.com", "githubusercontent.com", "google.com", "youtube.com", "googlevideo.com", "githubassets.com", "googleusercontent.com", "gstatic.com", "googleapis.com", "ytimg.com", "ggpht.com","openai.com", "chatgpt.com", "anthropic.com", "claude.ai"].iter().map(|s| s.to_string()).collect()
}
const ALWAYS_TUNNEL: &[&str] = &["github.com", "githubusercontent.com"];
fn data_dir() -> std::path::PathBuf {
    #[cfg(windows)] { std::env::var("APPDATA").map(std::path::PathBuf::from).unwrap_or(std::env::temp_dir()).join("pony-desktop") }
    #[cfg(not(windows))] { std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".pony-desktop")).unwrap_or(std::env::temp_dir()) }
}
fn app_config_path() -> std::path::PathBuf { data_dir().join("app_config.json") }
fn whitelist_file_path() -> std::path::PathBuf { data_dir().join("whitelist.json") }
fn load_whitelist_from_file() -> Vec<String> {
    std::fs::read_to_string(whitelist_file_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(seed)
}
fn load_proxy_mode_from_file() -> proxy::pac::ProxyMode {
    let cfg = app_config_get();
    if let Some(m) = cfg.get("proxy_mode").and_then(|v| v.as_str()) {
        m.parse().unwrap_or(proxy::pac::ProxyMode::Whitelist)
    } else {
        proxy::pac::ProxyMode::Whitelist
    }
}
fn init_watch_from_file() {
    if WHITELIST_TX.get().is_none() {
        let wl = load_whitelist_from_file();
        let (tx, _rx) = watch::channel(wl);
        let _ = WHITELIST_TX.set(tx);
    }
    if PROXY_MODE_TX.get().is_none() {
        let mode = load_proxy_mode_from_file();
        let (tx, _rx) = watch::channel(mode);
        let _ = PROXY_MODE_TX.set(tx);
    }
}
fn ensure_watch() -> &'static watch::Sender<Vec<String>> {
    WHITELIST_TX.get_or_init(|| {
        let wl = load_whitelist_from_file();
        let (tx, _rx) = watch::channel(wl);
        tx
    })
}
fn ensure_mode_watch() -> &'static watch::Sender<proxy::pac::ProxyMode> {
    PROXY_MODE_TX.get_or_init(|| {
        let mode = load_proxy_mode_from_file();
        let (tx, _rx) = watch::channel(mode);
        tx
    })
}
static TUNNEL_TX: OnceLock<watch::Sender<(Option<String>, Option<String>)>> = OnceLock::new();
fn ensure_tunnel_watch() -> &'static watch::Sender<(Option<String>, Option<String>)> {
    TUNNEL_TX.get_or_init(|| {
        let (u, t) = tunnel_config_load();
        let (tx, _rx) = watch::channel((u, t));
        tx
    })
}
#[tauri::command]
fn proxy_whitelist_get() -> Vec<String> {
    if let Some(tx) = WHITELIST_TX.get() {
        let snapshot = { let g = tx.borrow(); g.clone() };
        return snapshot;
    }
    load_whitelist_from_file()
}
#[tauri::command]
fn proxy_mode_get() -> String {
    let mode = if let Some(tx) = PROXY_MODE_TX.get() {
        *tx.borrow()
    } else {
        load_proxy_mode_from_file()
    };
    match mode {
        proxy::pac::ProxyMode::Whitelist => "whitelist".into(),
        proxy::pac::ProxyMode::Global => "global".into(),
    }
}
#[tauri::command]
fn proxy_mode_set(app: tauri::AppHandle, mode: String) -> Result<(), String> {
    let parsed_mode: proxy::pac::ProxyMode = mode.parse()?;
    let tx = ensure_mode_watch();
    let _ = tx.send(parsed_mode);
    let _ = app_config_set(serde_json::json!({ "proxy_mode": mode }));
    if ENGINE_ON.load(AOrd::SeqCst) {
        let _ = proxy::sysproxy::update_pac_timestamp();
    }
    sync_tray_and_emit(&app, ENGINE_ON.load(AOrd::SeqCst));
    use tauri::Emitter;
    let _ = app.emit("proxy-mode-changed", serde_json::json!({ "mode": mode }));
    Ok(())
}
fn is_valid_domain(e: &str) -> bool {
    if e.is_empty() || e.len() > 253 { return false; }
    if e.contains('_') || e.contains('*') || e.contains(':') || e.contains('/') || e.contains('?') || e.contains('#') || e.contains(' ') { return false; }
    let parts: Vec<&str> = e.split('.').collect();
    if parts.len() < 2 { return false; } // 必须包含二级域及以上，严禁单段 TLD 通配
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 63 { return false; }
        let bytes = p.as_bytes();
        if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len()-1].is_ascii_alphanumeric() { return false; }
        if !bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-') { return false; }
        // 顶级域 (TLD) 不得为纯数字或带减号，且长度 >= 2
        if i == parts.len() - 1 && (!p.chars().all(|c| c.is_ascii_alphabetic()) || p.len() < 2) {
            return false;
        }
    }
    true
}
fn suffix_match(host: &str, entry: &str) -> bool {
    let h = host.to_ascii_lowercase();
    let e = entry.to_ascii_lowercase();
    if h == e { return true; }
    h.ends_with(&format!(".{e}"))
}
#[tauri::command]
fn proxy_whitelist_set(entries: Vec<String>) -> Result<(), String> {
    let mut normalized: Vec<String> = entries.iter().map(|s| {
        let mut h = s.trim().to_ascii_lowercase();
        while h.ends_with('.') { h.pop(); }
        h
    }).filter(|s| !s.is_empty()).collect();
    normalized.sort_unstable();
    normalized.dedup();
    for e in &normalized {
        if !is_valid_domain(e) { return Err(format!("无效域名: {e}")); }
    }
    // PAC 注入面：禁止包含管理面 host
    {
        let bypass_set = proxy::pac::collect_bypass_hosts();
        for b in bypass_set.iter() {
            if normalized.iter().any(|e| e == b) || normalized.iter().any(|e| suffix_match(b, e)) {
                return Err(format!("白名单禁止包含管理面 host: {b}"));
            }
            if proxy::whitelist::matches(b, &normalized) {
                return Err(format!("白名单禁止包含管理面 host: {b}"));
            }
        }
    }
    let existing = if let Some(tx) = WHITELIST_TX.get() { tx.borrow().clone() } else { load_whitelist_from_file() };
    for e in &normalized {
        if !existing.contains(e) && proxy::whitelist::matches(e, &existing) {
            if let Some(cover) = existing.iter().find(|c| proxy::whitelist::matches(e, &[(*c).clone()])) {
                return Err(format!("{} 已包含在 {} 中，无需重复添加", e, cover));
            }
            // 通用兜底：已被别名覆盖但未找到显式 cover 文案时仍拒绝
            return Err(format!("{} 已包含在现有名单的别名中，无需重复添加", e));
        }
    }
    // 父域合并子域自动移除：计算最小集
    let mut sorted = normalized.clone();
    sorted.sort_by(|a,b| a.len().cmp(&b.len()).then(a.cmp(b)));
    let mut minimal: Vec<String> = Vec::new();
    for cand in sorted {
        if proxy::whitelist::matches(&cand, &minimal) { continue; }
        minimal.retain(|ex| !proxy::whitelist::matches(ex, std::slice::from_ref(&cand)));
        minimal.push(cand);
    }
    minimal.sort_unstable();
    // validate→normalize→write tmp+rename→watch.send→broadcast_change 顺序，广播在锁外
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("whitelist.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&minimal).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("whitelist.json")).map_err(|e| e.to_string())?;
    let tx = ensure_watch();
    let _ = tx.send(minimal.clone());
    if ENGINE_ON.load(AOrd::SeqCst) {
        let _ = proxy::sysproxy::update_pac_timestamp();
    } else {
        let ok = proxy::sysproxy::broadcast_change();
        if !ok { log::warn!("broadcast_change after whitelist set failed"); }
    }
    Ok(())
}

// ---- 隧道中继配置 ----
const TUNNEL_FILE: &str = "tunnel.json";
fn validate_tunnel_url(url: &str) -> Result<(), String> {
  let urls: Vec<&str> = url.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
  if urls.is_empty() { return Err("tunnel url cannot be empty".into()); }
  for u in urls {
    if !u.starts_with("wss://") && !u.starts_with("ws://") {
      return Err(format!("tunnel url '{u}' must start with wss:// or ws://"));
    }
  }
  Ok(())
}
#[tauri::command]
fn proxy_tunnel_get() -> serde_json::Value {
  let url = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v.get("url").and_then(|u| u.as_str()).map(String::from)).unwrap_or_default();
  let has_token = matches!(cred_get_impl(CREDENTIAL_USER_TUNNEL), Ok(Some(t)) if !t.is_empty());
  serde_json::json!({ "url": url, "has_token": has_token })
}
#[tauri::command]
fn proxy_tunnel_set_url(url: String) -> Result<(), String> {
  let url = url.trim().to_string();
  validate_tunnel_url(&url)?;
  let dir = data_dir();
  std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
  let tmp = dir.join("tunnel.json.tmp");
  let target = dir.join(TUNNEL_FILE);
  std::fs::write(&tmp, serde_json::to_string(&serde_json::json!({ "url": url })).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
  std::fs::rename(&tmp, target).map_err(|e| e.to_string())?;
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  Ok(())
}
#[tauri::command]
fn tunnel_token_save(secret: String) -> Result<(), String> {
  if secret.trim().is_empty() { return Err("empty tunnel token".into()); }
  cred_set_impl(CREDENTIAL_USER_TUNNEL, secret)?;
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  Ok(())
}
#[tauri::command]
fn tunnel_token_clear() -> Result<(), String> {
  cred_delete_impl(CREDENTIAL_USER_TUNNEL)?;
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  Ok(())
}
fn tunnel_config_load() -> (Option<String>, Option<String>) {
  let url = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v.get("url").and_then(|u| u.as_str()).map(String::from)).filter(|u| validate_tunnel_url(u).is_ok());
  let token = cred_get_impl(CREDENTIAL_USER_TUNNEL).ok().flatten().filter(|t| !t.is_empty());
  match (url, token) { (Some(u), Some(t)) => (Some(u), Some(t)), _ => (None, None) }
}
static ENGINE_ON: AtomicBool = AtomicBool::new(false);
static SNAPSHOT: std::sync::Mutex<Option<proxy::sysproxy::Snapshot>> = std::sync::Mutex::new(None);
static ENGINE_TASK: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>> = std::sync::Mutex::new(None);
fn sync_tray_and_emit(app: &tauri::AppHandle, on: bool) {
  use tauri::Emitter;
  let current_mode = if let Some(tx) = PROXY_MODE_TX.get() { *tx.borrow() } else { load_proxy_mode_from_file() };
  let mode_str = match current_mode { proxy::pac::ProxyMode::Whitelist => "whitelist", proxy::pac::ProxyMode::Global => "global" };
  let _ = app.emit("proxy-status-changed", serde_json::json!({"on": on, "mode": mode_str}));
  let _ = app.emit("proxy-ready", serde_json::json!({"ready": true, "on": on, "mode": mode_str}));
  if let Some(toggle) = TRAY_TOGGLE_ITEM.get() {
    let _ = toggle.set_checked(on);
    let _ = toggle.set_text(if on { "系统代理: 已启用" } else { "系统代理: 已停用" });
  }
  if let Some(mode_wl) = TRAY_MODE_WL_ITEM.get() {
    let _ = mode_wl.set_checked(current_mode == proxy::pac::ProxyMode::Whitelist);
  }
  if let Some(mode_gb) = TRAY_MODE_GB_ITEM.get() {
    let _ = mode_gb.set_checked(current_mode == proxy::pac::ProxyMode::Global);
  }
}
fn proxy_enable_inner(app: tauri::AppHandle) -> Result<(), String> {
    if ENGINE_ON.load(AOrd::SeqCst) { sync_tray_and_emit(&app, true); return Ok(()); }
    let mut wl = { if let Some(tx)=WHITELIST_TX.get(){tx.borrow().clone()} else { init_watch_from_file(); WHITELIST_TX.get().map(|tx| tx.borrow().clone()).unwrap_or_else(load_whitelist_from_file) } };
    for h in ALWAYS_TUNNEL { if !wl.iter().any(|w| w==h) { wl.push(h.to_string()); } }
    let (tunnel_url, tunnel_token) = tunnel_config_load();
    if !wl.is_empty() && (tunnel_url.is_none() || tunnel_token.is_none()) {
        return Err("隧道未配置：白名单流量无法出网。请先在「设置 → 隧道中继」保存端点与令牌（二者缺一不可），再开启总开关".into());
    }
    {
        let tx = ensure_watch();
        if tx.borrow().clone() != wl { let _ = tx.send(wl.clone()); }
        let _ = ensure_tunnel_watch().send((tunnel_url, tunnel_token));
    }
    // Engine singleton: reuse existing task if alive (热更新 via watch, 不重复 bind)
    let already_running = ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()).is_some();
    if !already_running {
        let rx = ensure_watch().subscribe();
        let rx_mode = ensure_mode_watch().subscribe();
        let rx_tunnel = ensure_tunnel_watch().subscribe();
        let stats = std::sync::Arc::new(proxy::engine::EngineStats::default());
        let rx_clone = rx.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let cfg = proxy::engine::EngineConfig {
                listen_addr: "127.0.0.1:18900".into(),
                whitelist: rx_clone,
                mode: rx_mode,
                tunnel: rx_tunnel,
            };
            if let Err(e) = proxy::engine::run(cfg, stats).await { log::warn!("proxy engine exited: {e}"); }
        });
        *ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    } else {
        // Already running: whitelist 与 tunnel 已 via watch 更新，无需重 spawn
        log::info!("engine already running, reuse existing listener");
    }
    // Probe: must succeed before setting PAC, otherwise fail fast (Blocker #1)
    let probe_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if std::net::TcpStream::connect("127.0.0.1:18900").is_ok() { break; }
        if std::time::Instant::now() >= probe_deadline { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if std::net::TcpStream::connect("127.0.0.1:18900").is_err() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect("127.0.0.1:18900").is_err() {
            // 清理刚 spawn 的失败 task
            if let Some(h) = ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()).take() {
                h.abort();
            }
            return Err("代理引擎启动失败：127.0.0.1:18900 未就绪，请检查端口占用后重试".into());
        }
    }
    let snap = proxy::sysproxy::enable(proxy::sysproxy::Mode::Pac)?;
    *SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()) = Some(snap);
    ENGINE_ON.store(true, AOrd::SeqCst);
    sync_tray_and_emit(&app, true);
    use tauri::Emitter; let _ = app.emit("proxy-ready", serde_json::json!({"on": true}));
    Ok(())
}
fn proxy_disable_inner(app: tauri::AppHandle) -> Result<(), String> {
    let snap_opt = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(snap) = snap_opt.clone() {
        proxy::sysproxy::disable(&snap).map_err(|e| { log::warn!("proxy disable failed: {e}"); e })?;
    } else {
        let _ = proxy::sysproxy::disable_with_persisted_fallback(None).map_err(|e| { log::warn!("disable fallback failed: {e}"); e });
    }
    *SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()) = None;
    let _was_on = ENGINE_ON.swap(false, AOrd::SeqCst);
    sync_tray_and_emit(&app, false);
    Ok(())
}
#[tauri::command]
fn proxy_enable(app: tauri::AppHandle) -> Result<(), String> { proxy_enable_inner(app) }
#[tauri::command]
fn proxy_disable(app: tauri::AppHandle) -> Result<(), String> { proxy_disable_inner(app) }
#[tauri::command]
fn proxy_status() -> serde_json::Value {
    let mode = if let Some(tx) = PROXY_MODE_TX.get() { *tx.borrow() } else { load_proxy_mode_from_file() };
    serde_json::json!({
        "engine_running": ENGINE_ON.load(AOrd::SeqCst),
        "mode": match mode { proxy::pac::ProxyMode::Whitelist => "whitelist", proxy::pac::ProxyMode::Global => "global" }
    })
}
async fn dial_via_proxy(proxy_addr: &str, host: &str, port: u16) -> std::io::Result<()> {
    let mut s = tokio::net::TcpStream::connect(proxy_addr).await?;
    let req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut buf: Vec<u8> = Vec::with_capacity(128);
    let mut tmp = [0u8; 1024];
    loop {
        let n = s.read(&mut tmp).await?;
        if n == 0 { return Err(std::io::Error::other("proxy closed before response")); }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8*1024 { break; }
    }
    let head = String::from_utf8_lossy(&buf);
    if head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2") { Ok(()) } else { Err(std::io::Error::other(format!("proxy refused: {}", head.lines().next().unwrap_or("")))) }
}
#[tauri::command]
async fn proxy_test_sites() -> Result<Vec<serde_json::Value>, String> {
    if !ENGINE_ON.load(AOrd::SeqCst) { return Err("代理未启用：请先打开系统代理总开关".into()); }
    const PROXY_ADDR: &str = "127.0.0.1:18900";
    let sites = ["google.com", "x.com", "openai.com", "anthropic.com", "github.com"];
    let mut out = Vec::new();
    for site in sites {
        let started = std::time::Instant::now();
        let r = tokio::time::timeout(std::time::Duration::from_secs(10), dial_via_proxy(PROXY_ADDR, site, 443)).await;
        let ok = matches!(r, Ok(Ok(())));
        let ms = started.elapsed().as_millis() as u64;
        let error = match &r { Err(_)=>"timeout".to_string(), Ok(Err(e))=>e.to_string(), Ok(Ok(()))=>String::new() };
        out.push(serde_json::json!({"site": site, "ok": ok, "ms": ms, "error": error}));
    }
    Ok(out)
}
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
#[tauri::command]
fn proxy_pac() -> String {
    let entries = { if let Some(tx)=WHITELIST_TX.get(){ let g=tx.borrow(); g.clone() } else { load_whitelist_from_file() } };
    let mode = { if let Some(tx)=PROXY_MODE_TX.get(){ *tx.borrow() } else { load_proxy_mode_from_file() } };
    proxy::pac::generate_pac(&entries, mode)
}
#[tauri::command]
async fn api_bypass_fetch(method: String, url: String, headers: Option<std::collections::HashMap<String,String>>, body: Option<String>) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
    let m = match method.to_ascii_uppercase().as_str() {
        "GET"=>reqwest::Method::GET, "POST"=>reqwest::Method::POST, "PATCH"=>reqwest::Method::PATCH, "DELETE"=>reqwest::Method::DELETE, "PUT"=>reqwest::Method::PUT, _=>reqwest::Method::GET,
    };
    let mut req = client.request(m, &url);
    if let Some(hs)=headers { for (k,v) in hs { req = req.header(k, v); } }
    if let Some(b)=body { req = req.body(b).header("Content-Type", "application/json"); }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.clone()));
    Ok(serde_json::json!({"status": status, "body": json, "text": text}))
}
#[tauri::command]
fn proxy_auto_config_get() -> serde_json::Value { std::fs::read_to_string(app_config_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::json!({})) }
#[tauri::command]
fn proxy_auto_config_set(patch: serde_json::Value) -> Result<(), String> { app_config_set(patch) }
#[tauri::command]
fn app_config_get() -> serde_json::Value { proxy_auto_config_get() }
#[tauri::command]
fn app_config_set(patch: serde_json::Value) -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cur = proxy_auto_config_get();
    if let (Some(map_cur), Some(map_patch)) = (cur.as_object_mut(), patch.as_object()) {
        for (k,v) in map_patch { map_cur.insert(k.clone(), v.clone()); }
    } else { cur = patch; }
    let tmp = dir.join("app_config.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&cur).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, app_config_path()).map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
fn proxy_bypass_hosts() -> Vec<String> { proxy::pac::collect_bypass_hosts().into_iter().collect() }

#[cfg(test)]
mod tests {
    use super::*;
    async fn fake_proxy(script: &'static str) -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { loop { if let Ok((mut s, _))=l.accept().await { tokio::spawn(async move { let mut buf=[0u8;1024]; let _=s.read(&mut buf).await; let _=s.write_all(script.as_bytes()).await; }); } } });
        addr
    }
    #[tokio::test]
    async fn dial_via_proxy_ok_on_200() {
        let addr = fake_proxy("HTTP/1.1 200 Connection Established\r\n\r\n").await;
        dial_via_proxy(&addr.to_string(), "www.google.com", 443).await.expect("200 应视为链路可达");
    }
    #[tokio::test]
    async fn dial_via_proxy_err_on_refusal() {
        let addr = fake_proxy("HTTP/1.1 403 Denied\r\n\r\n").await;
        let r = dial_via_proxy(&addr.to_string(), "www.google.com", 443).await;
        let msg = r.expect_err("非 2xx 必须报错").to_string();
        assert!(msg.contains("403"), "错误应携带拒绝状态行，实际: {msg}");
    }
    #[tokio::test]
    async fn dial_via_proxy_err_on_close_without_response() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { let (s,_)=l.accept().await.unwrap(); drop(s); });
        assert!(dial_via_proxy(&addr.to_string(), "x.com", 443).await.is_err());
    }
    #[test]
    fn test_app_config_set_and_get() {
        let res = app_config_set(serde_json::json!({"auto_proxy": true, "dont_ask": true}));
        assert!(res.is_ok());
        let got = app_config_get();
        assert_eq!(got["auto_proxy"], true);
    }
}
