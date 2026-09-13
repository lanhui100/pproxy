mod proxy;

use std::sync::atomic::{AtomicBool, Ordering as AOrd};
use std::sync::OnceLock;
use tokio::sync::watch;

static TRAY_TOGGLE_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();
static TRAY_MODE_WL_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();
static TRAY_MODE_GB_ITEM: OnceLock<tauri::menu::CheckMenuItem<tauri::Wry>> = OnceLock::new();

#[tauri::command]
fn proxy_prepare_update_exit(app: tauri::AppHandle) -> Result<(), String> {
  log::info!("proxy_prepare_update_exit: 准备更新退出，复原系统代理并清理单实例锁");
  // 1. 复原系统代理，防止安装过渡期系统断网
  if let Err(e) = proxy_disable_inner(app) {
    log::warn!("proxy_prepare_update_exit: proxy_disable failed: {e}");
  }
  // 2. 清理单实例锁，确保新版本安装后自启不会撞锁闪退
  let _ = std::fs::remove_file(data_dir().join("instance.lock"));
  Ok(())
}

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
      let on = ENGINE_ON.load(AOrd::SeqCst);
      let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
      let toggle = CheckMenuItem::with_id(app, "proxy_toggle", if on { "系统代理: 已启用" } else { "系统代理: 已停用" }, true, on, None::<&str>)?;
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
      // 启动自启：默认开启智能模式代理 (auto_proxy 缺省为 true)
      let auto_handle = app.handle().clone();
      tauri::async_runtime::spawn(async move {
        let cfg = app_config_get();
        let auto_proxy = cfg.get("auto_proxy").and_then(|v| v.as_bool()).unwrap_or(true);
        if auto_proxy {
          let res = tauri::async_runtime::spawn_blocking({
            let app = auto_handle.clone();
            move || proxy_enable_inner(app)
          }).await;
          match res {
            Ok(Ok(())) => log::info!("Auto-enabled proxy on startup in smart mode"),
            Ok(Err(e)) => log::info!("Auto-enable proxy skipped or pending config: {e}"),
            Err(e) => log::warn!("Auto-enable task join error: {e}"),
          }
        }
      });
      // 流量统计周期落盘（60s）：进程崩溃最多损失一个周期的计数
      tauri::async_runtime::spawn(async move {
        loop {
          tokio::time::sleep(std::time::Duration::from_secs(60)).await;
          traffic_flush();
        }
      });
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
      proxy_whitelist_get, proxy_whitelist_set, proxy_mode_get, proxy_mode_set,
      proxy_enable, proxy_disable, proxy_pac, proxy_status, proxy_test_sites,
      proxy_tunnel_get, proxy_tunnel_set_url, tunnel_token_save, tunnel_token_clear,
      tunnel_connect_code_import, tunnel_self_check,
      proxy_auto_config_get, proxy_auto_config_set, app_config_get, app_config_set,
      proxy_bypass_hosts,
      proxy_rescue, proxy_import_sync, proxy_mode_switch, proxy_get_current_config,
      proxy_traffic_stats, proxy_test_egress, proxy_test_site_via, proxy_test_site_local,
      proxy_access_url_generate, proxy_api_token_get, proxy_api_token_set,
      open_external_url, proxy_prepare_update_exit,
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
const CREDENTIAL_USER_TUNNEL: &str = "tunnel_token";
const CREDENTIAL_USER_API_TOKEN: &str = "api_proxy_token";
/// 方案 B（chained）远端代理密码的独立凭据槽；凭据按用途分槽，严禁混用。
const CREDENTIAL_USER_PROXY: &str = "proxy_password";

fn cred_entry(user: &str) -> Result<keyring::Entry, String> {
  keyring::Entry::new(CREDENTIAL_SERVICE, user).map_err(|e| format!("keyring entry error: {e}"))
}
#[cfg(debug_assertions)]
fn cred_dev_file(user: &str) -> Option<std::path::PathBuf> {
  std::env::var("PONY_DESKTOP_DEV_FILE_KEYRING").ok().map(|_| std::env::temp_dir().join("pony-desktop-dev-keyring").join(user))
}
fn cred_fallback_file(user: &str) -> std::path::PathBuf {
  data_dir().join(format!(".{user}.dat"))
}

fn cred_set_impl(user: &str, secret: String) -> Result<(), String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) {
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    return std::fs::write(p, secret).map_err(|e| e.to_string());
  }

  // 1. 本地私有目录双重备份（防 Windows Keyring 权限受限/写失败/被外部脏数据覆盖）
  let fallback = cred_fallback_file(user);
  let _ = std::fs::create_dir_all(fallback.parent().unwrap());
  let _ = std::fs::write(&fallback, secret.as_bytes());

  // 2. 写入系统凭据库
  if let Ok(ent) = cred_entry(user) {
    let _ = ent.set_password(&secret);
  }
  Ok(())
}

fn cred_get_impl(user: &str) -> Result<Option<String>, String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { return Ok(std::fs::read_to_string(p).ok()); }

  // 1. 优先读取系统凭据库
  let keyring_val = cred_entry(user).ok().and_then(|ent| ent.get_password().ok());

  // 2. 读取本地私有目录兜底文件
  let fallback_val = std::fs::read_to_string(cred_fallback_file(user)).ok().filter(|s| !s.trim().is_empty());

  // 3. 智能判定：优先使用本地显式写入的兜底备份，若本地无再回退 keyring，
  // 杜绝操作系统 Keyring 遗留的陈旧脏 Token 反向污染！
  match (fallback_val, keyring_val) {
    (Some(f), _) => Ok(Some(f.trim().to_string())),
    (None, Some(k)) => Ok(Some(k.trim().to_string())),
    (None, None) => Ok(None),
  }
}

fn cred_delete_impl(user: &str) -> Result<(), String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { let _=std::fs::remove_file(p); return Ok(()); }

  let fallback = cred_fallback_file(user);
  let _ = std::fs::remove_file(fallback);

  if let Ok(ent) = cred_entry(user) {
    let _ = ent.delete_credential();
  }
  Ok(())
}

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
static UPSTREAM_TX: OnceLock<watch::Sender<Option<proxy::engine::Upstream>>> = OnceLock::new();
fn ensure_upstream_watch() -> &'static watch::Sender<Option<proxy::engine::Upstream>> {
    UPSTREAM_TX.get_or_init(|| {
        let (tx, _rx) = watch::channel(None);
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
/// gate 隧道端点（WS↔TCP 桥）：部署于 gate.example.com/ws（见 deploy/cf-gate-worker/wrangler.toml）。
/// 注意：与 HTTP 数据面网关（edge.example.com，cf-worker）不是同一域名，切勿混用。
const GATE_WS_URL: &str = "wss://gate.example.com/ws";
/// 默认双 gate 端点（主备 failover）：裸 token 保存且无端点配置时自动补齐。
const DEFAULT_TUNNEL_URLS: &str = "wss://vgate.example.com/api/ws,wss://gate.example.com/ws";

/// 旧配置迁移：早期版本把 HTTP 网关域名（edge.example.com）误当作 WS gate 端点，
/// 且曾缺 /ws 路径。读到这类值一律映射到正确的 gate 端点（防止拨测超时/隧道连接失败）。
/// 同时将旧版 CF 在前的默认端点自动迁移为主备双端点，确保 Google/AI API 稳定出网。
/// 单一 CF 端点（纯 gate.example.com）同样补齐默认双端点，避免缺 Vercel 兜底（P2-4）。
fn migrate_tunnel_url(url: &str) -> String {
    let t = url.trim();
    if t.starts_with("wss://edge.example.com") || t.starts_with("ws://edge.example.com") {
        return GATE_WS_URL.to_string();
    }
    if t == "wss://gate.example.com/ws,wss://vgate.example.com/api/ws" {
        return DEFAULT_TUNNEL_URLS.to_string();
    }
    if t == GATE_WS_URL {
        return DEFAULT_TUNNEL_URLS.to_string();
    }
    t.to_string()
}
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
  let url = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url)).unwrap_or_default();
  let (has_token, cred_error) = match cred_get_impl(CREDENTIAL_USER_TUNNEL) {
    Ok(Some(t)) if !t.is_empty() => (true, serde_json::Value::Null),
    Ok(_) => (false, serde_json::Value::Null),
    // 凭据损坏（如外部工具以非 keyring 编码写入）须显式上报，不再静默吞为「未配置」
    Err(e) => (false, serde_json::json!(format!("凭据损坏或编码不兼容（{e}）：请重新粘贴加速授权码"))),
  };
  serde_json::json!({ "url": url, "has_token": has_token, "cred_error": cred_error, "fingerprint": tunnel_token_fingerprint() })
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
  let secret = secret.trim().to_string();
  cred_set_impl(CREDENTIAL_USER_TUNNEL, secret.clone())?;
  // 直发用户输入的 secret（不回读凭据），与 configure_direct_tunnel 同口径：
  // 凭据回读失败（如外部工具以非 keyring 编码写入）不影响本次保存即时生效。
  let (url, _) = tunnel_config_load();
  let url = match url {
    Some(u) => u,
    // 无合法端点配置时补齐默认双 gate（裸 token 粘贴即完成全部配置）
    None => { proxy_tunnel_set_url(DEFAULT_TUNNEL_URLS.to_string())?; DEFAULT_TUNNEL_URLS.to_string() }
  };
  let _ = ensure_tunnel_watch().send((Some(url), Some(secret)));
  Ok(())
}

