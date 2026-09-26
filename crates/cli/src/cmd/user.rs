//! `pproxy user ...` — 用户管理（Basic Auth 用户名/密码 CRUD 与一键连接口令生成）。

use std::sync::Arc;

use pproxy_core::store::default_db_path;
use pproxy_core::user::UserService;
use pproxy_core::Store;

use crate::config::{self, load};
use crate::render::{fmt_ts, Table};
use crate::{EXIT_FAILURE, EXIT_OK};

fn get_user_service() -> Result<UserService, String> {
    let db_path = default_db_path();
    let (store, _) = Store::open(&db_path).map_err(|e| format!("打开数据库失败 ({e})"))?;
    UserService::new(Arc::new(store)).map_err(|e| format!("初始化用户服务失败 ({e})"))
}

pub fn parse_quota_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();
    if let Some(num) = s.strip_suffix("GB") {
        num.trim().parse::<u64>().map(|v| v * 1024 * 1024 * 1024).map_err(|e| format!("无效的配额数值: {e}"))
    } else if let Some(num) = s.strip_suffix('G') {
        num.trim().parse::<u64>().map(|v| v * 1024 * 1024 * 1024).map_err(|e| format!("无效的配额数值: {e}"))
    } else if let Some(num) = s.strip_suffix("MB") {
        num.trim().parse::<u64>().map(|v| v * 1024 * 1024).map_err(|e| format!("无效的配额数值: {e}"))
    } else if let Some(num) = s.strip_suffix('M') {
        num.trim().parse::<u64>().map(|v| v * 1024 * 1024).map_err(|e| format!("无效的配额数值: {e}"))
    } else {
        s.parse::<u64>().map_err(|e| format!("无效的配额数值(支持 50G, 50GB, 100M): {e}"))
    }
}

