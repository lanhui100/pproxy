mod proxy;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_http::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .setup(|app| {
      use tauri::menu::{Menu, MenuItem};
      use tauri::tray::TrayIconBuilder;
      let on = MenuItem::with_id(app, "proxy_on", "启用代理", true, None::<&str>)?;
      let off = MenuItem::with_id(app, "proxy_off", "停用代理", true, None::<&str>)?;
      let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
      let menu = Menu::with_items(app, &[&on, &off, &quit])?;
      TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Pony Proxy")
        .menu(&menu)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "proxy_on" => { let _ = proxy_enable(); }
            "proxy_off" => { let _ = proxy_disable(); }
            "quit" => { let _ = proxy_disable(); app.exit(0); }
            _ => {}
        })
        .build(app)?;
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      credential_get,
      credential_set,
      credential_delete,
      proxy_whitelist_get,
      proxy_whitelist_set,
      proxy_enable,
      proxy_disable,
      proxy_pac,
      proxy_status,
      proxy_test_sites,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

/// admin token 存取（M5 §6.4）：仅经 OS 凭据库，前端只持引用句柄。
/// WHY 不落 localStorage/配置文件：凭据纪律（禁止明文落盘）；Windows 走
/// Credential Manager，dev fallback 文件模式仅在 debug 构建显式 env 触发。
const CREDENTIAL_SERVICE: &str = "pony-desktop";
const CREDENTIAL_USER: &str = "admin_token";

fn entry() -> Result<keyring::Entry, String> {
  keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
    .map_err(|e| format!("keyring entry error: {e}"))
}

#[tauri::command]
fn credential_set(secret: String) -> Result<(), String> {
  #[cfg(debug_assertions)]
  if std::env::var("PONY_DESKTOP_DEV_FILE_KEYRING").is_ok() {
    // dev fallback：仅 debug 构建且显式环境变量触发（spec §6.4 护栏）
    let dir = std::env::temp_dir().join("pony-desktop-dev-keyring");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    return std::fs::write(dir.join("admin_token"), secret).map_err(|e| e.to_string());
  }
  let ent = entry()?;
  ent.set_password(&secret).map_err(|e| format!("credential set failed: {e}"))
}

#[tauri::command]
fn credential_get() -> Result<Option<String>, String> {
  #[cfg(debug_assertions)]
  if std::env::var("PONY_DESKTOP_DEV_FILE_KEYRING").is_ok() {
    let p = std::env::temp_dir().join("pony-desktop-dev-keyring/admin_token");
    return Ok(std::fs::read_to_string(p).ok());
  }
  match entry()?.get_password() {
    Ok(v) => Ok(Some(v)),
    Err(keyring::Error::NoEntry) => Ok(None),
    Err(e) => Err(format!("credential get failed: {e}")),
  }
}

#[tauri::command]
fn credential_delete() -> Result<(), String> {
  #[cfg(debug_assertions)]
  if std::env::var("PONY_DESKTOP_DEV_FILE_KEYRING").is_ok() {
    let p = std::env::temp_dir().join("pony-desktop-dev-keyring/admin_token");
    let _ = std::fs::remove_file(p);
    return Ok(());
  }
  match entry()?.delete_credential() {
    Ok(()) => Ok(()),
    Err(keyring::Error::NoEntry) => Ok(()),
    Err(e) => Err(format!("credential delete failed: {e}")),
  }
}

// ---- M6 系统级白名单代理（spec m6 §4/§5）----
use std::sync::atomic::{AtomicBool, Ordering as AOrd};

const WL_KEY: &str = "pony-proxy-whitelist";