/// 本机隧道令牌指纹：SHA-256 前 8 位 hex（用于与运维侧/gate 部署核对，不含可爆破材料）。
fn tunnel_token_fingerprint() -> Option<String> {
  use sha2::{Digest, Sha256};
  let t = cred_get_impl(CREDENTIAL_USER_TUNNEL).ok().flatten().filter(|s| !s.is_empty())?;
  let h = Sha256::digest(t.as_bytes());
  Some(hex::encode(&h[..4]))
}

/// 从配置或默认端点中解析指定接口 (cf / vercel) 的 gate URL。
fn resolve_gate_url_for_iface(iface: &str) -> Option<String> {
    let url_raw = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url))
        .unwrap_or_else(|| DEFAULT_TUNNEL_URLS.to_string());
    let urls: Vec<String> = url_raw.split([',', ';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    match iface {
        "vercel" => urls.into_iter().find(|u| u.contains("vercel") || u.contains("vgate")).or_else(|| Some(DEFAULT_TUNNEL_URLS.split(',').next().unwrap_or("").to_string())),
        "cf" => urls.into_iter().find(|u| !u.contains("vercel") && !u.contains("vgate")).or_else(|| Some(DEFAULT_TUNNEL_URLS.split(',').nth(1).unwrap_or(GATE_WS_URL).to_string())),
        _ => None,
    }
}

/// 从 ws/wss/http/https URL 提取 host:port（缺省端口根据 scheme 设为 443 或 80）。
fn extract_host_port_from_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    let (scheme, without_scheme) = if let Some(rest) = s.strip_prefix("wss://") {
        ("wss", rest)
    } else if let Some(rest) = s.strip_prefix("ws://") {
        ("ws", rest)
    } else if let Some(rest) = s.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = s.strip_prefix("http://") {
        ("http", rest)
    } else {
        ("", s)
    };
    let host_part = without_scheme.split('/').next()?.split('?').next()?.split('#').next()?;
    if host_part.is_empty() { return None; }
    if host_part.contains(':') {
        Some(host_part.to_string())
    } else {
        let default_port = if scheme == "ws" || scheme == "http" { 80 } else { 443 };
        Some(format!("{host_part}:{default_port}"))
    }
}

/// 解析 pony-gate:// 连接口令：base64url(JSON {"u": url, "t": token})。
/// 长期有效、无加密（机密性与 token 等同）；与一次性迁移用的 pproxy-sync:// 定位不同。
fn parse_connect_code(code: &str) -> Result<(String, String), String> {
  use base64::Engine as _;
  let encoded = code
    .trim()
    .strip_prefix("pony-gate://")
    .ok_or_else(|| "连接口令须以 pony-gate:// 开头".to_string())?;
  let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .decode(encoded)
    .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded))
    .map_err(|_| "连接口令格式错误（Base64 解码失败）".to_string())?;
  let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "连接口令内容不是合法 JSON".to_string())?;
  // 版本字段：缺省视为 v1；未来字段不兼容时拒绝
  if let Some(vn) = v.get("v").and_then(|x| x.as_u64()) {
    if vn != 1 { return Err(format!("连接口令版本不支持（v{vn}），请更新软件后重试")); }
  }
  let url = v.get("u").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
    .ok_or_else(|| "连接口令缺少端点字段 u".to_string())?;
  let token = v.get("t").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
    .ok_or_else(|| "连接口令缺少令牌字段 t".to_string())?;
  validate_tunnel_url(url)?;
  // 连接口令强制 wss://（明文 ws:// 会让 Bearer token 裸奔，属于投毒入口）
  if url.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).any(|u| !u.starts_with("wss://")) {
    return Err("连接口令的端点必须使用 wss:// 加密端点".into());
  }
  Ok((url.to_string(), token.to_string()))
}

/// 一键导入 pony-gate:// 连接口令：写端点 + 写凭据 + 直发 watch，即时生效（无需重启）。
#[tauri::command]
fn tunnel_connect_code_import(code: String) -> Result<serde_json::Value, String> {
  let (url, token) = parse_connect_code(&code)?;
  proxy_tunnel_set_url(url.clone())?;
  cred_set_impl(CREDENTIAL_USER_TUNNEL, token.clone())?;
  let _ = ensure_tunnel_watch().send((Some(url.clone()), Some(token)));
  let _ = app_config_set(serde_json::json!({
    "mode_type": "direct",
    "configured": true
  }));
  Ok(serde_json::json!({
    "success": true,
    "url": url,
    "fingerprint": tunnel_token_fingerprint(),
    "message": "连接口令已导入，端点与令牌即时生效",
  }))
}

/// 隧道健康自检：本机凭据可读性 + token 指纹 + 逐 gate 实测（WS 升级成功即证明该端哈希与本机 token 一致）。
#[tauri::command]
async fn tunnel_self_check() -> Result<serde_json::Value, String> {
  let cred = cred_get_impl(CREDENTIAL_USER_TUNNEL);
  let (cred_ok, cred_error, token) = match &cred {
    Ok(Some(t)) if !t.is_empty() => (true, serde_json::Value::Null, Some(t.clone())),
    Ok(_) => (false, serde_json::json!("凭据为空：请粘贴加速授权码"), None),
    Err(e) => (false, serde_json::json!(format!("凭据损坏或编码不兼容（{e}）：请重新粘贴加速授权码")), None),
  };
  let url_raw = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok()
    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url))
    .unwrap_or_else(|| GATE_WS_URL.to_string());
  let urls: Vec<String> = url_raw.split([',', ';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
  let mut gates = Vec::new();
  for u in urls {
    let name = if u.contains("vercel") || u.contains("vgate") { "vercel" } else { "cf" };
    let item = match &token {
      Some(t) => match proxy::engine_tunnel::probe_gate_rtt(&u, t).await {
        Ok(ms) => serde_json::json!({ "name": name, "url": u, "ok": true, "ms": ms }),
        Err(e) => serde_json::json!({ "name": name, "url": u, "ok": false, "error": e }),
      },
      None => serde_json::json!({ "name": name, "url": u, "ok": false, "error": "no token" }),
    };
    gates.push(item);
  }
  Ok(serde_json::json!({
    "fingerprint": tunnel_token_fingerprint(),
    "cred_ok": cred_ok,
    "cred_error": cred_error,
    "gates": gates,
  }))
}
#[tauri::command]
fn tunnel_token_clear() -> Result<(), String> {
  cred_delete_impl(CREDENTIAL_USER_TUNNEL)?;
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  Ok(())
}
fn tunnel_config_load() -> (Option<String>, Option<String>) {
  let url = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url)).filter(|u| validate_tunnel_url(u).is_ok());
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
    // 按出网模式装配通道：chained → 远端上游代理；direct → WS gate 隧道
    let cfg_json = app_config_get();
    let mut mode_type = cfg_json.get("mode_type").and_then(|v| v.as_str()).unwrap_or("direct").to_string();
    let raw_host = cfg_json.get("remote_host").and_then(|v| v.as_str()).unwrap_or("").trim();
    if mode_type == "chained"
        && (raw_host.is_empty() || raw_host.starts_with("127.0.0.1:"))
        && cred_get_impl(CREDENTIAL_USER_TUNNEL).ok().flatten().is_some()
        && cred_get_impl(CREDENTIAL_USER_PROXY).ok().flatten().unwrap_or_default().is_empty()
    {
        mode_type = "direct".to_string();
        let _ = app_config_set(serde_json::json!({ "mode_type": "direct" }));
    }
    let upstream = if mode_type == "chained" {
        let raw_host = cfg_json.get("remote_host").and_then(|v| v.as_str()).unwrap_or("").trim();
        if raw_host.is_empty() {
            return Err("远端代理未配置：请先在首页「方案 B」填写服务器地址或粘贴连接口令".into());
        }
        // 容错：剥掉误带的 scheme；缺端口回落 8899
        let bare = raw_host
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let host = if bare.contains(':') { bare.to_string() } else { format!("{bare}:8899") };
        let username = cfg_json.get("username").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let password = cred_get_impl(CREDENTIAL_USER_PROXY).ok().flatten().unwrap_or_default();
        Some(proxy::engine::Upstream { host, username, password })
    } else {
        None
    };
    if upstream.is_none() {
        let (tunnel_url, tunnel_token) = tunnel_config_load();
        if !wl.is_empty() && (tunnel_url.is_none() || tunnel_token.is_none()) {
            // 细分根因：凭据读取失败（编码污染）与「未配置」是两类事故，文案必须区分
            if let Err(e) = cred_get_impl(CREDENTIAL_USER_TUNNEL) {
                return Err(format!("隧道凭据读取失败（{e}）：可能是凭据被外部工具以不兼容编码写入。请在「设置 → 方案 A」重新粘贴加速授权码即可修复，无需重启"));
            }
            return Err("隧道未配置：白名单流量无法出网。请先在「设置 → 方案 A」填写授权码，或在首页粘贴同步口令，再开启总开关".into());
        }
        let _ = ensure_tunnel_watch().send((tunnel_url, tunnel_token));
    }
    {
        let tx = ensure_watch();
        if tx.borrow().clone() != wl { let _ = tx.send(wl.clone()); }
        let _ = ensure_upstream_watch().send(upstream);
    }
    // Engine singleton: reuse existing task if alive (热更新 via watch, 不重复 bind)
    let already_running = ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()).is_some();
    if !already_running {
        let rx = ensure_watch().subscribe();
        let rx_mode = ensure_mode_watch().subscribe();
        let rx_tunnel = ensure_tunnel_watch().subscribe();
        let rx_pool = ensure_tunnel_watch().subscribe();
        let rx_upstream = ensure_upstream_watch().subscribe();
        let stats = get_or_init_engine_stats();
        let rx_clone = rx.clone();
        // 方案 A：待命隧道池（预建 WS，establish 热态首帧）——池持有独立 tunnel watch，
        // 端点/token 变化时自动清池重建
        let pool = proxy::engine_tunnel::TunnelPool::new(rx_pool);
        let handle = tauri::async_runtime::spawn(async move {
            let cfg = proxy::engine::EngineConfig {
                listen_addr: "127.0.0.1:18900".into(),
                whitelist: rx_clone,
                mode: rx_mode,
                tunnel: rx_tunnel,
                upstream: rx_upstream,
                pool,
            };
            if let Err(e) = proxy::engine::run(cfg, stats).await { log::warn!("proxy engine exited: {e}"); }
        });
        *ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    } else {
        // Already running: whitelist 与 tunnel/upstream 已 via watch 更新，无需重 spawn
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
    traffic_flush();
    sync_tray_and_emit(&app, false);
    Ok(())
}
#[tauri::command]
/// 异步化：proxy_enable_inner 含文件 IO 与系统代理注册表操作，同步命令跑在主线程会冻结 UI。
/// spawn_blocking 让其在阻塞线程池执行，前端 invoke 调用方式不变。
async fn proxy_enable(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || proxy_enable_inner(app))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn proxy_disable(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || proxy_disable_inner(app))
        .await
        .map_err(|e| e.to_string())?
}
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
    let sites = ["google.com", "github.com", "x.com", "openai.com", "anthropic.com"];

    let mut tasks = Vec::new();
    for site in sites {
        tasks.push(async move {
            let started = std::time::Instant::now();
            // 单站 10s：CF 托管目标（openai/anthropic）须先吃一次 CF gate 拒绝再 failover
            // 到非 CF 出口，实测全程 ~5.5s 起，4s 预算必然误报超时；测试并行执行，互不拖累。
            let r = tokio::time::timeout(std::time::Duration::from_secs(10), dial_via_proxy(PROXY_ADDR, site, 443)).await;
            let ok = matches!(r, Ok(Ok(())));
            let ms = started.elapsed().as_millis() as u64;
            let error = match &r {
                Err(_) => "timeout".to_string(),
                Ok(Err(e)) => e.to_string(),
                Ok(Ok(())) => String::new(),
            };
            serde_json::json!({"site": site, "ok": ok, "ms": ms, "error": error})
        });
    }

    let results = futures_util::future::join_all(tasks).await;
    Ok(results)
}

