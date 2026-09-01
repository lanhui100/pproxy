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

pub fn add(
    username: &str,
    password: Option<&str>,
    expires_days: Option<u32>,
) -> Result<i32, String> {
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
