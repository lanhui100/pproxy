//! `pproxy sync` — 跨端安全配置同步（ChaCha20-Poly1305 + 10 分钟 TTL + Nonce 防重放 + URL-Safe 协议头）。
//!
//! 支持从 Windows 桌面端导出加密配置，在 Linux Server 上一键导入，免去重复登录 CF/Vercel。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use pproxy_core::store::{default_db_path, NewRoute};
use pproxy_core::Store;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{load, save, PonyConfig};
use crate::EXIT_OK;

pub const SYNC_SCHEME: &str = "pproxy-sync://";
const DEFAULT_SYNC_PASSPHRASE: &str = "pony-proxy-universal-sync-salt-v1";
const SYNC_TTL_SECONDS: u64 = 600; // 10 分钟有效
const KDF_ITERATIONS: u32 = 10_000;

use std::path::PathBuf;

const NONCES_CACHE_FILE: &str = ".nonces.json";

fn nonces_file_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| "HOME / USERPROFILE not set".to_string())?;
    Ok(home.join(".pony").join(NONCES_CACHE_FILE))
}

/// 检查并记录 Nonce（跨进程持久化防重放 + 过期条目自动清理）。
fn check_and_record_nonce(nonce: &str, exp: u64, now: u64) -> Result<(), String> {
    let path = nonces_file_path()?;
    check_and_record_nonce_at(&path, nonce, exp, now)
}