// ---- 流量统计（CF / Vercel / Upstream 出口用量）：引擎原子计数 + 周期持久化 ----
static ENGINE_STATS: OnceLock<std::sync::Arc<proxy::engine::EngineStats>> = OnceLock::new();

fn get_or_init_engine_stats() -> std::sync::Arc<proxy::engine::EngineStats> {
    ENGINE_STATS.get_or_init(|| std::sync::Arc::new(proxy::engine::EngineStats::default())).clone()
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct EgressBucket {
    reqs: u64,
    up: u64,
    down: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct TrafficPersist {
    /// 本地日期 YYYY-MM-DD；与今天不一致时：旧「今日」并入 7 日历史后清零（跨天 rollover）
    date: String,
    today_cf: EgressBucket,
    today_vercel: EgressBucket,
    today_upstream: EgressBucket,
    total_cf: EgressBucket,
    total_vercel: EgressBucket,
    total_upstream: EgressBucket,
    /// 已结束的最近若干天（不含今天），最多 6 条；展示时与今日拼成 7 天柱状图
    history: Vec<DayEntry>,
    /// 近 24 小时分桶记录（每小时一条），按时间升序
    hourly: Vec<HourEntry>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct DayEntry {
    date: String,
    cf: EgressBucket,
    vercel: EgressBucket,
    upstream: EgressBucket,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct HourEntry {
    hour: String,
    cf: EgressBucket,
    vercel: EgressBucket,
    upstream: EgressBucket,
}

static TRAFFIC_STATE: std::sync::Mutex<Option<TrafficPersist>> = std::sync::Mutex::new(None);
/// 已并入 TRAFFIC_STATE 的引擎原子读数基线（仅进程内存，绝不持久化——
/// 重启后引擎原子归零，若把旧基线落盘会把新一轮计数全部吞掉）。
type TrafficRawCounters = (u64, u64, u64, u64, u64, u64, u64, u64, u64);
static TRAFFIC_BASE: std::sync::Mutex<Option<TrafficRawCounters>> = std::sync::Mutex::new(None);

fn traffic_path() -> std::path::PathBuf { data_dir().join("traffic.json") }

fn today_str() -> String { chrono::Local::now().format("%Y-%m-%d").to_string() }

fn current_hour_str() -> String { chrono::Local::now().format("%Y-%m-%d %H:00").to_string() }

/// 合并引擎原子计数增量，返回当日 + 累计 + 历史快照（不碰磁盘）。
fn traffic_snapshot() -> TrafficPersist {
    use std::sync::atomic::Ordering::Relaxed;
    let cur = {
        let s = get_or_init_engine_stats();
        (
            s.cf_up.load(Relaxed),
            s.cf_down.load(Relaxed),
            s.cf_reqs.load(Relaxed),
            s.vercel_up.load(Relaxed),
            s.vercel_down.load(Relaxed),
            s.vercel_reqs.load(Relaxed),
            s.upstream_up.load(Relaxed),
            s.upstream_down.load(Relaxed),
            s.upstream_reqs.load(Relaxed),
        )
    };
    let mut guard = TRAFFIC_STATE.lock().unwrap_or_else(|p| p.into_inner());
    let mut st = guard.take().unwrap_or_else(|| {
        std::fs::read_to_string(traffic_path())
            .ok()
            .and_then(|s| serde_json::from_str::<TrafficPersist>(&s).ok())
            .unwrap_or_default()
    });
    let today = today_str();
    if st.date != today {
        // rollover：把旧今日归档（有内容才归档），仅保留最近 6 个已结束天
        if !st.date.is_empty()
            && (st.today_cf.reqs > 0 || st.today_cf.up > 0 || st.today_cf.down > 0
                || st.today_vercel.reqs > 0 || st.today_vercel.up > 0 || st.today_vercel.down > 0
                || st.today_upstream.reqs > 0 || st.today_upstream.up > 0 || st.today_upstream.down > 0)
        {
            st.history.push(DayEntry {
                date: st.date.clone(),
                cf: st.today_cf.clone(),
                vercel: st.today_vercel.clone(),
                upstream: st.today_upstream.clone(),
            });
            let keep = st.history.len().saturating_sub(6);
            if keep > 0 {
                st.history.drain(0..keep);
            }
        }
        st.date = today;
        st.today_cf = EgressBucket::default();
        st.today_vercel = EgressBucket::default();
        st.today_upstream = EgressBucket::default();
    }

    let cur_hour = current_hour_str();
    if st.hourly.last().map(|h| &h.hour) != Some(&cur_hour) {
        st.hourly.push(HourEntry {
            hour: cur_hour,
            cf: EgressBucket::default(),
            vercel: EgressBucket::default(),
            upstream: EgressBucket::default(),
        });
        let keep = st.hourly.len().saturating_sub(24);
        if keep > 0 {
            st.hourly.drain(0..keep);
        }
    }

    let mut base = TRAFFIC_BASE.lock().unwrap_or_else(|p| p.into_inner());
    let b = base.unwrap_or(cur);
    let d = (
        cur.0.saturating_sub(b.0),
        cur.1.saturating_sub(b.1),
        cur.2.saturating_sub(b.2),
        cur.3.saturating_sub(b.3),
        cur.4.saturating_sub(b.4),
        cur.5.saturating_sub(b.5),
        cur.6.saturating_sub(b.6),
        cur.7.saturating_sub(b.7),
        cur.8.saturating_sub(b.8),
    );
    if d != (0, 0, 0, 0, 0, 0, 0, 0, 0) {
        st.today_cf.up += d.0;
        st.today_cf.down += d.1;
        st.today_cf.reqs += d.2;
        st.today_vercel.up += d.3;
        st.today_vercel.down += d.4;
        st.today_vercel.reqs += d.5;
        st.today_upstream.up += d.6;
        st.today_upstream.down += d.7;
        st.today_upstream.reqs += d.8;
        st.total_cf.up += d.0;
        st.total_cf.down += d.1;
        st.total_cf.reqs += d.2;
        st.total_vercel.up += d.3;
        st.total_vercel.down += d.4;
        st.total_vercel.reqs += d.5;
        st.total_upstream.up += d.6;
        st.total_upstream.down += d.7;
        st.total_upstream.reqs += d.8;
        if let Some(last_h) = st.hourly.last_mut() {
            last_h.cf.up += d.0;
            last_h.cf.down += d.1;
            last_h.cf.reqs += d.2;
            last_h.vercel.up += d.3;
            last_h.vercel.down += d.4;
            last_h.vercel.reqs += d.5;
            last_h.upstream.up += d.6;
            last_h.upstream.down += d.7;
            last_h.upstream.reqs += d.8;
        }
    }
    *base = Some(cur);
    let snap = st.clone();
    *guard = Some(st);
    snap
}

/// 把当前快照落盘（tmp + rename，与 app_config 同一防半截写口径）。
fn traffic_flush() {
    let snap = traffic_snapshot();
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("traffic.json.tmp");
    if std::fs::write(&tmp, serde_json::to_string(&snap).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, traffic_path());
    }
}

fn bucket_json(b: &EgressBucket) -> serde_json::Value {
    serde_json::json!({ "requests": b.reqs, "bytes_up": b.up, "bytes_down": b.down })
}

/// CF / Vercel / Upstream 出网用量：今日 / 累计 / 近 7 日 / 近 24 小时（按时间升序）。
#[tauri::command]
fn proxy_traffic_stats() -> serde_json::Value {
    let s = traffic_snapshot();
    let mut history: Vec<serde_json::Value> = s
        .history
        .iter()
        .map(|d| {
            serde_json::json!({
                "date": d.date,
                "cf": bucket_json(&d.cf),
                "vercel": bucket_json(&d.vercel),
                "upstream": bucket_json(&d.upstream),
            })
        })
        .collect();
    history.push(serde_json::json!({
        "date": s.date,
        "cf": bucket_json(&s.today_cf),
        "vercel": bucket_json(&s.today_vercel),
        "upstream": bucket_json(&s.today_upstream),
    }));
    let hourly: Vec<serde_json::Value> = s
        .hourly
        .iter()
        .map(|h| {
            serde_json::json!({
                "hour": h.hour,
                "cf": bucket_json(&h.cf),
                "vercel": bucket_json(&h.vercel),
                "upstream": bucket_json(&h.upstream),
            })
        })
        .collect();
    serde_json::json!({
        "today": {
            "cf": bucket_json(&s.today_cf),
            "vercel": bucket_json(&s.today_vercel),
            "upstream": bucket_json(&s.today_upstream),
        },
        "total": {
            "cf": bucket_json(&s.total_cf),
            "vercel": bucket_json(&s.total_vercel),
            "upstream": bucket_json(&s.total_upstream),
        },
        "history": history,
        "hourly": hourly,
    })
}

/// 出网接口联通性与往返延迟（RTT）拨测：
/// 与下方站点测试保持完全一致的测量口径（单物理往返 RTT，剥离冷建连应用层开销）：
/// - 方案 A（Direct 独立加速）：经对应 gate 隧道热态 WS 发送 Ping 测到边缘节点的纯物理往返延迟（RTT）；
///   若未配置授权码或 WS 建立失败，回退到 gate 端点的 TCP 握手 RTT；
/// - 方案 B（Chained 远端代理）：测到用户自建服务器的真实物理 TCP 握手往返延迟（RTT）。
#[tauri::command]
async fn proxy_test_egress(iface: String) -> Result<serde_json::Value, String> {
    // 方案 B：Chained 远端代理接口
    if iface == "chained" {
        let cfg_json = app_config_get();
        let raw_host = cfg_json.get("remote_host").and_then(|v| v.as_str()).unwrap_or("").trim();
        if raw_host.is_empty() {
            return Err("未配置远端代理服务器地址".into());
        }
        let bare = raw_host
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let host_port = if bare.contains(':') { bare.to_string() } else { format!("{bare}:8899") };

        let started = std::time::Instant::now();
        let dial_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        match tokio::time::timeout_at(dial_deadline, tokio::net::TcpStream::connect(&host_port)).await {
            Ok(Ok(_)) => {
                let ms = started.elapsed().as_millis() as u64;
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": true,
                    "ms": ms,
                }));
            }
            Ok(Err(e)) => {
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": false,
                    "ms": 0,
                    "error": e.to_string(),
                }));
            }
            Err(_) => {
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": false,
                    "ms": 0,
                    "error": "timeout",
                }));
            }
        }
    }

    // 方案 A：Direct 独立中继隧道模式（cf / vercel）
    let gate = resolve_gate_url_for_iface(&iface)
        .ok_or_else(|| "未知接口：仅支持 cf / vercel / chained".to_string())?;

    let token = match cred_get_impl(CREDENTIAL_USER_TUNNEL) {
        Ok(t) => t,
        // 凭据损坏必须显式失败，不得回落 TCP 探测造成「接口假绿」
        Err(e) => {
            return Ok(serde_json::json!({
                "iface": iface,
                "ok": false,
                "ms": 0,
                "error": format!("凭据损坏或编码不兼容（{e}）：请在「设置 → 方案 A」重新粘贴加速授权码"),
            }));
        }
    };
    if let Some(ref tok) = token {
        match proxy::engine_tunnel::probe_gate_rtt(&gate, tok).await {
            Ok(ms) => {
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": true,
                    "ms": ms,
                }));
            }
            Err(e) => {
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": false,
                    "ms": 0,
                    "error": e,
                }));
            }
        }
    }

    // 未配置授权码时，按 TCP 握手 RTT 测试节点连通性
    let host_port = extract_host_port_from_url(&gate).unwrap_or_else(|| match iface.as_str() {
        "vercel" => "vgate.example.com:443".to_string(),
        _ => "gate.example.com:443".to_string(),
    });
    let started = std::time::Instant::now();
    let dial_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    match tokio::time::timeout_at(dial_deadline, tokio::net::TcpStream::connect(host_port)).await {
        Ok(Ok(_)) => Ok(serde_json::json!({
            "iface": iface,
            "ok": true,
            "ms": started.elapsed().as_millis() as u64,
        })),
        Ok(Err(e)) => Ok(serde_json::json!({
            "iface": iface,
            "ok": false,
            "ms": 0,
            "error": e.to_string(),
        })),
        Err(_) => Ok(serde_json::json!({
            "iface": iface,
            "ok": false,
            "ms": 0,
            "error": "timeout",
        })),
    }
}