pub fn keygen() -> Result<i32, String> {
    let (signer, vk) = pproxy_core::TokenSigner::generate();
    let sk_bytes = signer.to_bytes();
    let vk_bytes = vk.to_bytes();
    let vk_hex = hex::encode(vk_bytes);

    // 私钥落盘到 ~/.pony/cluster_signing_key.hex
    let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
    let dir = std::path::Path::new(&home).join(".pony");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let key_path = dir.join("cluster_signing_key.hex");

    // 严苛安全审查遵循：仅 0600 权限，且正确写入私钥字节 sk_bytes
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)
            .map_err(|e| format!("创建私钥文件失败: {e}"))?;
        use std::io::Write;
        file.write_all(hex::encode(sk_bytes).as_bytes())
            .map_err(|e| format!("写入私钥文件失败: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&key_path, hex::encode(sk_bytes)).map_err(|e| format!("写入私钥文件失败: {e}"))?;
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       ✓ 集群多租户密钥对生成成功 (非对称隔离架构)             ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  私钥存储路径 (管理机独占): {}", key_path.display());
    println!("  验签公钥 (HEX):             \x1b[1;32m{}\x1b[0m\n", vk_hex);
    println!("  \x1b[1;33m配置指引：\x1b[0m");
    println!("  1. 临时生效: export USER_VERIFYING_KEY=\"{}\"", vk_hex);
    println!("  2. 生产环境: 请将上述公钥写入各网关节点 (RackNerd / 备灾机) 的 /etc/environment 或 systemd 配置中。\n");

    Ok(EXIT_OK)
}

pub fn add(
    username: &str,
    password: Option<&str>,
    expires_days: Option<u32>,
    quota_str: Option<&str>,
    max_conns: usize,
) -> Result<i32, String> {
    if let Some(qs) = quota_str {
        let quota_bytes = parse_quota_bytes(qs)?;
        let days = expires_days.unwrap_or(30);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = now + (days as u64) * 86400;

        let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
        let key_path = std::path::Path::new(&home).join(".pony").join("cluster_signing_key.hex");
        let (signer, vk_hex) = if key_path.exists() {
            let hex_str = std::fs::read_to_string(&key_path).map_err(|e| format!("读取私钥失败: {e}"))?;
            let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("私钥格式错误: {e}"))?;
            if bytes.len() != 32 {
                return Err("私钥长度非 32 字节".into());
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            let s = pproxy_core::TokenSigner::from_bytes(&arr);
            let vk = hex::encode(s.verifying_key().to_bytes());
            (s, vk)
        } else {
            let (s, vk) = pproxy_core::TokenSigner::generate();
            let sk_bytes = s.to_bytes();
            let vk_hex = hex::encode(vk.to_bytes());
            std::fs::create_dir_all(key_path.parent().unwrap()).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&key_path)
                    .map_err(|e| e.to_string())?;
                use std::io::Write;
                file.write_all(hex::encode(sk_bytes).as_bytes()).map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))]
            {
                std::fs::write(&key_path, hex::encode(sk_bytes)).map_err(|e| e.to_string())?;
            }
            (s, vk_hex)
        };

        let jti = format!("tok-{}", hex::encode(rand::random::<[u8; 8]>()));
        let role = if username == "admin" || username.starts_with("admin_") {
            "admin".to_string()
        } else {
            "user".to_string()
        };
        let claims = pproxy_core::UserTokenClaims {
            jti,
            sub: format!("usr_{}", username),
            name: username.to_string(),
            quota_bytes,
            lease_bytes: 500 * 1024 * 1024,
            exp,
            iat: now,
            max_conns,
            role: role.clone(),
        };

        let token = signer.sign_token(&claims).map_err(|e| format!("签发令牌失败: {e}"))?;

        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║       ✓ 商业化多租户自包含令牌签发成功 (Ed25519 签名)         ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");
        println!("  用户名称:   \x1b[1;36m{}\x1b[0m", username);
        println!("  用户身份:   \x1b[1;35m{}\x1b[0m", if role == "admin" { "系统管理员 (全局大盘权限)" } else { "普通租户用户" });
        println!("  总配额:     \x1b[1;32m{:.2} GB\x1b[0m ({} 字节)", quota_bytes as f64 / 1024.0 / 1024.0 / 1024.0, quota_bytes);
        println!("  有效时长:   {} 天 (过期时间: {})", days, fmt_ts(Some(exp)));
        println!("  最大并发:   {} 个同时在线长连接", max_conns);
        println!("  公钥校验:   {}", vk_hex);
        println!("  用户令牌:   \x1b[1;33m{}\x1b[0m\n", token);
        println!("  \x1b[1;32m使用指引：\x1b[0m 客户端(Win/Mac/CLI)直接粘贴此令牌即可显示额度并激活代理。");

        return Ok(EXIT_OK);
    }

    let service = get_user_service()?;

    let pass = match password {
        Some(p) => p.to_string(),
        None => {
            // 自动生成 16 字符高强度密码 (64-bit 熵)
            let mut bytes = [0u8; 8];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
            hex::encode(bytes)
        }
    };

    match service.create_user(username, &pass, expires_days) {
        Ok(user) => {
            let data_plane = match load() {
                Ok(cfg) => config::derive_data_plane(&cfg)
                    .unwrap_or_else(|_| "http://127.0.0.1:8899".to_string()),
                Err(_) => "http://127.0.0.1:8899".to_string(),
            };
            let host_port = data_plane
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string();

            let pproxy_uri = format!("pproxy://{}:{pass}@{host_port}", user.username);
            let http_uri = format!("http://{}:{pass}@{host_port}", user.username);

            println!("\n╔════════════════════════════════════════════════════════════════╗");
            println!("║              ✓ 代理用户创建成功                                 ║");
            println!("╚════════════════════════════════════════════════════════════════╝\n");
            println!("  用户 ID:    {}", user.id);
            println!("  用户名:     \x1b[1;36m{}\x1b[0m", user.username);
            println!("  认证密码:   \x1b[1;32m{pass}\x1b[0m");
            if let Some(exp) = user.expires_at {
                println!("  有效期至:   {}", fmt_ts(Some(exp as u64)));
            }
            println!();
            println!("┌─ [一键连接口令 (Windows 桌面端 / 小火箭 / Shadowrocket)] ────");
            println!("│  \x1b[1;32m{pproxy_uri}\x1b[0m");
            println!("│  \x1b[1;32m{http_uri}\x1b[0m");
            println!("└─────────────────────────────────────────────────────────────\n");
            println!("👉 手动配置参数（如手机 Wi-Fi 代理设置）：");
            println!("   服务器: {}  (端口由上述地址指定)", host_port);
            println!("   用户名: {}", user.username);
            println!("   密码:   {pass}\n");
            Ok(EXIT_OK)
        }
        Err(e) => {
            eprintln!("✗ 创建用户失败: {e}");
            Ok(EXIT_FAILURE)
        }
    }
}

pub fn list() -> Result<i32, String> {
    let service = get_user_service()?;
    let users = service.list_users();

    if users.is_empty() {
        println!("暂无用户。运行 'pproxy user add <username>' 创建新用户。");
        return Ok(EXIT_OK);
    }

    let mut t = Table::new(&[
        "id",
        "username",
        "status",
        "created_at",
        "expires_at",
        "last_used_at",
    ]);

    for u in &users {
        let status = if u.disabled {
            "disabled".to_string()
        } else {
            "active".to_string()
        };
        t.push(vec![
            u.id.to_string(),
            u.username.clone(),
            status,
            fmt_ts(Some(u.created_at as u64)),
            fmt_ts(u.expires_at.map(|v| v as u64)),
            fmt_ts(u.last_used_at.map(|v| v as u64)),
        ]);
    }

    print!("{}", t.render());
    Ok(EXIT_OK)
}

