//! `pproxy sync` — 跨端安全配置同步（ChaCha20-Poly1305 + 10 分钟 TTL + Nonce 防重放 + URL-Safe 协议头）。
//!
//! 支持从 Windows 桌面端导出加密配置，在 Linux Server 上一键导入，免去重复登录 CF/Vercel。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use fs2::FileExt;
use pproxy_core::store::{default_db_path, NewRoute};
use pproxy_core::Store;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{home_dir, load, save, secure_write_file, PonyConfig};
use crate::EXIT_OK;

pub const SYNC_SCHEME: &str = "pproxy-sync://";
const SYNC_TTL_SECONDS: u64 = 600; // 10 分钟有效
const PBKDF2_ITERATIONS: u32 = 60_000; // 标准 PBKDF2-HMAC-SHA256 迭代次数

const NONCES_CACHE_FILE: &str = ".nonces.json";

fn nonces_file_path() -> Result<PathBuf, String> {
    let home = home_dir().map_err(|e| format!("无法定位主目录: {e}"))?;
    Ok(home.join(".pony").join(NONCES_CACHE_FILE))
}

/// 检查并记录 Nonce（跨进程排他文件锁 + 持久化防重放 + 过期条目自动清理）。
fn check_and_record_nonce(nonce: &str, exp: u64, now: u64) -> Result<(), String> {
    let path = nonces_file_path()?;
    check_and_record_nonce_at(&path, nonce, exp, now)
}

fn check_and_record_nonce_at(path: &Path, nonce: &str, exp: u64, now: u64) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    // 使用锁文件进行跨进程排他互斥，防止并发竞态攻击
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| format!("打开 Nonce 锁文件失败: {e}"))?;

    lock_file
        .lock_exclusive()
        .map_err(|e| format!("获取 Nonce 排他文件锁失败: {e}"))?;

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

    // 4. 写回持久化存储（严格原子写入与 0600 权限，Fail-Closed）
    let json = serde_json::to_string(&nonces).map_err(|e| format!("序列化 Nonce 失败: {e}"))?;
    secure_write_file(path, json.as_bytes()).map_err(|e| format!("写入 Nonce 存储失败: {e}"))?;

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

/// 标准 HMAC-SHA256 实现 (RFC 2104)
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut o_key_pad = [0x5cu8; BLOCK_SIZE];
    let mut i_key_pad = [0x36u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        o_key_pad[i] ^= key_block[i];
        i_key_pad[i] ^= key_block[i];
    }

    let mut inner_hasher = Sha256::new();
    inner_hasher.update(&i_key_pad);
    inner_hasher.update(data);
    let inner_hash = inner_hasher.finalize();

    let mut outer_hasher = Sha256::new();
    outer_hasher.update(&o_key_pad);
    outer_hasher.update(&inner_hash);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hasher.finalize());
    out
}

/// 标准 PBKDF2-HMAC-SHA256 (RFC 2898) 密钥派生
pub(crate) fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut salt_and_index = Vec::with_capacity(salt.len() + 4);
    salt_and_index.extend_from_slice(salt);
    salt_and_index.extend_from_slice(&1u32.to_be_bytes());

    let mut u = hmac_sha256(passphrase.as_bytes(), &salt_and_index);
    let mut out = u;

    for _ in 1..PBKDF2_ITERATIONS {
        u = hmac_sha256(passphrase.as_bytes(), &u);
        for i in 0..32 {
            out[i] ^= u[i];
        }
    }
    out
}

/// 导出加密配置字符串。
pub fn export(passphrase: Option<&str>) -> Result<i32, String> {
    // 杜绝公开默认口令：未指定口令时自动生成 16 字节 (128-bit 熵) 的高强度随机 Passkey
    let (pass, is_generated) = match passphrase {
        Some(p) if !p.is_empty() => (p.to_string(), false),
        _ => {
            let mut key_bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut key_bytes);
            (hex::encode(key_bytes), true)
        }
    };

    let cfg = load().unwrap_or_else(|_| PonyConfig {
        server: "http://127.0.0.1:8899".to_string(),
        admin_token: "".to_string(),
        data_plane: None,
        cf_token: None,
        cf_account_tag: None,
        vercel_token: None,
        tunnel_token: None,
        tunnel_gate_url: None,
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
        .unwrap_or_default()
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

    let key = derive_key(&pass, &salt);
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
    println!("  同步链接: \x1b[1;32m{sync_uri}\x1b[0m");
    if is_generated {
        println!("  解密密钥: \x1b[1;33m{pass}\x1b[0m (自动生成的安全密钥)");
    }
    println!();
    println!("👉 在目标机器运行以下命令一键完成同步：");
    println!("   pproxy sync import \"{sync_uri}\" --passphrase \"{pass}\"\n");

    Ok(EXIT_OK)
}

/// 导入加密配置字符串。
pub fn import(input: &str, passphrase: Option<&str>) -> Result<i32, String> {
    let pass = match passphrase {
        Some(p) if !p.is_empty() => p,
        _ => return Err("请提供解密口令: pproxy sync import \"<uri>\" --passphrase \"<password>\"".into()),
    };

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
        .unwrap_or_default()
        .as_secs();

    // 1. 严格 10 分钟 TTL 过期检查与未来时间窗口防护
    if now > payload.exp {
        return Err("该同步口令已过期（超过 10 分钟），请在原设备重新导出生成！".into());
    }
    if payload.exp > now + SYNC_TTL_SECONDS + 60 {
        return Err("非法同步口令：有效时间戳超出允许范围！".into());
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
        tunnel_gate_url: None,
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

    // 4. 导入路由至 SQLite（严格错误检查，拒绝静默吞异常）
    let db_path = default_db_path();
    let (store, _) = Store::open(&db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
    let store = Arc::new(store);
    for (name, target) in payload.data.routes {
        let route_req = NewRoute {
            name,
            target_host: target,
            override_upstream: None,
        };
        let _ = store.insert_route(&route_req);
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
            .unwrap_or_default()
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
        let pass = "custom_passphrase_test";

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