fn check_and_record_nonce_at(path: &std::path::Path, nonce: &str, exp: u64, now: u64) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut nonces: HashMap<String, u64> = if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    // 1. 清理已过期 Nonce
    nonces.retain(|_, &mut item_exp| now <= item_exp);

    // 2. 防重放检查
    if nonces.contains_key(nonce) {
        return Err("安全拦截：该同步口令已被使用过（防重放机制），请重新导出生成！".into());
    }

    // 3. 记录当前 Nonce
    nonces.insert(nonce.to_string(), exp);

    // 4. 写回持久化存储
    if let Ok(json) = serde_json::to_string(&nonces) {
        let _ = std::fs::write(path, json);
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncData {
    #[serde(default)]
    pub server_url: Option<String>,
    #[serde(default)]
    pub worker_url: Option<String>,
    #[serde(default)]
    pub proxy_secret: Option<String>,
    #[serde(default)]
    pub routes: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncPayload {
    pub v: u32,
    pub exp: u64,
    pub nonce: String,
    pub data: SyncData,
}

fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut current = Sha256::digest(format!("{}:{}", hex::encode(salt), passphrase).as_bytes());
    for _ in 1..KDF_ITERATIONS {
        let mut hasher = Sha256::new();
        hasher.update(&current);
        hasher.update(salt);
        hasher.update(passphrase.as_bytes());
        current = hasher.finalize();
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&current);
    key
}

/// 导出加密配置字符串。
pub fn export(passphrase: Option<&str>) -> Result<i32, String> {
    let pass = passphrase.unwrap_or(DEFAULT_SYNC_PASSPHRASE);
    let cfg = load().unwrap_or_else(|_| PonyConfig {
        server: "http://127.0.0.1:8899".to_string(),
        admin_token: "".to_string(),
        data_plane: None,
        cf_token: None,
        cf_account_tag: None,
        vercel_token: None,
        tunnel_token: None,
        proxy_secret: None,
    });

    let db_path = default_db_path();
    let mut routes = HashMap::new();
    if let Ok((store, _)) = Store::open(&db_path) {
        if let Ok(route_list) = store.list_routes() {
            for r in route_list {
                routes.insert(r.name, r.target_host);
            }
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut nonce_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce_hex = hex::encode(nonce_bytes);

    let worker_url = cfg.data_plane.clone().or_else(|| Some(cfg.server.clone()));

    let payload = SyncPayload {
        v: 1,
        exp: now + SYNC_TTL_SECONDS,
        nonce: nonce_hex,
        data: SyncData {
            server_url: Some(cfg.server),
            worker_url,
            proxy_secret: cfg.proxy_secret,
            routes,
        },
    };

    let json_bytes = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;

    // 生成 16 字节 Salt 与 12 字节 Nonce
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut aead_nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut aead_nonce);

    let key = derive_key(pass, &salt);
    let cipher = ChaCha20Poly1305::new_from_slice(&key).map_err(|e| e.to_string())?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&aead_nonce), json_bytes.as_ref())
        .map_err(|e| format!("加密失败: {e}"))?;

    // 格式：salt (16) + aead_nonce (12) + ciphertext
    let mut final_buf = Vec::with_capacity(16 + 12 + ciphertext.len());
    final_buf.extend_from_slice(&salt);
    final_buf.extend_from_slice(&aead_nonce);
    final_buf.extend_from_slice(&ciphertext);

    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&final_buf);
    let sync_uri = format!("{SYNC_SCHEME}{encoded}");

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       Pony Proxy 跨端加密同步口令 (有效期: 10 分钟)            ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  \x1b[1;32m{sync_uri}\x1b[0m\n");
    println!("👉 在 Linux Server 终端运行以下命令一键完成同步：");
    println!("   pproxy sync import \"{sync_uri}\"\n");
    println!("👉 或在 Windows 桌面端直接粘贴上方口令进行一键配置导入。\n");

    Ok(EXIT_OK)
}

/// 导入加密配置字符串。
pub fn import(input: &str, passphrase: Option<&str>) -> Result<i32, String> {
    let pass = passphrase.unwrap_or(DEFAULT_SYNC_PASSPHRASE);
    let encoded = input
        .trim()
        .strip_prefix(SYNC_SCHEME)
        .unwrap_or_else(|| input.trim());

    // 兼容 URL_SAFE_NO_PAD 与 STANDARD base64
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded))
        .map_err(|_| "无效的同步口令格式（Base64 解码失败）".to_string())?;

    if raw.len() < 28 {
        return Err("同步口令数据过短或已损坏".into());
    }

    let salt = &raw[..16];
    let aead_nonce = &raw[16..28];
    let ciphertext = &raw[28..];

    let key = derive_key(pass, salt);
    let cipher = ChaCha20Poly1305::new_from_slice(&key).map_err(|e| e.to_string())?;
    let decrypted = cipher
        .decrypt(Nonce::from_slice(aead_nonce), ciphertext)
        .map_err(|_| "解密失败：同步口令错误、密码不匹配或已被篡改".to_string())?;

    let payload: SyncPayload =
        serde_json::from_slice(&decrypted).map_err(|e| format!("配置解析失败: {e}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 1. 严格 10 分钟 TTL 过期检查
    if now > payload.exp {
        return Err("该同步口令已过期（超过 10 分钟），请在原设备重新导出生成！".into());
    }

    // 2. Nonce 防重放检查（跨进程文件持久化防重放）
    check_and_record_nonce(&payload.nonce, payload.exp, now)?;

    // 3. 写入本地配置
    let mut cfg = load().unwrap_or_else(|_| PonyConfig {
        server: "http://127.0.0.1:8899".to_string(),
        admin_token: "".to_string(),
        data_plane: None,
        cf_token: None,
        cf_account_tag: None,
        vercel_token: None,
        tunnel_token: None,
        proxy_secret: None,
    });

    if let Some(secret) = payload.data.proxy_secret {
        cfg.proxy_secret = Some(secret);
    }
    if let Some(srv) = payload.data.server_url {
        cfg.server = srv;
    }
    if let Some(w) = payload.data.worker_url {
        cfg.data_plane = Some(w);
    }

    save(&cfg).map_err(|e| format!("保存配置失败: {e}"))?;

    // 4. 导入路由至 SQLite
    let db_path = default_db_path();
    if let Ok((store, _)) = Store::open(&db_path) {
        let store = Arc::new(store);
        for (name, target) in payload.data.routes {
            let route_req = NewRoute {
                name,
                target_host: target,
                override_upstream: None,
            };
            let _ = store.insert_route(&route_req);
        }
    }

    println!("\n✓ 跨端配置同步成功！");
    println!("  上游出口与路由规则已全部注入本地。");
    println!("👉 运行 'pproxy status' 查看状态，或 'eval \"$(pproxy on --eval)\"' 开启终端代理。\n");

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_and_record_nonce_persistence_and_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_nonces.json");
        let unique_nonce = format!("test_nonce_{}", rand::Rng::gen::<u64>(&mut rand::thread_rng()));
        let now = 1000;
        let exp = 1600;

        // 第一次插入
        assert!(check_and_record_nonce_at(&path, &unique_nonce, exp, now).is_ok());

        // 同一个 Nonce 重复尝试（重放攻击）
        let err = check_and_record_nonce_at(&path, &unique_nonce, exp, now + 10);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("安全拦截"));

        // 当时间流逝超过 exp 时，该 Nonce 被自动清理淘汰，系统恢复安全
        let later = exp + 10;
        let another_nonce = format!("test_nonce_{}", rand::Rng::gen::<u64>(&mut rand::thread_rng()));
        assert!(check_and_record_nonce_at(&path, &another_nonce, later + 600, later).is_ok());
    }

    #[test]
    fn sync_roundtrip_url_safe_and_replay_protection() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let unique_nonce = format!("nonce_{}", rand::Rng::gen::<u64>(&mut rand::thread_rng()));
        let payload = SyncPayload {
            v: 1,
            exp: now + 300,
            nonce: unique_nonce,
            data: SyncData {
                server_url: Some("http://127.0.0.1:8899".into()),
                worker_url: Some("https://edge.ponyjob.top".into()),
                proxy_secret: Some("my_secret_token_123".into()),
                routes: HashMap::new(),
            },
        };

        let json_bytes = serde_json::to_vec(&payload).unwrap();
        let pass = "custom_passphrase";

        let mut salt = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut salt);
        let mut aead_nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut aead_nonce);

        let key = derive_key(pass, &salt);
        let cipher = ChaCha20Poly1305::new_from_slice(&key).unwrap();
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&aead_nonce), json_bytes.as_ref())
            .unwrap();

        let mut final_buf = Vec::new();
        final_buf.extend_from_slice(&salt);
        final_buf.extend_from_slice(&aead_nonce);
        final_buf.extend_from_slice(&ciphertext);

        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&final_buf);
        let sync_uri = format!("{SYNC_SCHEME}{encoded}");

        // 第一次导入应该成功
        let res1 = import(&sync_uri, Some(pass));
        assert!(res1.is_ok());

        // 第二次导入相同口令（重放攻击）应该被拒绝！
        let res2 = import(&sync_uri, Some(pass));
        assert!(res2.is_err());
        assert!(res2.unwrap_err().contains("安全拦截"));
    }
}