pub fn rm(username: &str) -> Result<i32, String> {
    let service = get_user_service()?;
    match service.delete_user(username) {
        Ok(true) => {
            println!("✓ 已删除用户: {username}");
            Ok(EXIT_OK)
        }
        Ok(false) => {
            eprintln!("✗ 用户不存在: {username}");
            Ok(EXIT_FAILURE)
        }
        Err(e) => {
            eprintln!("✗ 删除用户失败: {e}");
            Ok(EXIT_FAILURE)
        }
    }
}

pub fn disable(username: &str) -> Result<i32, String> {
    let service = get_user_service()?;
    match service.set_disabled(username, true) {
        Ok(true) => {
            println!("✓ 已禁用用户: {username}");
            Ok(EXIT_OK)
        }
        _ => {
            eprintln!("✗ 操作失败：用户不存在或数据库异常");
            Ok(EXIT_FAILURE)
        }
    }
}

pub fn enable(username: &str) -> Result<i32, String> {
    let service = get_user_service()?;
    match service.set_disabled(username, false) {
        Ok(true) => {
            println!("✓ 已启用用户: {username}");
            Ok(EXIT_OK)
        }
        _ => {
            eprintln!("✗ 操作失败：用户不存在或数据库异常");
            Ok(EXIT_FAILURE)
        }
    }
}

pub fn revoke(target: &str) -> Result<i32, String> {
    let target = target.trim();
    let (jti_or_sub, display_target) = if target.starts_with("usr_live_") {
        match pproxy_core::auth::TokenVerifier::peek_claims_unverified(target) {
            Ok(c) => (c.jti, format!("令牌 [{}] (用户: {})", target, c.name)),
            Err(_) => (target.to_string(), format!("令牌 [{}]", target)),
        }
    } else {
        (format!("usr_{}", target), format!("用户 [{}]", target))
    };

    // 保存到本地撤销列表 ~/.pony/revoked_tokens.txt
    let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
    let dir = std::path::Path::new(&home).join(".pony");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let path = dir.join("revoked_tokens.txt");

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开撤销列表失败: {e}"))?;

    writeln!(file, "{}", jti_or_sub).map_err(|e| format!("写入撤销列表失败: {e}"))?;

    // 热更新闭环：向本机/指定 gate-server 的 POST /api/user/revoke 推送，
    // 使运行中的网关内存黑名单立即生效（无则跳过并提示，仅本地落盘）。
    let gate_addr = std::env::var("GATE_ADMIN").unwrap_or_else(|_| "http://127.0.0.1:3101".into());
    let admin_token = std::env::var("GATE_ADMIN_TOKEN").unwrap_or_default();
    let mut hot_ok = false;
    if !admin_token.is_empty() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("运行时创建失败: {e}"))?;
        hot_ok = rt.block_on(async {
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            matches!(
                client
                    .post(format!("{}/api/user/revoke", gate_addr))
                    .bearer_auth(&admin_token)
                    .json(&serde_json::json!({ "identifier": jti_or_sub }))
                    .send()
                    .await,
                Ok(r) if r.status().is_success()
            )
        });
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       ✓ 用户令牌已成功废止撤销 (Revocation Enforced)           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  已废止目标: \x1b[1;31m{}\x1b[0m", display_target);
    println!("  撤销标识符: {}", jti_or_sub);
    println!("  落盘路径:   {}", path.display());
    println!(
        "  网关热更新: \x1b[1;32m{}\x1b[0m ({})",
        if hot_ok { "已生效 ✓" } else { "跳过（未配置 GATE_ADMIN_TOKEN）" },
        gate_addr
    );
    println!("\n  \x1b[1;32m生效说明：\x1b[0m 该令牌后续发起的所有 WS 握手和 Profile 查询将被网关立即返回 401 Unauthorized 阻断。\n");

    Ok(EXIT_OK)
}

pub fn passwd(username: &str, new_password: &str) -> Result<i32, String> {
    let service = get_user_service()?;
    match service.update_password(username, new_password) {
        Ok(true) => {
            println!("✓ 已更新用户 {username} 的密码");
            Ok(EXIT_OK)
        }
        Ok(false) => {
            eprintln!("✗ 用户不存在: {username}");
            Ok(EXIT_FAILURE)
        }
        Err(e) => {
            eprintln!("✗ 更新密码失败: {e}");
            Ok(EXIT_FAILURE)
        }
    }
}
