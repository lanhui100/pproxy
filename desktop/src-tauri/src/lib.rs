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
      tunnel_connect_code_import, tunnel_self_check, tunnel_config_set,
      proxy_auto_config_get, proxy_auto_config_set, app_config_get, app_config_set,
      proxy_bypass_hosts,
      proxy_rescue, proxy_import_sync, proxy_mode_switch, proxy_get_current_config,
      proxy_traffic_stats, proxy_test_egress, proxy_test_site_via, proxy_test_site_local,
      proxy_access_url_generate, proxy_api_token_get, proxy_api_token_set,
      proxy_cluster_nodes_get,
      open_external_url, proxy_prepare_update_exit, proxy_open_log_dir,
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

/// 凭据写入审计（P0/E7：本地写入时间线——无它 H1/H2 永远扯皮）。
/// 只记时间戳/来源/指纹，永不记明文。
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct CredMeta {
  last_write_ts: u64,
  source: String,
  fp8: String,
}
fn cred_meta_path(user: &str) -> std::path::PathBuf {
  data_dir().join(format!(".{user}.meta.json"))
}
fn cred_meta_load(user: &str) -> CredMeta {
  std::fs::read_to_string(cred_meta_path(user))
    .ok()
    .and_then(|s| serde_json::from_str::<CredMeta>(&s).ok())
    .unwrap_or_default()
}
fn cred_meta_record(user: &str, source: &str, secret: &str) {
  use sha2::{Digest, Sha256};
  let fp8 = hex::encode(&Sha256::digest(secret.as_bytes())[..4]);
  let meta = CredMeta {
    last_write_ts: std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_secs())
      .unwrap_or(0),
    source: source.to_string(),
    fp8,
  };
  let dir = data_dir();
  let _ = std::fs::create_dir_all(&dir);
  let tmp = dir.join(format!(".{user}.meta.json.tmp"));
  if std::fs::write(&tmp, serde_json::to_string(&meta).unwrap_or_default()).is_ok() {
    let _ = std::fs::rename(&tmp, cred_meta_path(user));
  }
}

/// 分源指纹（P0/E4：H2 实锤的前提——fallback 与 keyring 任一分叉即本地坏）。
/// 现只返回赢家时 keyring 污染在 fallback 存在下永久隐身，故三分全暴露。
#[derive(Clone, serde::Serialize, Default)]
struct CredDetail {
  /// 有效值（keyring 优先，见 cred_get_impl）
  value: Option<String>,
  fp_fallback: Option<String>,
  fp_keyring: Option<String>,
  keyring_error: Option<String>,
  winner: &'static str,
}
fn cred_fingerprint_of(s: &str) -> String {
  use sha2::{Digest, Sha256};
  hex::encode(&Sha256::digest(s.as_bytes())[..4])
}
/// 时间戳备份裁剪：只保留最近 5 个 `.dat.bak.<millis>_<pid>`（按文件名排序删旧；最新 `.bak` 不在此列）。
fn cred_prune_timestamped_backups(fb_path: &std::path::Path) {
  let prefix = format!("{}.bak.", fb_path.display());
  let dir = match fb_path.parent() {
    Some(d) => d,
    None => return,
  };
  let mut olds: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
    .ok()
    .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.display().to_string().starts_with(&prefix)).collect())
    .unwrap_or_default();
  olds.sort();
  while olds.len() > 5 {
    let victim = olds.remove(0);
    let _ = std::fs::remove_file(victim);
  }
}

/// 时间戳备份全删（显式清除路径：.dat.bak + 全部时间戳副本，不留活密钥）。
fn cred_remove_timestamped_backups(fb_path: &std::path::Path) {
  let prefix = format!("{}.bak.", fb_path.display());
  let dir = match fb_path.parent() {
    Some(d) => d,
    None => return,
  };
  if let Ok(rd) = std::fs::read_dir(dir) {
    for e in rd.filter_map(|e| e.ok()) {
      let p = e.path();
      if p.display().to_string().starts_with(&prefix) {
        let _ = std::fs::remove_file(p);
      }
    }
  }
}

/// 新时间戳备份名（毫秒 + pid：秒级同名在高频分叉下会覆盖丢代）。
fn cred_timestamped_bak_path(fb_path: &std::path::Path) -> std::path::PathBuf {
  let ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis())
    .unwrap_or(0);
  std::path::PathBuf::from(format!("{}.bak.{ms}_{}", fb_path.display(), std::process::id()))
}

/// Unix 显式 0600（fallback 存明文密钥；Windows 走 ACL，不动）。
#[cfg(unix)]
fn cred_restrict_permissions(p: &std::path::Path) {
  use std::os::unix::fs::PermissionsExt;
  let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}

/// 分叉自修复写段（锁内串行）：重读 fallback，仍分叉才备份 + tmp/rename 覆盖 + 审计。
/// 调用方 `cred_detail_impl` 不持锁（keyring IPC 已在锁外完成），此处只锁文件读改写，
/// 与 `cred_set` / `cred_delete` 互斥，并发“读-改-写自愈 × 写”不撕裂。
/// 返回 winner；重读已一致（并发 set 抢先）则无需覆盖，返回 `"keyring"`。
fn cred_self_heal_locked(user: &str, keyring_val: &str) -> &'static str {
  let _guard = CRED_LOCK.lock().unwrap_or_else(|p| p.into_inner());
  let fb_path = cred_fallback_file(user);
  let cur_fb = std::fs::read_to_string(&fb_path)
    .ok()
    .as_deref()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(|s| s.to_string());
  match cur_fb {
    Some(f) if f != keyring_val => {
      let bak_path = fb_path.with_extension("dat.bak");
      let _ = std::fs::copy(&fb_path, &bak_path);
      let ts_bak = cred_timestamped_bak_path(&fb_path);
      let _ = std::fs::copy(&fb_path, &ts_bak);
      cred_prune_timestamped_backups(&fb_path);
      let tmp = fb_path.with_extension("dat.tmp");
      if std::fs::write(&tmp, keyring_val.as_bytes()).is_ok()
        && std::fs::rename(&tmp, &fb_path).is_ok()
      {
        #[cfg(unix)]
        cred_restrict_permissions(&fb_path);
        cred_meta_record(user, "self_heal_diverged", keyring_val);
        log::warn!("credential divergence for {user}: keyring wins, fallback self-healed (old kept at .bak)");
      } else {
        log::warn!("credential divergence for {user}: keyring wins in memory, fallback heal failed");
      }
      "keyring(diverged)"
    }
    _ => "keyring",
  }
}
fn cred_detail_impl(user: &str) -> Result<CredDetail, String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) {
    let v = std::fs::read_to_string(p).ok().filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string());
    let fp = v.as_deref().map(cred_fingerprint_of);
    return Ok(CredDetail { value: v, fp_fallback: fp.clone(), fp_keyring: fp, keyring_error: None, winner: "dev_file" });
  }
  let fallback_raw = std::fs::read_to_string(cred_fallback_file(user)).ok();
  let fallback_val = fallback_raw.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
  let fp_fallback = fallback_val.as_deref().map(cred_fingerprint_of);
  let (keyring_val, keyring_error) = match cred_entry(user) {
    Err(e) => (None, Some(e)),
    Ok(ent) => match ent.get_password() {
      Ok(v) => {
        let t = v.trim().to_string();
        if t.is_empty() { (None, None) } else { (Some(t), None) }
      }
      Err(keyring::Error::NoEntry) => (None, None),
      Err(e) => (None, Some(format!("credential get failed: {e}"))),
    },
  };
  let fp_keyring = keyring_val.as_deref().map(cred_fingerprint_of);
  // P0-3 降级方向：keyring（系统凭据库）> fallback（本地备份）。
  // 旧 fallback 残留曾永久屏蔽 keyring 新值；现分叉时以 keyring 为准并自修复 fallback，同时上报。
  // Must-fix D：自修复前先备份旧 fallback（.bak），成功后补审计——keyring 被污染时不销毁唯一正确副本。
  // 新鲜度仲裁：meta 证明 fallback 是人工写入来源的最新值（meta.fp8 == fallback 且 != keyring）
  // → 疑似 keyring 被外部污染，不覆盖 fallback，只反向告警，等人工裁决。
  let meta = cred_meta_load(user);
  // 权威 store 决策（产品裁决：分叉永不静默覆盖，以“最后一次人工写入”为准）：
  // - meta.fp8 == fallback 且 != keyring → keyring 疑似被外部污染，有效值取 fallback，
  //   winner=`fallback(diverged-unhealed)`，只告警，等用户显式重贴统一（不自动覆盖）。
  // - 其余分叉 → keyring 为准并自修复 fallback（旧副本留 .bak + 时间戳多代），winner=`keyring(diverged)`。
  // meta 仅为 advisory（本地可写文件，不做信任假设；缺失/损坏即回退 keyring-wins）。
  let (value, winner) = match (&keyring_val, &fallback_val) {
    (Some(k), Some(f)) if k != f => {
      let fp_f = fp_fallback.as_deref().unwrap_or("");
      let fp_k = fp_keyring.as_deref().unwrap_or("");
      let manual = matches!(meta.source.as_str(), "tunnel_token_save" | "connect_code_import" | "configure_direct_tunnel" | "tunnel_config_set");
      if manual && !meta.fp8.is_empty() && meta.fp8 == fp_f && meta.fp8 != fp_k {
        log::warn!("credential divergence for {user}: fallback matches last manual write fp8={fp_f} (source={}), keyring fp8={fp_k} suspect — effective value is fallback, awaiting explicit re-paste", meta.source);
        (Some(f.clone()), "fallback(diverged-unhealed)")
      } else {
        // 锁内串行读改写（与 cred_set / cred_delete 互斥），重读仍分叉才覆盖
        let winner = cred_self_heal_locked(user, k);
        (Some(k.clone()), winner)
      }
    }
    (Some(k), _) => (Some(k.clone()), "keyring"),
    (None, Some(f)) => (Some(f.clone()), "fallback"),
    (None, None) => (None, "none"),
  };
  Ok(CredDetail { value, fp_fallback, fp_keyring, keyring_error, winner })
}