#[tauri::command]
fn proxy_whitelist_get() -> Vec<String> {
    let dir = data_dir();
    std::fs::read_to_string(dir.join("whitelist.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(seed)
}

fn seed() -> Vec<String> {
    ["github.com", "google.com", "youtube.com", "googlevideo.com", "githubassets.com", "googleusercontent.com", "gstatic.com", "googleapis.com", "ytimg.com", "ggpht.com"]
        .iter().map(|s| s.to_string()).collect()
}

fn data_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    { std::env::var("APPDATA").map(std::path::PathBuf::from).unwrap_or(std::env::temp_dir()).join("pony-desktop") }
    #[cfg(not(windows))]
    { std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".pony-desktop")).unwrap_or(std::env::temp_dir()) }
}

#[tauri::command]
fn proxy_whitelist_set(entries: Vec<String>) -> Result<(), String> {
    for e in &entries {
        if e.is_empty() || e.len() > 253 || !e.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) {
            return Err("invalid entry".into());
        }
    }
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("whitelist.json"), serde_json::to_string(&entries).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

static ENGINE_ON: AtomicBool = AtomicBool::new(false);
static mut ENGINE_TASK: Option<tauri::async_runtime::JoinHandle<()>> = None;
static SNAPSHOT: std::sync::Mutex<Option<proxy::sysproxy::Snapshot>> = std::sync::Mutex::new(None);

#[tauri::command]
fn proxy_enable() -> Result<(), String> {
    if ENGINE_ON.load(AOrd::SeqCst) { return Ok(()); }
    let wl = proxy_whitelist_get();
    let stats = std::sync::Arc::new(proxy::engine::EngineStats::default());
    let _ = PROXY_STATS.set(std::sync::Arc::clone(&stats));
    tauri::async_runtime::spawn(async move {
        // 2026-08 审计整改：禁止硬编码隧道端点/令牌（旧值已随安装包分发并轮换作废）。
        // 隧道为 opt-in：待设置页接入后由 keyring/配置注入，缺省直连不走 gate。
        if let Err(e) = proxy::engine::run(proxy::engine::EngineConfig { listen_addr: "127.0.0.1:18900".into(), whitelist: wl, tunnel_url: None, tunnel_token: None }, stats).await {
            log::warn!("proxy engine exited: {e}");
        }
    });
    let snap = proxy::sysproxy::enable(proxy::sysproxy::Mode::Pac)?;
    *SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner()) = Some(snap);
    ENGINE_ON.store(true, AOrd::SeqCst);
    Ok(())
}

#[tauri::command]
fn proxy_disable() -> Result<(), String> {
    if !ENGINE_ON.load(AOrd::SeqCst) { return Ok(()); }
    let snap_guard = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(snap) = snap_guard.clone() {
        proxy::sysproxy::disable(&snap)?;
    }
    ENGINE_ON.store(false, AOrd::SeqCst);
    // 引擎进程内循环随应用生命周期运行（停用=仅还原系统代理）
    Ok(())
}

#[tauri::command]
fn proxy_status() -> serde_json::Value {
    serde_json::json!({
        "engine_running": ENGINE_ON.load(AOrd::SeqCst),
    })
}

static PROXY_STATS: std::sync::OnceLock<proxy::engine::SharedStats> = std::sync::OnceLock::new();

#[tauri::command]
async fn proxy_test_sites() -> Result<Vec<serde_json::Value>, String> {
    let sites = ["www.google.com", "www.youtube.com", "x.com", "github.com"];
    let stats = PROXY_STATS.get().cloned();
    let mut out = Vec::new();
    for site in sites {
        let started = std::time::Instant::now();
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpDial::dial(site, 443),
        ).await;
        let ok = matches!(r, Ok(Ok(_)));
        let ms = started.elapsed().as_millis() as u64;
        out.push(serde_json::json!({
            "site": site, "ok": ok, "ms": ms,
            "error": if let Ok(Err(e)) = &r { e.to_string() } else { String::new() },
        }));
        if let Some(s) = &stats { s.direct.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
    }
    Ok(out)
}

struct TcpDial;
impl TcpDial { async fn dial(host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    tokio::net::TcpStream::connect((host, port)).await
} }

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tauri::command]
fn proxy_pac() -> String {
    proxy::pac::generate_pac(&proxy_whitelist_get())
}