/// 按选定出网接口（cf / vercel / chained）拨测指定站点：
/// - 方案 A（Direct 独立加速）：经对应 gate 隧道热态长连接测物理往返延迟（RTT）；
/// - 方案 B（Chained 远端代理）：经 TCP 握手测本地到用户自建服务器的真实物理往返延迟（RTT）。
#[tauri::command]
async fn proxy_test_site_via(iface: String, host: String) -> Result<serde_json::Value, String> {    let host = host.trim().trim_start_matches("https://").trim_start_matches("http://")
        .trim_end_matches('/').to_lowercase();
    if host.is_empty()
        || host.len() > 253
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err("非法域名".into());
    }

    // 方案 B：Chained 远端代理接口
    if iface == "chained" {
        let cfg_json = app_config_get();
        let raw_host = cfg_json.get("remote_host").and_then(|v| v.as_str()).unwrap_or("").trim();
        if raw_host.is_empty() {
            return Err("未配置远端代理服务器地址".into());
        }
        let bare = raw_host
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let host_port = if bare.contains(':') { bare.to_string() } else { format!("{bare}:8899") };

        let started = std::time::Instant::now();
        let dial_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        match tokio::time::timeout_at(dial_deadline, tokio::net::TcpStream::connect(&host_port)).await {
            Ok(Ok(_)) => {
                let ms = started.elapsed().as_millis() as u64;
                return Ok(serde_json::json!({
                    "site": host,
                    "iface": "chained",
                    "ok": true,
                    "ms": ms,
                }));
            }
            Ok(Err(e)) => {
                return Ok(serde_json::json!({
                    "site": host,
                    "iface": "chained",
                    "ok": false,
                    "ms": 0,
                    "error": e.to_string(),
                }));
            }
            Err(_) => {
                return Ok(serde_json::json!({
                    "site": host,
                    "iface": "chained",
                    "ok": false,
                    "ms": 0,
                    "error": "timeout",
                }));
            }
        }
    }

    // 方案 A：Direct 独立中继隧道模式（cf / vercel）
    let gate = resolve_gate_url_for_iface(&iface)
        .ok_or_else(|| "未知接口：仅支持 cf / vercel / chained".to_string())?;
    let token = cred_get_impl(CREDENTIAL_USER_TUNNEL)
        .ok()
        .flatten()
        .ok_or_else(|| "未配置授权码：请先在「设置」完善方案 A 配置".to_string())?;
    match proxy::engine_tunnel::probe_via_gate(&gate, &token, &host, 443).await {
        Ok(ms) => Ok(serde_json::json!({
            "site": host,
            "iface": iface,
            "ok": true,
            "ms": ms,
        })),
        Err(e) => Ok(serde_json::json!({
            "site": host,
            "iface": iface,
            "ok": false,
            "ms": 0,
            "error": e,
        })),
    }
}