fn cred_set_impl(user: &str, secret: String) -> Result<(), String> {
  cred_set_impl_with_source(user, secret, "unknown")
}

/// 凭据文件互斥：所有触碰 fallback/.bak/meta 的读改写必须经此锁串行
///（`cred_set` 全段、`cred_self_heal_locked` 写段、`cred_delete` 全段；
/// `cred_detail_impl` 的 keyring IPC 在锁外，重读-覆盖收敛到 `cred_self_heal_locked` 内）。
/// meta 写只发生在上述三处锁内，不存在独立调用方。禁止在 async 上下文持锁（均为同步命令路径）。
static CRED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 带来源审计的凭据写入（P0）：双写任一失败即 Err（禁止“内存先行、磁盘静默失败”导致的内存-磁盘分裂）。
fn cred_set_impl_with_source(user: &str, secret: String, source: &str) -> Result<(), String> {
  let _guard = CRED_LOCK.lock().unwrap_or_else(|p| p.into_inner());
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) {
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(p, &secret).map_err(|e| e.to_string())?;
    cred_meta_record(user, source, &secret);
    return Ok(());
  }

  // 1. 先写系统凭据库（主）。失败即整体失败——禁止 fallback 先行造成“备份新、主旧”分叉。
  let ent = cred_entry(user)?;
  ent.set_password(&secret).map_err(|e| format!("credential set failed: {e}"))?;

  // 2. 再写本地私有目录备份（与 meta 同目录顺序写：先 fallback tmp+rename，再审计）。
  // 失败同样整体失败，但 keyring 已成功 → 返回半写分类错误，调用方必须向用户报错重贴。
  let dir = data_dir();
  let fallback = dir.join(format!(".{user}.dat"));
  std::fs::create_dir_all(&dir).map_err(|e| format!("half-written:keyring-ok,fallback-dir-{e}"))?;
  let tmp = dir.join(format!(".{user}.dat.tmp"));
  std::fs::write(&tmp, secret.as_bytes()).map_err(|e| format!("half-written:keyring-ok,fallback-{e}"))?;
  std::fs::rename(&tmp, &fallback).map_err(|e| format!("half-written:keyring-ok,fallback-{e}"))?;

  cred_meta_record(user, source, &secret);
  Ok(())
}

/// Must-fix E 口径（value-first + 显式告警）：
/// keyring 报错但 fallback 有有效值时，返回 fallback 值并附带 keyring 告警（两接口一致），
/// 禁止“同一时刻 proxy_tunnel_get 说有、self_check 说无”的矛盾。
/// 无 keyring 环境声明：本应用要求系统凭据库可用；keyring 持续报错时 UI 会显式告警，
/// 用户应检查凭据库权限/服务（如 Linux secret-service），而非静默降级。
fn cred_get_with_warning(user: &str) -> (Result<Option<String>, String>, Option<String>) {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) {
    return (Ok(std::fs::read_to_string(p).ok()), None);
  }
  match cred_detail_impl(user) {
    Err(e) => (Err(e), None),
    Ok(d) => {
      let warn = d.keyring_error.clone().map(|e| format!("系统凭据库告警（已用本地备份继续，请检查凭据库权限/服务）：{e}"));
      match (d.value, d.keyring_error) {
        (v, Some(_)) if v.is_some() => (Ok(v), warn),
        (_v, Some(e)) => (Err(e), None),
        (v, None) => (Ok(v), None),
      }
    }
  }
}

fn cred_get_impl(user: &str) -> Result<Option<String>, String> {
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { return Ok(std::fs::read_to_string(p).ok()); }

  let d = cred_detail_impl(user)?;
  // value-first：fallback 有有效值即返回（附带告警由调用方透出），无值才 Err
  if d.value.is_some() {
    return Ok(d.value);
  }
  if let Some(e) = d.keyring_error {
    return Err(e);
  }
  Ok(None)
}

fn cred_delete_impl(user: &str) -> Result<(), String> {
  cred_delete_impl_with_source(user, "unknown")
}

/// fallback 全件套删除（显式清除路径：.dat + .dat.bak + 全部时间戳副本，不留活密钥；
/// 与自愈“保留 .bak”严格区分）。纯文件操作，不碰 keyring，可独立单测。
fn cred_remove_fallback_all(user: &str) {
  let fallback = cred_fallback_file(user);
  let _ = std::fs::remove_file(&fallback);
  let _ = std::fs::remove_file(fallback.with_extension("dat.bak"));
  cred_remove_timestamped_backups(&fallback);
}

