#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_http::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .setup(|app| {
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