/// 经本地引擎（127.0.0.1:18900）对指定站点做真实出网拨测（P2-5 修复）：
/// 与 `proxy_test_sites` 同口径 —— 走引擎的 CONNECT 分流，命中白名单/全局则经隧道出网、
/// 未命中则直连，真实反映"前端链接状态 = 实际可用性"，而非绕过引擎直拨 gate。
/// 需代理已启用（ENGINE_ON），未启用时明确报错而不是伪装成功。
#[tauri::command]
async fn proxy_test_site_local(host: String) -> Result<serde_json::Value, String> {
    let host = host.trim().trim_start_matches("https://").trim_start_matches("http://")
        .trim_end_matches('/').to_lowercase();
    if host.is_empty()
        || host.len() > 253
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err("非法域名".into());
    }
    if !ENGINE_ON.load(AOrd::SeqCst) {
        return Ok(serde_json::json!({
            "site": host,
            "iface": "local",
            "ok": false,
            "ms": 0,
            "error": "代理未启用：请先打开系统代理总开关",
        }));
    }
    let started = std::time::Instant::now();
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        dial_via_proxy("127.0.0.1:18900", &host, 443),
    ).await;
    let ok = matches!(r, Ok(Ok(())));
    let ms = started.elapsed().as_millis() as u64;
    let error = match &r {
        Err(_) => "timeout".to_string(),
        Ok(Err(e)) => e.to_string(),
        Ok(Ok(())) => String::new(),
    };
    Ok(serde_json::json!({
        "site": host,
        "iface": "local",
        "ok": ok,
        "ms": ms,
        "error": error,
    }))
}
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
#[tauri::command]
fn proxy_pac() -> String {
    let entries = { if let Some(tx)=WHITELIST_TX.get(){ let g=tx.borrow(); g.clone() } else { load_whitelist_from_file() } };
    let mode = { if let Some(tx)=PROXY_MODE_TX.get(){ *tx.borrow() } else { load_proxy_mode_from_file() } };
    proxy::pac::generate_pac(&entries, mode)
}
#[tauri::command]
fn proxy_auto_config_get() -> serde_json::Value { std::fs::read_to_string(app_config_path()).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).unwrap_or(serde_json::json!({})) }
#[tauri::command]
fn proxy_auto_config_set(patch: serde_json::Value) -> Result<(), String> { app_config_set(patch) }
#[tauri::command]
fn app_config_get() -> serde_json::Value { proxy_auto_config_get() }
static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tauri::command]
fn app_config_set(patch: serde_json::Value) -> Result<(), String> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cur = proxy_auto_config_get();
    if let (Some(map_cur), Some(map_patch)) = (cur.as_object_mut(), patch.as_object()) {
        for (k,v) in map_patch { map_cur.insert(k.clone(), v.clone()); }
    } else { cur = patch; }
    std::fs::write(app_config_path(), serde_json::to_string_pretty(&cur).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
fn proxy_bypass_hosts() -> Vec<String> { proxy::pac::collect_bypass_hosts().into_iter().collect() }

// ---- API 反代地址生成（设置页一键生成接入地址）----
// 语义对齐 README「API 反向代理网关使用」与 crates/cli/src/export.rs：
// 反代地址 = {数据面基址}/{token}/{route}，本地 127.0.0.1:8899、公网 access.example.com。
// 路由由模型提供商 base_url 的主机名自动推导（对齐旧 serviceTemplates / urls.parseServiceUrlInput）。
const PROVIDER_ROUTES: &[(&str, &str)] = &[
    ("api.openai.com", "openai"),
    ("openai.com", "openai"),
    ("api.anthropic.com", "anthropic"),
    ("anthropic.com", "anthropic"),
    ("claude.ai", "anthropic"),
    ("generativelanguage.googleapis.com", "gemini"),
    ("gemini.google.com", "gemini"),
    ("googleapis.com", "gemini"),
    ("api.twitter.com", "x"),
    ("openrouter.ai", "openrouter"),
    ("api.groq.com", "groq"),
    ("groq.com", "groq"),
    ("api.mistral.ai", "mistral"),
    ("mistral.ai", "mistral"),
    ("api.x.ai", "xai"),
    ("x.ai", "xai"),
    ("api.b.ai", "bai"),
    ("b.ai", "bai"),
    ("huggingface.co", "hf"),
    ("github.com", "github"),
    ("ollama.com", "ollama"),
];

/// 安全剥除两端包裹的中英文引号、Markdown 链接语法及多余空白（支持多字节 UTF-8 全角字符）
fn strip_wrapping_quotes_and_space(mut s: &str) -> &str {
    s = s.trim();
    // 兼容 Markdown 链接语法: [title](https://api.openai.com)
    if s.starts_with('[') && s.ends_with(')') {
        if let Some(open_paren) = s.rfind("](") {
            s = &s[open_paren + 2..s.len() - 1];
        }
    }
    loop {
        s = s.trim();
        let mut chars = s.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => break,
        };
        let last = match chars.last().or(Some(first)) {
            Some(c) => c,
            None => break,
        };
        let matched = match (first, last) {
            ('"', '"') | ('\'', '\'') | ('`', '`') => true,
            ('“', '”') | ('‘', '’') => true,
            ('「', '」') | ('『', '』') => true,
            ('＂', '＂') | ('＇', '＇') => true,
            _ => false,
        };
        if matched && s.len() >= first.len_utf8() + last.len_utf8() {
            s = &s[first.len_utf8()..s.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    s.trim()
}

/// 规范化模型提供商 base_url，同时提取 Host（小写、端口保留）与请求子路径（如 /v1、/v1/chat/completions；无子路径为空串）。
fn normalize_provider_base_url_and_path(raw: &str) -> Option<(String, String)> {
    let clean = strip_wrapping_quotes_and_space(raw);
    if clean.is_empty() {
        return None;
    }
    let mut s = clean.to_string();
    if s.starts_with("//") {
        s = s[2..].to_string();
    }
    let has_scheme = s.len() >= 3 && {
        let scheme_end = s.find("://").map(|i| i + 3).unwrap_or(0);
        scheme_end > 0
            && s[..scheme_end - 3].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && s[..scheme_end - 3].chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
    };
    if has_scheme {
        let u = url::Url::parse(&s).ok()?;
        let h = u.host_str()?;
        let host = match u.port() {
            Some(p) if h.contains(':') => format!("[{h}]:{p}"),
            Some(p) => format!("{h}:{p}"),
            None => h.to_string(),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase().trim_end_matches('.').to_string();
        if host.is_empty() {
            return None;
        }
        let p = u.path();
        let subpath = if p == "/" || p.is_empty() {
            String::new()
        } else {
            p.trim_end_matches('/').to_string()
        };
        return Some((host, subpath));
    }

    // 无 scheme 裸输入（如 api.openai.com 或 api.b.ai/v1）
    let (bare_host_part, rest_path) = match s.split_once('/') {
        Some((h, p)) => (h, p),
        None => (s.as_str(), ""),
    };
    let bare_host = bare_host_part.split(['?', '#']).next().unwrap_or("").trim_end_matches('.');
    if bare_host.is_empty() || !bare_host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']')) {
        return None;
    }
    let host = bare_host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase().trim_end_matches('.').to_string();
    if host.is_empty() {
        return None;
    }
    let clean_path = rest_path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
    let subpath = if clean_path.is_empty() {
        String::new()
    } else {
        format!("/{clean_path}")
    };
    Some((host, subpath))
}

#[cfg(test)]
fn normalize_provider_base_url(raw: &str) -> Option<String> {
    normalize_provider_base_url_and_path(raw).map(|(h, _)| h)
}

/// 由 provider host 推导服务路由名（须满足服务端校验 `^[a-z][a-z0-9_-]{0,63}$` 且非 pony_ 前缀）。
fn infer_route(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    // 1. 本地 Ollama 实例优先（端口 11434 或 localhost）
    if host.contains("11434") || host == "localhost" || host.starts_with("localhost:") {
        return Some("ollama".into());
    }

    // 2. 剥离端口号与末尾点，获得纯净域名（解决带 :443 等端口导致 PROVIDER_ROUTES 穿透失配问题）
    let host_without_port = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped)
    } else if let Some((h, port)) = host.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            &host
        }
    } else {
        &host
    };
    let clean_host = host_without_port.trim_end_matches('.');
    if clean_host.is_empty() {
        return None;
    }

    // 3. 静态白名单匹配（精确匹配 + 后缀匹配）
    for (suffix, route) in PROVIDER_ROUTES {
        if clean_host == *suffix || clean_host.ends_with(&format!(".{suffix}")) {
            return Some((*route).to_string());
        }
    }

    // 4. 通用降级推导：取首个非泛化标签（api/v1/gateway/proxy/ai 跳过，对齐旧 parseServiceUrlInput）
    let labels: Vec<&str> = clean_host.split('.').collect();
    let candidate = if labels.len() >= 3 {
        let first = labels[0];
        if ["api", "v1", "gateway", "proxy", "ai"].contains(&first) {
            // 特殊处理单字母主体 + ai 域名，如 api.b.ai -> bai
            if labels[1].len() == 1 && labels[1].chars().all(|c| c.is_ascii_alphabetic()) && labels[2] == "ai" {
                format!("{}{}", labels[1], labels[2])
            } else {
                labels[1].to_string()
            }
        } else {
            first.to_string()
        }
    } else if labels.len() == 2 && labels[0].len() == 1 && labels[0].chars().all(|c| c.is_ascii_alphabetic()) && labels[1] == "ai" {
        // 如 b.ai -> bai, x.ai -> xai
        format!("{}{}", labels[0], labels[1])
    } else {
        labels[0].to_string()
    };

    // 5. 路由合法性过滤：严格对齐服务端 `^[a-z][a-z0-9_-]{0,63}$` 且非 pony_ 前缀
    let mut route: String = candidate
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    route = route.trim_matches(|c| c == '-' || c == '_').to_string();
    if route.is_empty() {
        return None;
    }
    // 强制以小写字母开头（若纯数字或以数字开头如 01.ai，统一垫付前缀 x）
    if !route.starts_with(|c: char| c.is_ascii_lowercase()) {
        route = format!("x{route}");
    }
    // 防御保留字前缀 pony_
    if route.starts_with("pony_") {
        route = format!("x{route}");
    }
    if route.len() > 63 {
        route.truncate(63);
    }
    Some(route)
}