fn cred_delete_impl_with_source(user: &str, source: &str) -> Result<(), String> {
  let _guard = CRED_LOCK.lock().unwrap_or_else(|p| p.into_inner());
  #[cfg(debug_assertions)]
  if let Some(p) = cred_dev_file(user) { let _=std::fs::remove_file(p); return Ok(()); }

  // 顺序：keyring 先删（失败即整体失败，fallback 完好可续命，不分裂）；
  // 再删 fallback 全件套（.dat + .bak + 时间戳多代——显式清除不留活密钥，
  // 与自愈“保留 .bak”严格区分）；最后 tombstone 审计。
  if let Ok(ent) = cred_entry(user) {
    match ent.delete_credential() {
      Ok(()) => {}
      // NoEntry 视为成功（本来就无值）；其他 Err 上抛，禁止吞掉
      Err(keyring::Error::NoEntry) => {}
      Err(e) => return Err(format!("credential delete failed: {e}")),
    }
  }
  cred_remove_fallback_all(user);
  // Must-fix F：删除保留审计 tombstone（禁止审计归零——何时/何因清除是 H2 关键时间线）
  cred_meta_record_tombstone(user, source);
  Ok(())
}

/// 删除审计 tombstone（值已清，但保留何时/何因清除）。
fn cred_meta_record_tombstone(user: &str, source: &str) {
  let meta = CredMeta {
    last_write_ts: std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_secs())
      .unwrap_or(0),
    source: format!("{source}:cleared"),
    fp8: String::new(),
  };
  let dir = data_dir();
  let _ = std::fs::create_dir_all(&dir);
  let tmp = dir.join(format!(".{user}.meta.json.tmp"));
  if std::fs::write(&tmp, serde_json::to_string(&meta).unwrap_or_default()).is_ok() {
    let _ = std::fs::rename(&tmp, cred_meta_path(user));
  }
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
    static LOG_ONCE: std::sync::Once = std::sync::Once::new();
    #[cfg(windows)]
    let (dir, via_temp) = match std::env::var("APPDATA") {
        Ok(v) => (std::path::PathBuf::from(v).join("pony-desktop"), false),
        Err(_) => (std::env::temp_dir().join("pony-desktop"), true),
    };
    #[cfg(not(windows))]
    let (dir, via_temp): (std::path::PathBuf, bool) = match std::env::var("HOME") {
        Ok(h) => (std::path::PathBuf::from(h).join(".pony-desktop"), false),
        Err(_) => (std::env::temp_dir(), true),
    };
    LOG_ONCE.call_once(|| {
        log::info!("data_dir resolved: {}", dir.display());
    });
    // via_temp 时每次 warn 会刷日志（data_dir 调用频繁）——限频：进程内只告警一次
    if via_temp {
        static WARN_ONCE: std::sync::Once = std::sync::Once::new();
        WARN_ONCE.call_once(|| {
            log::warn!("data_dir: 环境变量缺失，已回退到系统临时目录: {}", dir.display());
        });
    }
    dir
}
/// 双源回读校验（A-P0-2）：禁止 value-first 单源通过——要求 fallback 与 keyring
/// 双指纹都等于期望 secret 指纹。任一源缺失/分叉即 Err（half-written 可证伪），
/// 防止 keyring-ok+fallback-fail 或 keyring 瞬时错读被误判为落盘成功。
fn cred_verify_dual_source(user: &str, secret: &str) -> Result<(), String> {
  let want = cred_fingerprint_of(secret.trim());
  let d = cred_detail_impl(user).map_err(|e| format!("隧道凭据落盘校验失败：{e}"))?;
  let ok_fb = d.fp_fallback.as_deref() == Some(want.as_str());
  let ok_kr = d.fp_keyring.as_deref() == Some(want.as_str());
  if ok_fb && ok_kr {
    return Ok(());
  }
  Err(format!(
    "隧道凭据落盘校验失败（双源不一致：fallback={} keyring={}，期望 fp8={want}）：请重试或检查系统凭据库权限",
    if ok_fb { "ok" } else { "mismatch" },
    if ok_kr { "ok" } else { "mismatch" },
  ))
}
/// 数据目录是否走了临时回退（供前端展示重启丢失风险警告；与 `data_dir()` 同口径，不读盘）。
fn data_dir_tmp_fallback() -> bool {
    #[cfg(windows)]
    {
        std::env::var("APPDATA").is_err()
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").is_err()
    }
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
/// gate 隧道端点（WS↔TCP 桥）：部署于 rn.ponygo.fun/ws（本私有部署 Rust gate-server，
/// 唯一启用 USER_VERIFYING_KEY 多租户验签 + 单令牌 fallback 兼容的出口，见 crates/gate-server）。
/// 注意：与 HTTP 数据面网关（edge.ponygo.fun，cf-worker）不是同一域名，切勿混用。
/// Vercel/CF 等 Node gate（vgate/gate.ponyjob.top）仅认单令牌 TUNNEL_TOKEN_HASH，
/// 无法验证 usr_live_ 多租户 token，故不列入默认端点（用户若显式配置单令牌可自行加回）。
const GATE_WS_URL: &str = "wss://rn.ponygo.fun/ws";
/// 内置首选出海节点（原生 VPS gate / Rust gate，兼容多租户 usr_live_ 与单令牌 fallback）
const RN_GATE_URL: &str = "wss://rn.ponygo.fun/ws";
/// 默认隧道端点（私有部署真实端点；合规出口分类器认 "rn." 前缀，Google 等照常经此出口）
const DEFAULT_TUNNEL_URLS: &str = "wss://rn.ponygo.fun/ws";

/// 旧配置迁移：早期脱敏三元组（rn/vgate/gate.example.com）与 Node gate 域名
/// （vgate/gate.ponyjob.top，仅单令牌）一律收敛到支持多租户的 rn.ponygo.fun/ws，
/// 防止拨测超时 / usr_live_ 401 / 隧道连接失败。
fn migrate_tunnel_url(url: &str) -> String {
    let t = url.trim();
    // 通用：凡含脱敏占位 example.com 或仅单令牌 Node gate 域名，统一收敛到真实默认端点
    if t.contains("example.com") || t.contains("ponyjob.top") {
        return DEFAULT_TUNNEL_URLS.to_string();
    }
    // 与真实默认端点一致时保持原值
    if t == GATE_WS_URL || t == DEFAULT_TUNNEL_URLS {
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
  // 口径统一（A-P1-8）：与 tunnel_config_load / tunnel_self_check 同走 migrate，
  // UI 空端点与引擎默认双端点回退不再分叉（get 展示 migrate 后值）。
  let url = tunnel_url_load().unwrap_or_default();
  let detail = cred_detail_impl(CREDENTIAL_USER_TUNNEL).unwrap_or_default();
  let (has_token, cred_error) = match (&detail.value, &detail.keyring_error) {
    (Some(t), _) if !t.is_empty() => (
      true,
      detail.keyring_error.as_deref().map(|e| serde_json::json!(format!("系统凭据库告警（已用本地备份继续，请检查凭据库权限/服务）：{e}"))).unwrap_or(serde_json::Value::Null),
    ),
    (_, Some(e)) => (false, serde_json::json!(format!("凭据损坏或编码不兼容（{e}）：请重新粘贴加速授权码"))),
    _ => (false, serde_json::Value::Null),
  };
  // P0/E4：分源指纹全暴露（H2 实锤前提）；P0/E7：上次写入审计
  let meta = cred_meta_load(CREDENTIAL_USER_TUNNEL);
  // effective_url：引擎实际使用的端点串（含默认双端点回退），UI 与引擎不再分叉
  let (eff_url, _) = tunnel_config_load();
  let user_claims = detail.value.as_deref().and_then(|tok| {
    use base64::Engine as _;
    let trimmed = tok.trim();
    if !trimmed.starts_with("usr_live_") { return None; }
    let rest = &trimmed["usr_live_".len()..];
    let dot_idx = rest.find('.')?;
    let payload_b64 = &rest[..dot_idx];
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice::<serde_json::Value>(&decoded).ok()
  });

  serde_json::json!({
    "url": url,
    "effective_url": eff_url.unwrap_or_default(),
    "has_token": has_token,
    "cred_error": cred_error,
    "fingerprint": detail.value.as_deref().map(cred_fingerprint_of),
    "fp_fallback": detail.fp_fallback,
    "fp_keyring": detail.fp_keyring,
    "cred_winner": detail.winner,
    "cred_meta": { "last_write_ts": meta.last_write_ts, "source": meta.source, "fp8": meta.fp8 },
    "data_dir": data_dir().display().to_string(),
    "data_dir_tmp_fallback": data_dir_tmp_fallback(),
    "user_claims": user_claims,
  })
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
  tunnel_watch_send_fresh();
  Ok(())
}
#[tauri::command]
fn tunnel_token_save(secret: String) -> Result<(), String> {
  if secret.trim().is_empty() { return Err("empty tunnel token".into()); }
  let secret = secret.trim().to_string();
  // P0：双写任一失败即整体失败（cred_set 双写 Err 化），禁止内存先行造成分裂
  cred_set_impl_with_source(CREDENTIAL_USER_TUNNEL, secret.clone(), "tunnel_token_save")?;
  // 双源回读校验（A-P0-2：禁止 value-first 单源通过）：失败禁止广播 watch
  cred_verify_dual_source(CREDENTIAL_USER_TUNNEL, &secret)?;
  // 直发用户输入的 secret（不回读凭据），与 configure_direct_tunnel 同口径：
  // 凭据回读失败（如外部工具以非 keyring 编码写入）不影响本次保存即时生效。
  // 单次广播（A-P1-10）：补默认端点时直写文件不广播，避免中间态被引擎/池观察到。
  let (url, _) = tunnel_config_load();
  let url = match url {
    Some(u) => u,
    // 无合法端点配置时补齐默认双 gate（裸 token 粘贴即完成全部配置）
    None => {
      let dir = data_dir();
      std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
      let tmp = dir.join("tunnel.json.tmp");
      let target = dir.join(TUNNEL_FILE);
      let body = serde_json::to_string(&serde_json::json!({ "url": DEFAULT_TUNNEL_URLS })).map_err(|e| e.to_string())?;
      std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
      std::fs::rename(&tmp, target).map_err(|e| e.to_string())?;
      DEFAULT_TUNNEL_URLS.to_string()
    }
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

fn is_native_vps(u: &str) -> bool {
    u.contains("searchxai") || u.contains("rn.") || u.contains("rn.example.com") || u.contains("192.210.231.8")
}

/// 从配置或默认端点中解析指定接口 (rn / cf / vercel) 的 gate URL。
fn resolve_gate_url_for_iface(iface: &str) -> Option<String> {
    let url_raw = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url))
        .unwrap_or_else(|| DEFAULT_TUNNEL_URLS.to_string());
    let urls: Vec<String> = url_raw.split([',', ';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    match iface {
        "rn" => urls.iter().find(|u| is_native_vps(u)).cloned().or(Some(RN_GATE_URL.to_string())),
        // 当配置中存在多端点时优先使用匹配项；若仅配置了多租户 rn 端点，则降级复用 rn 端点进行测速（避免连向只认单口令的 Node/CF 网关直接报 401）
        "vercel" => urls.iter().find(|u| u.contains("vercel") || u.contains("vgate"))
            .cloned()
            .or_else(|| urls.iter().find(|u| is_native_vps(u)).cloned())
            .or_else(|| urls.first().cloned())
            .or(Some(RN_GATE_URL.to_string())),
        "cf" => urls.iter().find(|u| !u.contains("vercel") && !u.contains("vgate") && !is_native_vps(u))
            .cloned()
            .or_else(|| urls.iter().find(|u| is_native_vps(u)).cloned())
            .or_else(|| urls.first().cloned())
            .or(Some(GATE_WS_URL.to_string())),
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
  if code.len() > 4096 {
    return Err("连接口令长度超出限制（最大 4096 字符）".into());
  }
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

/// 一键导入 pony-gate:// 连接口令：经原子命令写端点 + 写凭据 + 单次广播，即时生效（无需重启）。
#[tauri::command]
fn tunnel_connect_code_import(code: String) -> Result<serde_json::Value, String> {
  let (url, token) = parse_connect_code(&code)?;
  let set_res = tunnel_config_set(Some(url.clone()), Some(token))?;
  Ok(serde_json::json!({
    "success": true,
    "url": url,
    "fingerprint": set_res.get("fingerprint").cloned().unwrap_or(serde_json::Value::Null),
    "message": "连接口令已导入，端点与令牌即时生效",
  }))
}

/// 隧道健康自检：本机凭据可读性 + token 指纹 + 逐 gate 实测（WS 升级成功即证明该端哈希与本机 token 一致）。
/// P0/E3+E10：error 必须分类（auth401 / denied门禁 / 超时），禁止把门禁 denied 误判为鉴权 401。
#[tauri::command]
async fn tunnel_self_check() -> Result<serde_json::Value, String> {
  // Must-fix E：与 proxy_tunnel_get 同口径（value-first + 显式 keyring 告警）
  let (cred, keyring_warn) = cred_get_with_warning(CREDENTIAL_USER_TUNNEL);
  let (cred_ok, cred_error, token): (bool, serde_json::Value, Option<String>) = match cred {
    Ok(Some(t)) if !t.is_empty() => (
      true,
      keyring_warn.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
      Some(t),
    ),
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
    let name = if u.contains("searchxai") || u.contains("rn.") || u.contains("192.210.231.8") {
        "rn"
    } else if u.contains("vercel") || u.contains("vgate") {
        "vercel"
    } else {
        "cf"
    };
    let item = match &token {
      Some(t) => {
        // Upgrade 探针与 bind 探针并发跑；旧 kind 取 bind 分类（无 token 仍为 no_token）
        let (up_res, bind_res) = tokio::join!(
          proxy::engine_tunnel::probe_gate_rtt(&u, t),
          proxy::engine_tunnel::probe_via_gate(&u, t, "www.google.com", 443)
        );
        let (up_ok, up_ms, up_kind, up_err) = match up_res {
          Ok(ms) => (true, serde_json::json!(ms), "ok".to_string(), serde_json::Value::Null),
          Err(e) => (false, serde_json::json!(0), proxy::engine_tunnel::classify_probe_error(&e).to_string(), serde_json::json!(e)),
        };
        let (bind_ok, bind_ms, bind_kind, bind_err) = match bind_res {
          Ok(ms) => (true, serde_json::json!(ms), "ok".to_string(), serde_json::Value::Null),
          Err(e) => (false, serde_json::json!(0), proxy::engine_tunnel::classify_probe_error(&e).to_string(), serde_json::json!(e)),
        };
        serde_json::json!({
          "name": name, "url": u,
          "ok": bind_ok, "ms": bind_ms, "error": bind_err, "kind": bind_kind,
          "kind_upgrade": up_kind, "upgrade_ok": up_ok, "upgrade_ms": up_ms, "upgrade_error": up_err,
          "kind_bind": bind_kind, "bind_ok": bind_ok, "bind_ms": bind_ms,
        })
      }
      None => serde_json::json!({ "name": name, "url": u, "ok": false, "error": "no token", "kind": "no_token", "kind_upgrade": "no_token", "kind_bind": "no_token" }),
    };
    gates.push(item);
  }
  // P0/E4+E7：分源指纹 + 写入审计随自检一起暴露（H2 实锤三值）
  let detail = cred_detail_impl(CREDENTIAL_USER_TUNNEL).unwrap_or_default();
  let meta = cred_meta_load(CREDENTIAL_USER_TUNNEL);
  Ok(serde_json::json!({
    "fingerprint": tunnel_token_fingerprint(),
    "cred_ok": cred_ok,
    "cred_error": cred_error,
    "gates": gates,
    "fp_fallback": detail.fp_fallback,
    "fp_keyring": detail.fp_keyring,
    "cred_winner": detail.winner,
    "cred_meta": { "last_write_ts": meta.last_write_ts, "source": meta.source, "fp8": meta.fp8 },
    "data_dir": data_dir().display().to_string(),
    "data_dir_tmp_fallback": data_dir_tmp_fallback(),
  }))
}
#[tauri::command]
fn tunnel_token_clear() -> Result<(), String> {
  cred_delete_impl_with_source(CREDENTIAL_USER_TUNNEL, "tunnel_token_clear")?;
  // A-P1-12：显式清除直发广播，绕过防毒化守卫——清除是用户安全意图，
  // 即使 keyring 瞬时故障也不得让旧凭据在引擎续命（与瞬时故障的旧值续命严格区分）
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  // 清除令牌时，重置 configured 状态为 false，确保前端回到初始接入状态
  let _ = app_config_set(serde_json::json!({ "configured": false }));
  Ok(())
}
/// 端点+凭据原子设置：url/secret 均为 Option（None=沿用）；任一失败即 Err 且不广播。
/// 成功后回读校验凭据（有 secret 时）再单次广播，最后切 direct/configured。
#[tauri::command]
fn tunnel_config_set(url: Option<String>, secret: Option<String>) -> Result<serde_json::Value, String> {
  if url.is_none() && secret.is_none() {
    return Err("隧道端点与令牌均为空：请至少提供其中一项".into());
  }
  let norm_url: Option<String> = match url {
    Some(u) => {
      let t = u.trim().to_string();
      validate_tunnel_url(&t)?;
      if !t.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).all(|x| x.starts_with("wss://") || x.starts_with("ws://")) {
        return Err("tunnel url must start with wss:// or ws://".into());
      }
      Some(t)
    }
    None => None,
  };
  let norm_secret: Option<String> = match secret {
    Some(s) => {
      let t = s.trim().to_string();
      if t.is_empty() { return Err("隧道令牌为空：已保留旧令牌，未覆盖".into()); }
      Some(t)
    }
    None => None,
  };
  // 先写端点（有 url 时）再双写凭据（有 secret 时）；任一失败即 Err，
  // 且回滚已落盘的端点（真原子：失败即“什么都没变”，调用方可安全重试）。
  // 注意：端点此处直写文件而不调用 proxy_tunnel_set_url，避免中途广播破坏原子性。
  let prev_url_raw = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok();
  let rollback_url = |prev: &Option<String>| {
    let target = data_dir().join(TUNNEL_FILE);
    match prev {
      Some(prev) => {
        let tmp = data_dir().join("tunnel.json.tmp");
        if std::fs::write(&tmp, prev).is_ok() {
          let _ = std::fs::rename(&tmp, target);
        }
      }
      None => {
        let _ = std::fs::remove_file(target);
      }
    }
  };
  if let Some(ref u) = norm_url {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("tunnel.json.tmp");
    let target = dir.join(TUNNEL_FILE);
    std::fs::write(&tmp, serde_json::to_string(&serde_json::json!({ "url": u })).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, target).map_err(|e| e.to_string())?;
  }
  if let Some(ref s) = norm_secret {
    if let Err(e) = cred_set_impl_with_source(CREDENTIAL_USER_TUNNEL, s.clone(), "tunnel_config_set") {
      rollback_url(&prev_url_raw);
      return Err(e);
    }
  }
  // 成功后双源回读校验凭据（有 secret 时：双指纹一致才算落盘成功），再单次广播；
  // 校验失败回滚端点（真原子：失败即“什么都没变”）
  if let Some(ref s) = norm_secret {
    if let Err(e) = cred_verify_dual_source(CREDENTIAL_USER_TUNNEL, s) {
      rollback_url(&prev_url_raw);
      return Err(e);
    }
  }
  let _ = ensure_tunnel_watch().send(tunnel_config_load());
  // 配置标记落盘失败必须上抛（A-P0-3：禁止 `configured=true` 虚报）
  app_config_set(serde_json::json!({ "mode_type": "direct", "configured": true }))?;
  let echo_url = tunnel_url_load().unwrap_or_default();
  Ok(serde_json::json!({
    "success": true,
    "url": echo_url,
    "fingerprint": tunnel_token_fingerprint(),
    "message": "隧道配置已更新并即时生效",
  }))
}
/// P0：401 自愈成功后的落盘（禁止只广播 watch 不落盘——否则 probe/流量分叉 + 重启丢失）。
/// 与 cred_set 同口径（keyring 主 + fallback 备 + 审计），失败返回 Err 由调用方告警。
pub(crate) fn persist_healed_tunnel_token(secret: &str, source: &str) -> Result<(), String> {
    cred_set_impl_with_source(CREDENTIAL_USER_TUNNEL, secret.trim().to_string(), source)
}

/// 隧道端点加载：读 tunnel.json + migrate + validate，失败返回 None（不碰凭据）。
fn tunnel_url_load() -> Option<String> {
  std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok()
    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(migrate_tunnel_url))
    .filter(|u| validate_tunnel_url(u).is_ok())
}
/// 隧道令牌加载状态：Ok=值或无；Err=keyring 瞬时错（调用方决定是否毒化下游）。
fn tunnel_token_load_status() -> Result<Option<String>, String> {
  cred_get_impl(CREDENTIAL_USER_TUNNEL).map(|v| v.filter(|t| !t.is_empty()))
}
/// 防毒化广播：fresh 全空而旧 watch 非空且 keyring 当前报错 → 跳过发送（旧值续命），否则发送。
fn tunnel_watch_send_fresh() {
  let fresh = tunnel_config_load();
  if fresh == (None, None) {
    let stale = {
      let g = ensure_tunnel_watch().borrow();
      g.clone()
    };
    if stale != (None, None) {
      if let Ok(d) = cred_detail_impl(CREDENTIAL_USER_TUNNEL) {
        if d.keyring_error.is_some() {
          log::warn!("tunnel_watch_send_fresh: keyring 瞬时故障，跳过空值广播以防毒化引擎（旧值续命中）");
          return;
        }
      }
    }
  }
  let _ = ensure_tunnel_watch().send(fresh);
}

fn tunnel_config_load() -> (Option<String>, Option<String>) {
  let url = tunnel_url_load();
  let token = tunnel_token_load_status().ok().flatten();
  match (url, token) {
    (Some(u), Some(t)) => (Some(u), Some(t)),
    // url 缺失但 token 存在 → 回退默认双端点（token 存在才回退，避免无 token 时引擎空转）
    (None, Some(t)) => (Some(DEFAULT_TUNNEL_URLS.to_string()), Some(t)),
    _ => (None, None),
  }
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
        // 防毒化：fresh 全空而旧值非空且 keyring 瞬时错时跳过覆盖（旧值续命）
        tunnel_watch_send_fresh();
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
    today_rn: EgressBucket,
    today_upstream: EgressBucket,
    total_cf: EgressBucket,
    total_vercel: EgressBucket,
    total_rn: EgressBucket,
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
    rn: EgressBucket,
    upstream: EgressBucket,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct HourEntry {
    hour: String,
    cf: EgressBucket,
    vercel: EgressBucket,
    rn: EgressBucket,
    upstream: EgressBucket,
}

static TRAFFIC_STATE: std::sync::Mutex<Option<TrafficPersist>> = std::sync::Mutex::new(None);
/// 已并入 TRAFFIC_STATE 的引擎原子读数基线
type TrafficRawCounters = (u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64);
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
            s.rn_up.load(Relaxed),
            s.rn_down.load(Relaxed),
            s.rn_reqs.load(Relaxed),
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
                || st.today_rn.reqs > 0 || st.today_rn.up > 0 || st.today_rn.down > 0
                || st.today_upstream.reqs > 0 || st.today_upstream.up > 0 || st.today_upstream.down > 0)
        {
            st.history.push(DayEntry {
                date: st.date.clone(),
                cf: st.today_cf.clone(),
                vercel: st.today_vercel.clone(),
                rn: st.today_rn.clone(),
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
        st.today_rn = EgressBucket::default();
        st.today_upstream = EgressBucket::default();
    }

    let cur_hour = current_hour_str();
    if st.hourly.last().map(|h| &h.hour) != Some(&cur_hour) {
        st.hourly.push(HourEntry {
            hour: cur_hour,
            cf: EgressBucket::default(),
            vercel: EgressBucket::default(),
            rn: EgressBucket::default(),
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
        cur.9.saturating_sub(b.9),
        cur.10.saturating_sub(b.10),
        cur.11.saturating_sub(b.11),
    );
    if d != (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) {
        st.today_cf.up += d.0;
        st.today_cf.down += d.1;
        st.today_cf.reqs += d.2;
        st.today_vercel.up += d.3;
        st.today_vercel.down += d.4;
        st.today_vercel.reqs += d.5;
        st.today_rn.up += d.6;
        st.today_rn.down += d.7;
        st.today_rn.reqs += d.8;
        st.today_upstream.up += d.9;
        st.today_upstream.down += d.10;
        st.today_upstream.reqs += d.11;

        st.total_cf.up += d.0;
        st.total_cf.down += d.1;
        st.total_cf.reqs += d.2;
        st.total_vercel.up += d.3;
        st.total_vercel.down += d.4;
        st.total_vercel.reqs += d.5;
        st.total_rn.up += d.6;
        st.total_rn.down += d.7;
        st.total_rn.reqs += d.8;
        st.total_upstream.up += d.9;
        st.total_upstream.down += d.10;
        st.total_upstream.reqs += d.11;

        if let Some(last_h) = st.hourly.last_mut() {
            last_h.cf.up += d.0;
            last_h.cf.down += d.1;
            last_h.cf.reqs += d.2;
            last_h.vercel.up += d.3;
            last_h.vercel.down += d.4;
            last_h.vercel.reqs += d.5;
            last_h.rn.up += d.6;
            last_h.rn.down += d.7;
            last_h.rn.reqs += d.8;
            last_h.upstream.up += d.9;
            last_h.upstream.down += d.10;
            last_h.upstream.reqs += d.11;
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

/// CF / Vercel / RN / Upstream 出网用量：今日 / 累计 / 近 7 日 / 近 24 小时（按时间升序）。
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
                "rn": bucket_json(&d.rn),
                "upstream": bucket_json(&d.upstream),
            })
        })
        .collect();
    history.push(serde_json::json!({
        "date": s.date,
        "cf": bucket_json(&s.today_cf),
        "vercel": bucket_json(&s.today_vercel),
        "rn": bucket_json(&s.today_rn),
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
                "rn": bucket_json(&h.rn),
                "upstream": bucket_json(&h.upstream),
            })
        })
        .collect();
    serde_json::json!({
        "today": {
            "cf": bucket_json(&s.today_cf),
            "vercel": bucket_json(&s.today_vercel),
            "rn": bucket_json(&s.today_rn),
            "upstream": bucket_json(&s.today_upstream),
        },
        "total": {
            "cf": bucket_json(&s.total_cf),
            "vercel": bucket_json(&s.total_vercel),
            "rn": bucket_json(&s.total_rn),
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

    // 方案 A：Direct 独立中继隧道模式（rn / cf / vercel）
    let gate = resolve_gate_url_for_iface(&iface)
        .ok_or_else(|| "未知接口：仅支持 rn / cf / vercel / chained".to_string())?;

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
        // 全口径：Upgrade + 首帧 bind 一次测通（到 www.google.com:443），消除 Upgrade 假绿
        match proxy::engine_tunnel::probe_via_gate(&gate, tok, "www.google.com", 443).await {
            Ok(ms) => {
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": true,
                    "ms": ms,
                }));
            }
            Err(e) => {
                let err_msg = e.clone();
                let iface_clone = iface.clone();
                tauri::async_runtime::spawn(async move {
                    let fp = tunnel_token_fingerprint().unwrap_or_else(|| "none".to_string());
                    let payload = serde_json::json!({
                        "type": "egress_probe_failure",
                        "iface": iface_clone,
                        "error": err_msg,
                        "token_fp": fp,
                        "os": std::env::consts::OS
                    });
                    let _ = reqwest::Client::new()
                        .post("https://rn.ponygo.fun/api/client/telemetry")
                        .header("content-type", "application/json")
                        .json(&payload)
                        .timeout(std::time::Duration::from_secs(3))
                        .send()
                        .await;
                });
                return Ok(serde_json::json!({
                    "iface": iface,
                    "ok": false,
                    "ms": 0,
                    "error": e,
                }));
            }
        }
    }

    // 未配置授权码时，按 TCP 握手 RTT 测试节点连通性（统一多租户 rn 出口）
    let host_port = extract_host_port_from_url(&gate).unwrap_or_else(|| match iface.as_str() {
        "rn" | "vercel" | _ => "rn.ponygo.fun:443".to_string(),
    });
    let started = std::time::Instant::now();
    let dial_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
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

    // 方案 A：Direct 独立中继隧道模式（rn / cf / vercel）
    let gate = resolve_gate_url_for_iface(&iface)
        .ok_or_else(|| "未知接口：仅支持 rn / cf / vercel / chained".to_string())?;
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

/// 获取集群节点状态列表（供管理员前端 Dashboard 展示多节点健康状况）
#[tauri::command]
async fn proxy_cluster_nodes_get() -> Result<serde_json::Value, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let cluster_cfg_path = std::path::Path::new(&home).join(".pony").join("cluster.json");
    let mut peer_addrs: Vec<String> = Vec::new();
    let mut cluster_id = "pproxy-mesh".to_string();

    if let Ok(raw) = std::fs::read_to_string(&cluster_cfg_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(cid) = v.get("cluster_id").and_then(|c| c.as_str()) {
                cluster_id = cid.to_string();
            }
            if let Some(seed) = v.get("seed_addr").and_then(|s| s.as_str()) {
                peer_addrs.push(seed.to_string());
            }
        }
    }

    if let Ok(peers_env) = std::env::var("PPROXY_CLUSTER_PEERS") {
        for p in peers_env.split([',', ';']) {
            let s = p.trim().to_string();
            if !s.is_empty() && !peer_addrs.contains(&s) {
                peer_addrs.push(s);
            }
        }
    }

    // 默认内置经典三节点拓扑作为缺省候选
    let default_candidates = ["100.95.193.103:8899", "100.97.143.121:8899", "100.105.241.39:8899"];
    for def in default_candidates {
        let s = def.to_string();
        if !peer_addrs.contains(&s) {
            peer_addrs.push(s);
        }
    }

    let mut nodes = Vec::new();
    for addr in peer_addrs {
        let started = std::time::Instant::now();
        // 快速 TCP 探测
        let is_online = tokio::time::timeout(
            std::time::Duration::from_millis(800),
            tokio::net::TcpStream::connect(&addr),
        ).await.map(|r| r.is_ok()).unwrap_or(false);
        let ms = started.elapsed().as_millis() as u64;

        let name = if addr.contains("100.95.193.103") {
            "devserver (主力节点)"
        } else if addr.contains("100.97.143.121") {
            "preprod (备灾节点1)"
        } else if addr.contains("100.105.241.39") {
            "tencent (备灾节点2)"
        } else {
            "edge-node"
        };

        nodes.push(serde_json::json!({
            "name": name,
            "address": addr,
            "online": is_online,
            "latency_ms": if is_online { Some(ms) } else { None },
        }));
    }

    Ok(serde_json::json!({
        "cluster_id": cluster_id,
        "nodes": nodes,
    }))
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

    // 自动异步遥测：当拨测发生非空错误时，静默上报错误上下文至网关
    if !ok && !error.is_empty() {
        let err_clone = error.clone();
        let host_clone = host.clone();
        tauri::async_runtime::spawn(async move {
            let fp = tunnel_token_fingerprint().unwrap_or_else(|| "none".to_string());
            let payload = serde_json::json!({
                "type": "site_probe_failure",
                "site": host_clone,
                "error": err_clone,
                "ms": ms,
                "token_fp": fp,
                "os": std::env::consts::OS
            });
            let _ = reqwest::Client::new()
                .post("https://rn.ponygo.fun/api/client/telemetry")
                .header("content-type", "application/json")
                .json(&payload)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await;
        });
    }

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
fn proxy_open_log_dir() -> Result<String, String> {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&dir).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
    Ok(dir.display().to_string())
}

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
    // 与 whitelist 同口径：tmp+rename 防半截写
    let tmp = dir.join("app_config.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&cur).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, app_config_path()).map_err(|e| e.to_string())?;
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
    std::fs::create_dir_all(&dir).map_err(|e| format!("tunnel dir failed: {e}"))?;
    let tmp = dir.join("tunnel.json.tmp");
    let target = dir.join(TUNNEL_FILE);
    // Must-fix F：端点落盘失败必须上抛（与 proxy_tunnel_set_url 一致），禁止静默
    std::fs::write(&tmp, serde_json::to_string(&serde_json::json!({ "url": ws_url })).unwrap_or_default()).map_err(|e| format!("tunnel url write failed: {e}"))?;
    std::fs::rename(&tmp, target).map_err(|e| format!("tunnel url persist failed: {e}"))?;

    // P0：空 secret 禁止覆盖（调用方误传空串即整体拒绝，不静默保留旧值造成“碰巧正确”）
    if secret.trim().is_empty() {
        return Err("empty tunnel token: refusing to overwrite".into());
    }
    // P0：落盘 + 直发同一 secret（禁止内存-磁盘分裂）；审计来源
    cred_set_impl_with_source(CREDENTIAL_USER_TUNNEL, secret.trim().to_string(), "configure_direct_tunnel")?;
    // Must-fix F：watch 广播必须在回读校验成功之后——校验失败 Err 时内存不得先行
    // 落盘后双源回读校验（防 keyring 静默失败导致的重启后 401）
    cred_verify_dual_source(CREDENTIAL_USER_TUNNEL, secret)?;
    let _ = ensure_tunnel_watch().send((Some(ws_url), Some(secret.trim().to_string())));
    Ok(())
}

/// 切换模式 A（个人独立直连）与模式 B（连接远端代理）
#[tauri::command]
fn proxy_mode_switch(mode_type: String, config: serde_json::Value) -> Result<serde_json::Value, String> {
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
            // P0：空串禁止覆盖旧密码（Dashboard 手动分支曾无条件发送 password:"" 直接销毁旧密码）
            if pass.trim().is_empty() {
                return Err("远端密码为空：已保留旧密码，未覆盖。请键入新密码后重试".into());
            }
            cred_set_impl_with_source(CREDENTIAL_USER_PROXY, pass.trim().to_string(), "mode_switch_chained")?;
        }
    } else if mode_type == "direct" {
        let worker = config.get("worker_url").and_then(|v| v.as_str()).unwrap_or("https://edge.example.com");
        patch["worker_url"] = serde_json::json!(worker);
        if let Some(sec) = config.get("proxy_secret").and_then(|v| v.as_str()) {
            // P0：空 proxy_secret 禁止静默保留旧 token——必须显式报错
            if sec.trim().is_empty() {
                return Err("隧道令牌为空：已保留旧令牌，未覆盖。请粘贴授权码后重试".into());
            }
            configure_direct_tunnel(worker, sec)?;
        } else {
            // P0：无 secret 的 direct 切换必须显式声明保留了哪一枚（指纹），禁止“碰巧正确”
            let fp = tunnel_token_fingerprint().unwrap_or_else(|| "none".to_string());
            patch["tunnel_retained_fp8"] = serde_json::json!(fp.clone());
            log::info!("proxy_mode_switch direct without proxy_secret: retained tunnel fp8={fp}");
        }
    }
    app_config_set(patch.clone())?;
    Ok(serde_json::json!({ "mode_type": mode_type, "tunnel_retained_fp8": patch.get("tunnel_retained_fp8") }))
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

        // P0：无 proxy_secret 的同步导入禁止 fail-open 复用陈旧凭据——必须显式拒绝或声明保留
        let retained_fp = match proxy_secret {
            Some(sec) if !sec.trim().is_empty() => {
                configure_direct_tunnel(worker_url, sec)?;
                None
            }
            _ => Some(tunnel_token_fingerprint().unwrap_or_else(|| "none".to_string())),
        };
        let mut patch = serde_json::json!({
            "mode_type": "direct",
            "server_url": server_url,
            "worker_url": worker_url,
            "configured": true
        });
        if let Some(fp) = &retained_fp {
            patch["tunnel_retained_fp8"] = serde_json::json!(fp);
            log::warn!("proxy_import_sync without proxy_secret: retained tunnel fp8={fp}");
        }
        let _ = app_config_set(patch);
        Ok(serde_json::json!({
            "success": true,
            "mode": "direct",
            "tunnel_retained_fp8": retained_fp,
            "message": if retained_fp.is_some() {
                "跨端同步成功！未携带隧道令牌，已保留本机旧令牌（见 tunnel_retained_fp8），如 401 请重贴授权码。"
            } else {
                "跨端同步成功！已自动切换为独立加速模式。"
            },
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
        // Must-fix F：双写 Err 必须上抛（禁止 let _ 吞掉落盘失败造成“配了但没存”）
        if password.trim().is_empty() {
            return Err("远端密码为空：已保留旧密码，未覆盖。请重新导出携带密码的口令".into());
        }
        cred_set_impl_with_source(CREDENTIAL_USER_PROXY, password.trim().to_string(), "proxy_import_sync")?;
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
    /// 文件凭据测试互斥锁：PONY_DESKTOP_DEV_FILE_KEYRING 是进程级环境变量，
    /// 触碰它的测试必须串行，否则并行 set/remove 会互相踩（全量跑挂、单跑过）。
    static CRED_FILE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
        // 早期脱敏三元组与仅单令牌 Node gate 一律收敛到支持多租户的 rn.ponygo.fun/ws
        assert_eq!(migrate_tunnel_url("wss://edge.example.com"), "wss://rn.ponygo.fun/ws");
        assert_eq!(migrate_tunnel_url("wss://gate.example.com/ws"), "wss://rn.ponygo.fun/ws");
        assert_eq!(migrate_tunnel_url("wss://gate.example.com/ws,wss://vgate.example.com/api/ws"), "wss://rn.ponygo.fun/ws");
        assert_eq!(migrate_tunnel_url("wss://vgate.ponyjob.top/api/ws,wss://gate.ponyjob.top/ws,wss://rn.ponygo.fun/ws"), "wss://rn.ponygo.fun/ws");
        // 已指向真实多租户端点保持原样
        assert_eq!(migrate_tunnel_url("wss://rn.ponygo.fun/ws"), "wss://rn.ponygo.fun/ws");
        // 自定义自有端点保持原样（真实域名不含占位/Node-gate 串）
        assert_eq!(migrate_tunnel_url("wss://self-gate.internal/ws"), "wss://self-gate.internal/ws");
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
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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

        // 默认无配置时 fallback 到默认端点（私有部署真实端点 vgate/gate.ponyjob.top + rn.ponygo.fun）
        assert!(resolve_gate_url_for_iface("rn").unwrap().contains("ponyjob.top") || resolve_gate_url_for_iface("rn").unwrap().contains("rn.ponygo.fun"));
        assert_eq!(resolve_gate_url_for_iface("unknown"), None);
    }

    // ---- P0 测试门禁（401 对抗审核 3-P1-7：4 个复现转单测，发版门）----
    // 依赖 PONY_DESKTOP_DEV_FILE_KEYRING 文件凭据（与 access_url_generate 用例同口径），
    // 不碰真实系统 keyring。

    #[test]
    fn p0_empty_tunnel_token_refused() {
        // 空 secret 禁止覆盖：tunnel_token_save("") 必须 Err
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PONY_DESKTOP_DEV_FILE_KEYRING", "1");
        assert!(tunnel_token_save("   ".into()).is_err());
        // configure_direct_tunnel 空 secret 必须 Err（禁止“碰巧正确”保留旧值）
        assert!(configure_direct_tunnel("wss://gate.example.com/ws", "  ").is_err());
        std::env::remove_var("PONY_DESKTOP_DEV_FILE_KEYRING");
    }

    #[test]
    fn p0_empty_proxy_password_refused() {
        // chained 空密码禁止覆盖旧密码：proxy_mode_switch 必须 Err
        let res = proxy_mode_switch(
            "chained".into(),
            serde_json::json!({ "remote_host": "1.2.3.4:8899", "username": "u", "password": "   " }),
        );
        assert!(res.is_err(), "空密码必须拒绝覆盖，实际: {res:?}");
    }

    #[test]
    fn p0_direct_switch_without_secret_retains_and_reports_fp() {
        // direct 无 secret 切换：必须显式声明保留了哪一枚（tunnel_retained_fp8），禁止静默
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PONY_DESKTOP_DEV_FILE_KEYRING", "1");
        let _ = tunnel_token_save("tok-p0-retain".into());
        let fp_before = tunnel_token_fingerprint();
        let res = proxy_mode_switch("direct".into(), serde_json::json!({ "worker_url": "https://edge.example.com" })).unwrap();
        assert_eq!(res["tunnel_retained_fp8"].as_str().map(|s| s.to_string()), fp_before);
        assert_eq!(tunnel_token_fingerprint(), fp_before, "无 secret 切换不得改动 token");
        let _ = cred_delete_impl(CREDENTIAL_USER_TUNNEL);
        std::env::remove_var("PONY_DESKTOP_DEV_FILE_KEYRING");
    }

    #[test]
    fn p0_save_url_only_keeps_token_fingerprint() {
        // saveTunnelConfig(url, "") 纯端点写：token 指纹必须不变
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PONY_DESKTOP_DEV_FILE_KEYRING", "1");
        let _ = tunnel_token_save("tok-p0-urlonly".into());
        let fp_before = tunnel_token_fingerprint();
        proxy_tunnel_set_url("wss://gate.example.com/ws,wss://vgate.example.com/api/ws".into()).unwrap();
        assert_eq!(tunnel_token_fingerprint(), fp_before);
        let _ = cred_delete_impl(CREDENTIAL_USER_TUNNEL);
        std::env::remove_var("PONY_DESKTOP_DEV_FILE_KEYRING");
    }

    #[test]
    fn p0_probe_error_classification() {
        // P0/E10：401 / denied / 超时必须三分，禁止门禁 denied 误判为鉴权
        assert_eq!(proxy::engine_tunnel::classify_probe_error("tunnel: HTTP error: 401 Unauthorized"), "auth401");
        assert_eq!(proxy::engine_tunnel::classify_probe_error("tunnel: denied: unsupported_colo:HKG"), "denied");
        assert_eq!(proxy::engine_tunnel::classify_probe_error("tunnel: denied: acl denied"), "denied");
        assert_eq!(proxy::engine_tunnel::classify_probe_error("dial timeout after 4s"), "timeout");
        assert_eq!(proxy::engine_tunnel::classify_probe_error("no token"), "no_token");
        assert_eq!(proxy::engine_tunnel::classify_probe_error("weird boom"), "other");
        // 401 判定与 RETRY 跳过同源
        assert!(proxy::engine_tunnel::is_auth_failure(&std::io::Error::other("tunnel: HTTP error: 401 Unauthorized")));
        assert!(!proxy::engine_tunnel::is_auth_failure(&std::io::Error::other("tunnel: denied: acl denied")));
        // B-P2-4：401 marker 不得是 "401" 裸子串（否则 denied 文案夹带 401 即误判）
        assert!(pproxy_transport::proto::AUTH_401_MARKER.contains("401"));
    }

    // ---- 双评审回改门禁（B-P0-1 / A-P0-3 / B-S-4）----

    #[test]
    fn review_explicit_clear_removes_all_backups() {
        // B-P0-1：显式清除的文件侧全清（.dat/.bak/时间戳多代，不留活密钥）。
        // 直接测 `cred_remove_fallback_all`（纯文件，不碰 keyring——CI/开发机真凭据不受影响）；
        // keyring 删除分支由 `cred_delete_impl_with_source` 的 NoEntry/Err 口径覆盖。
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = data_dir();
        let fb = dir.join(".tunnel_token.dat");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&fb, b"tok-clear-all");
        let _ = std::fs::copy(&fb, fb.with_extension("dat.bak"));
        let ts_bak = dir.join(".tunnel_token.dat.bak.123_456");
        let _ = std::fs::copy(&fb, &ts_bak);
        cred_remove_fallback_all(CREDENTIAL_USER_TUNNEL);
        assert!(!fb.exists(), ".dat 必须删除");
        assert!(!fb.with_extension("dat.bak").exists(), ".bak 必须删除（显式清除不留活密钥）");
        assert!(!ts_bak.exists(), "时间戳备份必须删除");
    }

    #[test]
    fn review_atomic_set_rolls_back_url_on_cred_failure() {
        // A-P0-3：tunnel_config_set 凭据失败时端点必须回滚（真原子）
        let _g = CRED_FILE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PONY_DESKTOP_DEV_FILE_KEYRING", "1");
        let url_before = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok();
        // 空 secret 触发凭据拒绝（先写端点、后凭据失败路径）
        let res = tunnel_config_set(
            Some("wss://rollback-check.example.com/ws".into()),
            Some("   ".into()),
        );
        assert!(res.is_err(), "空 secret 必须 Err，实际: {res:?}");
        let url_after = std::fs::read_to_string(data_dir().join(TUNNEL_FILE)).ok();
        assert_eq!(url_after, url_before, "失败必须回滚端点（什么都没变）");
        std::env::remove_var("PONY_DESKTOP_DEV_FILE_KEYRING");
    }

    #[test]
    fn review_config_set_rejects_both_none() {
        // B-S-4：双 None 与 Rust“至少提供一项”口径一致（直接 Err）
        let res = tunnel_config_set(None, None);
        assert!(res.is_err(), "双 None 必须 Err，实际: {res:?}");
    }
}