/// 构建反代地址 JSON（纯函数，便于单测）：route 与 subpath 由调用方推导，token 为 None 时用 <token> 占位。
fn build_access_urls(route: &str, subpath: &str, token: Option<&str>) -> serde_json::Value {
    let token_seg = token.unwrap_or("<token>");
    let local_url = format!("http://127.0.0.1:8899/{token_seg}/{route}{subpath}");
    let public_url = format!("https://access.example.com/{token_seg}/{route}{subpath}");
    serde_json::json!({
        "local_url": local_url,
        "public_url": public_url,
        "route": route,
        "has_token": token.is_some(),
        "token": token,
    })
}

/// 获取有效的 API 反代数据面令牌：
/// 1. 优先从专用凭据 CREDENTIAL_USER_API_TOKEN 读取（须以 pony_ 开头）；
/// 2. 其次检查 CREDENTIAL_USER_TUNNEL 是否也是 pony_ 开头的网关令牌；
/// 3. 若无或非 pony_ 开头（如 gate_ 出海隧道码），返回 None。
fn get_api_proxy_token_impl() -> Option<String> {
    if let Ok(Some(tok)) = cred_get_impl(CREDENTIAL_USER_API_TOKEN) {
        let trimmed = tok.trim();
        if !trimmed.is_empty() && trimmed.starts_with("pony_") {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(Some(tok)) = cred_get_impl(CREDENTIAL_USER_TUNNEL) {
        let trimmed = tok.trim();
        if trimmed.starts_with("pony_") {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// 保存 API 反代数据面专用令牌（须以 pony_ 开头）
fn set_api_proxy_token_impl(token: &str) -> Result<(), String> {
    let t = token.trim();
    if t.is_empty() {
        let _ = cred_delete_impl(CREDENTIAL_USER_API_TOKEN);
        return Ok(());
    }
    if !t.starts_with("pony_") {
        return Err("API 反代数据面令牌必须以 pony_ 开头（例如 pony_31abc...）".into());
    }
    cred_set_impl(CREDENTIAL_USER_API_TOKEN, t.to_string())
}

#[tauri::command]
fn proxy_api_token_get() -> Result<Option<String>, String> {
    Ok(get_api_proxy_token_impl())
}

#[tauri::command]
fn proxy_api_token_set(token: String) -> Result<(), String> {
    set_api_proxy_token_impl(&token)
}

/// 生成 API 反代接入地址：输入模型提供商 base_url（带不带 https:// 均可），
/// 自动推导服务路由并保留原始子路径（如 /v1），并使用本机保存的 pony_ 反代数据面授权码，
/// 生成本地（127.0.0.1:8899）与公网（access.example.com）两条反代地址。
/// 若传入 custom_token（且非空），将校验并持久化到本地 API 反代凭据库。
#[tauri::command]
fn proxy_access_url_generate(base_url: String, custom_token: Option<String>) -> Result<serde_json::Value, String> {
    let (host, subpath) = normalize_provider_base_url_and_path(&base_url)
        .ok_or_else(|| "无法识别的模型提供商地址，请粘贴形如 https://api.anthropic.com 的 base_url".to_string())?;
    let route = infer_route(&host)
        .ok_or_else(|| format!("无法从 {host} 推导服务路由，请检查地址"))?;

    if let Some(ref ct) = custom_token {
        let trimmed = ct.trim();
        if !trimmed.is_empty() {
            set_api_proxy_token_impl(trimmed)?;
        }
    }

    let token = get_api_proxy_token_impl();
    Ok(build_access_urls(&route, &subpath, token.as_deref()))
}

/// 在默认浏览器中打开外部链接
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    // 严格校验：仅接受 http/https 且不含任何 cmd 元字符（& | < > ^ 空格 引号 百分号），
    // 否则经 `cmd /C start` 执行时会被二次解析成命令分隔，构成命令注入（对抗审核发现）。
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("非法 URL 协议".into());
    }
    if url.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '"' | ' ' | '%')) {
        return Err("URL 包含不允许的字符".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW = 0x08000000
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        std::process::Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        std::process::Command::new("xdg-open").arg(&url).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 终极网络急救箱：一键无条件恢复直连
#[tauri::command]
fn proxy_rescue(app: tauri::AppHandle) -> Result<String, String> {
    proxy::sysproxy::rescue_network()?;
    ENGINE_ON.store(false, AOrd::SeqCst);
    *SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()) = None;
    sync_tray_and_emit(&app, false);
    Ok("网络已成功急救修复：已完全清除所有系统代理与 PAC 关联，网络已恢复直连。".to_string())
}

/// 获取当前代理配置与状态
#[tauri::command]
fn proxy_get_current_config() -> serde_json::Value {
    let cfg = app_config_get();
    let mode_type = cfg.get("mode_type").and_then(|v| v.as_str()).unwrap_or("direct");
    let configured = cfg.get("configured").and_then(|v| v.as_bool()).unwrap_or(false);
    let worker_url = cfg.get("worker_url").and_then(|v| v.as_str()).unwrap_or("");
    let remote_host = cfg.get("remote_host").and_then(|v| v.as_str()).unwrap_or("");
    let username = cfg.get("username").and_then(|v| v.as_str()).unwrap_or("");
    let has_secret = cred_get_impl(CREDENTIAL_USER_TUNNEL).ok().flatten().is_some();

    serde_json::json!({
        "mode_type": mode_type,
        "configured": configured,
        "worker_url": worker_url,
        "remote_host": remote_host,
        "username": username,
        "has_secret": has_secret
    })
}

fn configure_direct_tunnel(_worker_url: &str, secret: &str) -> Result<(), String> {
    // 方案 A 独立隧道端点：优先沿用已有合法配置；无配置时使用默认双 gate 端点
    let (existing_url, _) = tunnel_config_load();
    let ws_url = match existing_url {
        Some(u) if !u.is_empty() => u,
        _ => DEFAULT_TUNNEL_URLS.to_string(),
    };

    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("tunnel.json.tmp");
    let target = dir.join(TUNNEL_FILE);
    let _ = std::fs::write(&tmp, serde_json::to_string(&serde_json::json!({ "url": ws_url })).unwrap_or_default());
    let _ = std::fs::rename(&tmp, target);

    let _ = cred_set_impl(CREDENTIAL_USER_TUNNEL, secret.to_string());
    let _ = ensure_tunnel_watch().send((Some(ws_url), Some(secret.to_string())));
    Ok(())
}

/// 切换模式 A（个人独立直连）与模式 B（连接远端代理）
#[tauri::command]
fn proxy_mode_switch(mode_type: String, config: serde_json::Value) -> Result<(), String> {
    let mut patch = serde_json::json!({
        "mode_type": mode_type,
        "configured": true
    });
    if mode_type == "chained" {
        if let Some(host) = config.get("remote_host").and_then(|v| v.as_str()) {
            patch["remote_host"] = serde_json::json!(host);
        }
        if let Some(user) = config.get("username").and_then(|v| v.as_str()) {
            patch["username"] = serde_json::json!(user);
        }
        if let Some(pass) = config.get("password").and_then(|v| v.as_str()) {
            let _ = cred_set_impl(CREDENTIAL_USER_PROXY, pass.to_string());
        }
    } else if mode_type == "direct" {
        let worker = config.get("worker_url").and_then(|v| v.as_str()).unwrap_or("https://edge.example.com");
        patch["worker_url"] = serde_json::json!(worker);
        if let Some(sec) = config.get("proxy_secret").and_then(|v| v.as_str()) {
            let _ = configure_direct_tunnel(worker, sec);
        }
    }
    app_config_set(patch)
}

static USED_SYNC_NONCES: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);

/// 跨端口令一键导入（支持 pproxy-sync:// 与 pproxy:// 协议；pony-gate:// 转发连接口令导入）
#[tauri::command]
fn proxy_import_sync(sync_uri: String, passphrase: Option<String>) -> Result<serde_json::Value, String> {
    let raw = sync_uri.trim();
    // pony-gate:// 连接口令统一路由（防止用户粘错入口得到「未知口令」类报错）
    if raw.starts_with("pony-gate://") {
        return tunnel_connect_code_import(raw.to_string());
    }
    if raw.starts_with("pproxy-sync://") {
        use base64::Engine as _;
        let encoded = raw.strip_prefix("pproxy-sync://").unwrap_or(raw);
        // 开源安全整改（2026-09 审计）：移除公开默认口令。pproxy-sync:// 口令由导出方
        // 生成（CLI 无口令导出时会生成 128-bit 随机 Passkey），导入必须显式提供，禁止静态回退。
        let pass = passphrase
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                "导入 pproxy-sync:// 口令需要同步口令（导出时生成的随机 Passkey），请在口令输入框填写".to_string()
            })?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded))
            .map_err(|_| "无效的同步口令格式（Base64 解码失败）".to_string())?;
        if bytes.len() < 28 {
            return Err("同步口令损坏或过短".into());
        }
        let salt = &bytes[..16];
        let nonce = &bytes[16..28];
        let cipher_data = &bytes[28..];

        use sha2::{Digest, Sha256};
        let mut current = Sha256::digest(format!("{}:{}", hex::encode(salt), pass).as_bytes());
        for _ in 1..10_000 {
            let mut h = Sha256::new();
            h.update(current);
            h.update(salt);
            h.update(pass.as_bytes());
            current = h.finalize();
        }
        use chacha20poly1305::aead::{Aead, KeyInit};
        let cipher = chacha20poly1305::ChaCha20Poly1305::new_from_slice(&current).map_err(|e| e.to_string())?;
        let decrypted = cipher.decrypt(chacha20poly1305::Nonce::from_slice(nonce), cipher_data)
            .map_err(|_| "解密失败：同步口令错误、密码不匹配或已被篡改".to_string())?;
        let payload: serde_json::Value = serde_json::from_slice(&decrypted).map_err(|e| e.to_string())?;

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        if let Some(exp) = payload.get("exp").and_then(|v| v.as_u64()) {
            if now > exp {
                return Err("该同步口令已过期（超过 10 分钟），请在原设备重新导出！".into());
            }
        }

        // Nonce 防重放检查
        if let Some(nonce_str) = payload.get("nonce").and_then(|v| v.as_str()) {
            let mut lock = USED_SYNC_NONCES.lock().unwrap();
            let set = lock.get_or_insert_with(std::collections::HashSet::new);
            if set.contains(nonce_str) {
                return Err("安全拦截：该同步口令已被使用过（防重放），请重新导出生成！".into());
            }
            set.insert(nonce_str.to_string());
        }

        let data = payload.get("data").cloned().unwrap_or_default();
        let server_url = data.get("server_url").and_then(|v| v.as_str()).unwrap_or("http://127.0.0.1:8899");
        let worker_url = data.get("worker_url").and_then(|v| v.as_str()).unwrap_or("https://edge.example.com");
        let proxy_secret = data.get("proxy_secret").and_then(|v| v.as_str());

        let patch = serde_json::json!({
            "mode_type": "direct",
            "server_url": server_url,
            "worker_url": worker_url,
            "configured": true
        });
        let _ = app_config_set(patch);
        if let Some(sec) = proxy_secret {
            let _ = configure_direct_tunnel(worker_url, sec);
        }
        Ok(serde_json::json!({
            "success": true,
            "mode": "direct",
            "message": "跨端同步成功！已自动切换为独立加速模式。"
        }))
    } else if raw.starts_with("pproxy://") || raw.starts_with("http://") {
        let cleaned = raw.strip_prefix("pproxy://").or_else(|| raw.strip_prefix("http://")).unwrap();
        let (auth, host_port) = cleaned.split_once('@').ok_or_else(|| "连接口令格式错误，缺少 @".to_string())?;
        let (username, password) = auth.split_once(':').ok_or_else(|| "连接口令格式错误，缺少密码".to_string())?;
        let patch = serde_json::json!({
            "mode_type": "chained",
            "remote_host": host_port,
            "username": username,
            "configured": true
        });
        let _ = app_config_set(patch);
        let _ = cred_set_impl(CREDENTIAL_USER_PROXY, password.to_string());
        Ok(serde_json::json!({
            "success": true,
            "mode": "chained",
            "remote_host": host_port,
            "username": username,
            "message": format!("已成功连接远端代理服务器 ({host_port})！")
        }))
    } else {
        Err("无法识别的口令格式，请粘贴以 pproxy-sync:// 或 pproxy:// 开头的有效口令".into())
    }
}

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
    fn test_app_config_and_proxy_mode_defaults() {
        // 验证 auto_proxy 与 proxy_mode 的缺省逻辑
        let res = app_config_set(serde_json::json!({"auto_proxy": true, "proxy_mode": "whitelist"}));
        assert!(res.is_ok());
        let got = app_config_get();
        assert_eq!(got["auto_proxy"], true);
        assert_eq!(load_proxy_mode_from_file(), proxy::pac::ProxyMode::Whitelist);

        // 验证 ProxyMode 解析回退
        let mode_parsed: proxy::pac::ProxyMode = "unknown".parse().unwrap_or(proxy::pac::ProxyMode::Whitelist);
        assert_eq!(mode_parsed, proxy::pac::ProxyMode::Whitelist);
        let mode_global: proxy::pac::ProxyMode = "global".parse().unwrap_or(proxy::pac::ProxyMode::Whitelist);
        assert_eq!(mode_global, proxy::pac::ProxyMode::Global);

        // 验证 auto_proxy 在未显式设为 false 时默认解析为 true
        let empty_cfg = serde_json::json!({});
        let auto_proxy = empty_cfg.get("auto_proxy").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(auto_proxy);

        let disabled_cfg = serde_json::json!({"auto_proxy": false});
        let auto_proxy_disabled = disabled_cfg.get("auto_proxy").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!auto_proxy_disabled);
    }

    #[tokio::test]
    async fn test_proxy_test_site_via_chained_mode() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let _ = socket.write_all(b"OK").await;
            }
        });

        let _ = app_config_set(serde_json::json!({
            "mode_type": "chained",
            "remote_host": addr.to_string(),
        }));

        let res = proxy_test_site_via("chained".into(), "google.com".into()).await.unwrap();
        assert_eq!(res["ok"], true);
        assert_eq!(res["iface"], "chained");
        assert!(res["ms"].as_u64().is_some());

        // 还原回 direct
        let _ = app_config_set(serde_json::json!({
            "mode_type": "direct"
        }));
    }

    #[test]
    fn migrate_tunnel_url_maps_old_gate_to_correct_endpoint() {
        // 旧配置把 HTTP 网关域名当 gate 用（edge.example.com，可能缺 /ws）→ 必须迁移到 gate.example.com/ws
        assert_eq!(migrate_tunnel_url("wss://edge.example.com"), "wss://gate.example.com/ws");
        assert_eq!(migrate_tunnel_url("wss://edge.example.com/ws"), "wss://gate.example.com/ws");
        assert_eq!(migrate_tunnel_url("ws://edge.example.com"), "wss://gate.example.com/ws");
        // 正确端点与自定义端点保持原样
        assert_eq!(migrate_tunnel_url("wss://gate.example.com/ws,wss://vgate.example.com/api/ws"), "wss://vgate.example.com/api/ws,wss://gate.example.com/ws");
        assert_eq!(migrate_tunnel_url("wss://self-host.example.com/tunnel"), "wss://self-host.example.com/tunnel");
        // P2-4：单 CF 端点补齐默认双端点（Vercel 兜底）
        assert_eq!(migrate_tunnel_url("wss://gate.example.com/ws"), "wss://vgate.example.com/api/ws,wss://gate.example.com/ws");
    }

    // ---- pony-gate:// 连接口令解析 ----
    fn make_code(url: &str, token: &str) -> String {
        use base64::Engine as _;
        let j = serde_json::json!({ "u": url, "t": token }).to_string();
        format!("pony-gate://{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(j))
    }
    #[test]
    fn connect_code_parses_valid_payload() {
        let (u, t) = parse_connect_code(&make_code("wss://gate.example.com/ws,wss://vgate.example.com/api/ws", "tok-123")).unwrap();
        assert_eq!(u, "wss://gate.example.com/ws,wss://vgate.example.com/api/ws");
        assert_eq!(t, "tok-123");
    }
    #[test]
    fn connect_code_accepts_standard_base64_and_whitespace() {
        use base64::Engine as _;
        let j = serde_json::json!({ "u": "wss://gate.example.com/ws", "t": "abc" }).to_string();
        let code = format!("  pony-gate://{}  ", base64::engine::general_purpose::STANDARD.encode(j));
        let (u, t) = parse_connect_code(&code).unwrap();
        assert_eq!(u, "wss://gate.example.com/ws");
        assert_eq!(t, "abc");
    }
    #[test]
    fn connect_code_rejects_bad_inputs() {
        assert!(parse_connect_code("pony-gate://!!!not-base64!!!").is_err());
        assert!(parse_connect_code("pproxy-sync://xxxx").unwrap_err().contains("pony-gate://"));
        // 缺字段
        use base64::Engine as _;
        for payload in [
            serde_json::json!({ "t": "abc" }).to_string(),
            serde_json::json!({ "u": "wss://x/ws" }).to_string(),
            serde_json::json!({ "u": "", "t": "" }).to_string(),
        ] {
            let code = format!("pony-gate://{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload));
            assert!(parse_connect_code(&code).is_err(), "应拒绝: {payload}");
        }
        // 非法端点
        let bad = format!("pony-gate://{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({ "u": "https://evil.com", "t": "abc" }).to_string()));
        assert!(parse_connect_code(&bad).is_err());
    }

    // ---- API 反代地址生成（proxy_access_url_generate）----
    #[test]
    fn normalize_provider_base_url_handles_scheme_path_and_quotes() {
        // 带 https://
        assert_eq!(normalize_provider_base_url("https://api.anthropic.com"), Some("api.anthropic.com".into()));
        // 不带 scheme
        assert_eq!(normalize_provider_base_url("api.openai.com"), Some("api.openai.com".into()));
        // 带路径 / 查询 / 尾斜杠
        assert_eq!(normalize_provider_base_url("https://api.openai.com/v1/chat/completions"), Some("api.openai.com".into()));
        assert_eq!(normalize_provider_base_url("http://api.openai.com/v1?x=1"), Some("api.openai.com".into()));
        assert_eq!(normalize_provider_base_url("https://api.anthropic.com/"), Some("api.anthropic.com".into()));
        // 包裹引号 + 首尾空白
        assert_eq!(normalize_provider_base_url("  \"https://api.groq.com\"  "), Some("api.groq.com".into()));
        assert_eq!(normalize_provider_base_url("  \" https://api.groq.com \"  "), Some("api.groq.com".into()));
        // 中文全角引号
        assert_eq!(normalize_provider_base_url("“https://api.openai.com”"), Some("api.openai.com".into()));
        assert_eq!(normalize_provider_base_url("‘https://api.anthropic.com’"), Some("api.anthropic.com".into()));
        assert_eq!(normalize_provider_base_url("「https://api.mistral.ai」"), Some("api.mistral.ai".into()));
        // Markdown 链接语法
        assert_eq!(normalize_provider_base_url("[OpenAI](https://api.openai.com/v1)"), Some("api.openai.com".into()));
        // 末尾根域名点（FQDN）
        assert_eq!(normalize_provider_base_url("https://api.openai.com./v1"), Some("api.openai.com".into()));
        assert_eq!(normalize_provider_base_url("api.openai.com."), Some("api.openai.com".into()));
        // 大写 host 归一为小写
        assert_eq!(normalize_provider_base_url("HTTPS://API.OPENAI.COM"), Some("api.openai.com".into()));
        // 协议相对 //host/path
        assert_eq!(normalize_provider_base_url("//api.groq.com/v1"), Some("api.groq.com".into()));
        // 带端口（ollama 本地）
        assert_eq!(normalize_provider_base_url("http://127.0.0.1:11434/v1"), Some("127.0.0.1:11434".into()));
        // 非法输入
        assert_eq!(normalize_provider_base_url(""), None);
        assert_eq!(normalize_provider_base_url("   "), None);
        assert_eq!(normalize_provider_base_url("https://"), None);
        assert_eq!(normalize_provider_base_url("hello world"), None);
    }

    #[test]
    fn infer_route_maps_known_providers_and_falls_back() {
        assert_eq!(infer_route("api.anthropic.com").unwrap(), "anthropic");
        assert_eq!(infer_route("api.openai.com").unwrap(), "openai");
        assert_eq!(infer_route("generativelanguage.googleapis.com").unwrap(), "gemini");
        assert_eq!(infer_route("api.groq.com").unwrap(), "groq");
        assert_eq!(infer_route("openrouter.ai").unwrap(), "openrouter");
        assert_eq!(infer_route("api.mistral.ai").unwrap(), "mistral");
        assert_eq!(infer_route("api.x.ai").unwrap(), "xai");
        assert_eq!(infer_route("huggingface.co").unwrap(), "hf");
        assert_eq!(infer_route("api.twitter.com").unwrap(), "x");
        assert_eq!(infer_route("127.0.0.1:11434").unwrap(), "ollama");
        assert_eq!(infer_route("localhost").unwrap(), "ollama");
        assert_eq!(infer_route("localhost:11434").unwrap(), "ollama");

        // 短主体 + ai 顶级域自动推导（如 b.ai / api.b.ai -> bai）
        assert_eq!(infer_route("api.b.ai").unwrap(), "bai");
        assert_eq!(infer_route("b.ai").unwrap(), "bai");

        // 带显式端口输入仍能命中静态已知表（防 :443 穿透失配）
        assert_eq!(infer_route("api.openai.com:443").unwrap(), "openai");
        assert_eq!(infer_route("generativelanguage.googleapis.com:443").unwrap(), "gemini");
        assert_eq!(infer_route("claude.ai:443").unwrap(), "anthropic");
        assert_eq!(infer_route("huggingface.co:443").unwrap(), "hf");
        assert_eq!(infer_route("api.twitter.com:443").unwrap(), "x");
        assert_eq!(infer_route("api.x.ai:443").unwrap(), "xai");

        // 泛化前缀跳过（对齐旧 parseServiceUrlInput）
        assert_eq!(infer_route("api.custom-proxy.example.com").unwrap(), "custom-proxy");
        assert_eq!(infer_route("custom.example.com").unwrap(), "custom");

        // 数字开头域名合规垫付（01.ai -> x01）
        assert_eq!(infer_route("api.01.ai").unwrap(), "x01");
        assert_eq!(infer_route("01.ai").unwrap(), "x01");

        // 下划线连字符清理
        assert_eq!(infer_route("_custom-llm.example.com").unwrap(), "custom-llm");

        // 非法字符过滤与 pony_ 前缀规避
        assert_eq!(infer_route("pony_mirror.example.com").unwrap(), "xpony_mirror");
        assert!(infer_route("###").is_none());
    }

    #[test]
    fn build_access_urls_placeholder_and_token_and_subpath() {
        // 无凭据且无 subpath：<token> 占位
        let res = build_access_urls("anthropic", "", None);
        assert_eq!(res["route"], "anthropic");
        assert_eq!(res["local_url"], "http://127.0.0.1:8899/<token>/anthropic");
        assert_eq!(res["public_url"], "https://access.example.com/<token>/anthropic");
        assert_eq!(res["has_token"], false);

        // 有凭据且带 subpath（如 /v1）：完整保留
        let res2 = build_access_urls("bai", "/v1", Some("pony_31abc"));
        assert_eq!(res2["local_url"], "http://127.0.0.1:8899/pony_31abc/bai/v1");
        assert_eq!(res2["public_url"], "https://access.example.com/pony_31abc/bai/v1");
        assert_eq!(res2["has_token"], true);
    }

    #[test]
    fn access_url_generate_derives_route_and_subpath_and_custom_token() {
        std::env::set_var("PONY_DESKTOP_DEV_FILE_KEYRING", "1");
        // 测试 api.b.ai/v1 正向用例：推导为 bai 并保留 /v1
        let res = proxy_access_url_generate("api.b.ai/v1".into(), Some("pony_31abcbd448a003be0ea27524d60973d8".into())).unwrap();
        assert_eq!(res["route"], "bai");
        assert_eq!(res["local_url"], "http://127.0.0.1:8899/pony_31abcbd448a003be0ea27524d60973d8/bai/v1");
        assert_eq!(res["public_url"], "https://access.example.com/pony_31abcbd448a003be0ea27524d60973d8/bai/v1");
        assert_eq!(res["has_token"], true);

        // 带 https:// 与尾斜杠的输入同样正确处理
        let res2 = proxy_access_url_generate("https://api.openai.com/v1/".into(), None).unwrap();
        assert_eq!(res2["route"], "openai");
        assert!(res2["local_url"].as_str().unwrap().ends_with("/openai/v1"));

        // 非法输入明确报错
        assert!(proxy_access_url_generate("   ".into(), None).is_err());
        assert!(proxy_access_url_generate("hello world".into(), None).is_err());
        std::env::remove_var("PONY_DESKTOP_DEV_FILE_KEYRING");
    }

    #[test]
    fn resolve_gate_url_and_extract_host_port() {
        assert_eq!(extract_host_port_from_url("wss://custom.gate.io:8443/ws"), Some("custom.gate.io:8443".to_string()));
        assert_eq!(extract_host_port_from_url("wss://custom.gate.io/ws"), Some("custom.gate.io:443".to_string()));
        assert_eq!(extract_host_port_from_url("ws://127.0.0.1:8787/ws"), Some("127.0.0.1:8787".to_string()));
        assert_eq!(extract_host_port_from_url("ws://127.0.0.1/ws"), Some("127.0.0.1:80".to_string()));
        assert_eq!(extract_host_port_from_url("invalid url"), Some("invalid url:443".to_string()));

        // 默认无配置时 fallback 到默认端点
        assert!(resolve_gate_url_for_iface("cf").unwrap().contains("gate.example.com"));
        assert!(resolve_gate_url_for_iface("vercel").unwrap().contains("vgate.example.com"));
        assert_eq!(resolve_gate_url_for_iface("unknown"), None);
    }
}
