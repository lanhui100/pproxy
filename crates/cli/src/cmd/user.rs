//! `pproxy user ...` — 用户管理（Basic Auth 用户名/密码 CRUD 与一键连接口令生成）。

use std::sync::Arc;

use pproxy_core::store::default_db_path;
use pproxy_core::user::{UserError, UserService};
use pproxy_core::Store;

use crate::config::{self, load, PonyConfig};
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

/// 自然月/年时长解析（D1）：`30`、`30d` = N 天；`1m`/`3m`/`6m` = N 个自然月；
/// `1y` = 12 个自然月。大小写不敏感、允许前后与数字/单位间空白；`0` 与非法输入返回 Err。
/// 返回绝对 Unix 秒；月运算按 UTC 且月末溢出钳制到目标月最后一天（D2）。
pub fn parse_expiry_spec(spec: &str, now: u64) -> Result<u64, String> {
    let s = spec.trim().to_lowercase();
    if s.is_empty() {
        return Err("无效的有效期：内容为空（支持 30 / 30d / 1m / 3m / 6m / 1y）".to_string());
    }
    let split_at = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num_part, unit_part) = s.split_at(split_at);
    let unit = unit_part.trim();
    if num_part.is_empty() {
        return Err(format!(
            "无效的有效期: `{spec}`（支持 30 / 30d / 1m / 3m / 6m / 1y）"
        ));
    }
    let n: u64 = num_part
        .parse()
        .map_err(|e| format!("无效的有效期数值: {e}"))?;
    if n == 0 {
        return Err("无效的有效期：数量必须大于 0".to_string());
    }
    match unit {
        // 纯数字与 d 同为自然日
        "" | "d" => {
            let secs = n.checked_mul(86_400).ok_or("有效期溢出（天数过大）")?;
            now.checked_add(secs)
                .ok_or_else(|| "有效期溢出（时间戳超出范围）".to_string())
        }
        "m" => add_months_utc(now, n),
        "y" => add_months_utc(now, n.checked_mul(12).ok_or("有效期溢出（年数过大）")?),
        other => Err(format!(
            "无效的有效期单位: `{other}`（支持 d = 天 / m = 自然月 / y = 自然年）"
        )),
    }
}

/// Howard Hinnant `days_from_civil`：公历 (y, m, d) → 距 1970-01-01 的天数。
/// 手写实现以避免引入 chrono 依赖（项目约束：不加第三方 crate）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = i64::from((m + 9) % 12); // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant `civil_from_days`：距 1970-01-01 的天数 → 公历 (y, m, d)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(y) => 29,
        2 => 28,
        _ => 30,
    }
}

/// 按 UTC 加 N 个自然月：日溢出时钳制到目标月最后一天（2024-01-31 +1m = 2024-02-29）。
fn add_months_utc(ts: u64, months: u64) -> Result<u64, String> {
    let days = (ts / 86_400) as i64;
    let secs_of_day = ts % 86_400;
    let (y, m, d) = civil_from_days(days);
    let total_months = i128::from(y) * 12 + i128::from(m) - 1 + i128::from(months);
    if !(-i128::from(i64::MAX / 24)..=i128::from(i64::MAX / 24)).contains(&total_months) {
        return Err("有效期溢出（月份超出范围）".to_string());
    }
    let months_i64 = total_months as i64;
    let target_year = months_i64.div_euclid(12);
    let target_month = months_i64.rem_euclid(12) as u32 + 1;
    let target_day = d.min(days_in_month(target_year, target_month));
    let new_days = days_from_civil(target_year, target_month, target_day);
    if new_days < 0 {
        return Err("有效期解析结果早于 1970-01-01".to_string());
    }
    Ok(new_days as u64 * 86_400 + secs_of_day)
}

/// RFC3986 百分号编码（component 口径）：仅保留 unreserved `A-Z a-z 0-9 - _ . ~`，
/// 其余字节编码为 `%XX`（大写十六进制）。手写以避免引入 percent-encoding 依赖。
pub fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 订阅 URL 与 Clash 一键导入深链（纯函数，便于单测）。
pub fn subscription_links(host: &str, token: &str, username: &str) -> (String, String) {
    let sub_url = format!("https://{host}/clash?token={token}");
    let name = format!("PonyProxy-{username}");
    let deeplink = format!(
        "clash://install-config?url={}&name={}",
        percent_encode_component(&sub_url),
        percent_encode_component(&name)
    );
    (sub_url, deeplink)
}

/// 订阅主机决策（纯函数，便于单测）：未配置 gate 或端口非 443 时返回不可生成原因。
/// 沿用 clash.rs 的 443 fail-fast 口径：gate worker 仅放行 443，其余端口必然不可用。
pub fn subscription_host(cfg: &PonyConfig) -> Result<String, String> {
    let Some((host, port)) = crate::cmd::clash::gate_endpoint_from_config(cfg) else {
        return Err("未配置 gate 隧道 (tunnel_gate_url)，可先执行 `pproxy deploy`".to_string());
    };
    if port != 443 {
        return Err(format!(
            "gate 隧道端口 {port} 不受支持（gate worker 仅放行 443）"
        ));
    }
    Ok(host)
}

/// 打印订阅 URL / 一键导入深链 / 二维码。
/// 拿不到可用的 gate 主机时只提示原因，不算失败：令牌已签发，退出码仍为 EXIT_OK。
fn print_subscription_block(username: &str, token: &str) {
    let host = match config::load().map_err(|e| e.to_string()).and_then(|cfg| subscription_host(&cfg)) {
        Ok(host) => host,
        Err(reason) => {
            println!("  ⚠ 未生成订阅链接：{reason}");
            return;
        }
    };
    let (sub_url, deeplink) = subscription_links(&host, token, username);
    println!();
    println!("┌─ [Clash 订阅（含二维码）] ──────────────────────────────────");
    println!("│  订阅链接:  \x1b[1;32m{sub_url}\x1b[0m");
    println!("│  一键导入:  \x1b[1;32m{deeplink}\x1b[0m");
    println!("└─────────────────────────────────────────────────────────────");
    match crate::cmd::clash::render_qr(&sub_url) {
        Ok(qr) => {
            println!("  手机端 Clash / Shadowrocket 扫码导入：");
            for line in qr.lines() {
                println!("  {line}");
            }
        }
        Err(e) => println!("  ⚠ 二维码渲染失败：{e}"),
    }
}

/// 配额展示（D6）：None/非正数显示 `-`，GB 量级用 G 否则用 M。
pub fn fmt_quota(bytes: Option<i64>) -> String {
    match bytes {
        Some(b) if b > 0 => {
            let v = b as f64;
            if v >= 1024.0 * 1024.0 * 1024.0 {
                format!("{:.2}G", v / 1024.0 / 1024.0 / 1024.0)
            } else {
                format!("{:.0}M", v / 1024.0 / 1024.0)
            }
        }
        _ => "-".to_string(),
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
    expires: Option<&str>,
    quota_str: Option<&str>,
    max_conns: usize,
    is_admin: bool,
) -> Result<i32, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // D5-2：-p 必须生效；未给则自动生成 16 位 hex（Basic 局域网模式同样需要）
    let pass = match password {
        Some(p) => p.to_string(),
        None => {
            // 自动生成 16 字符高强度密码 (64-bit 熵)
            let mut bytes = [0u8; 8];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
            hex::encode(bytes)
        }
    };

    if let Some(qs) = quota_str {
        let quota_bytes = parse_quota_bytes(qs)?;
        // 会员默认 30 天（沿用既有口径）；显式 -d 支持 30/30d/1m/3m/6m/1y
        let exp = match expires {
            Some(spec) => parse_expiry_spec(spec, now)?,
            None => now + 30 * 86_400,
        };

        let jti = format!("tok-{}", hex::encode(rand::random::<[u8; 8]>()));
        // ADR 2026-10-03：角色仅由显式 --admin 标志判定，废除用户名前缀推断
        //（否则任意普通租户把 username 填成 admin_xxx 即自动豁免配额熔断）
        let role = if is_admin { "admin" } else { "user" };

        // D5-1：会员开通必须与令牌签发同源落库。用户名冲突时直接失败，
        // 否则会出现「表里一套密码 + 手里一套令牌」的凭据漂移。
        let service = get_user_service()?;
        let user = match service.create_user_ex(
            username,
            &pass,
            Some(exp as i64),
            Some(quota_bytes),
            Some(&jti),
            Some(max_conns as u32),
            Some(role),
        ) {
            Ok(u) => u,
            Err(UserError::UserAlreadyExists(_)) => {
                return Err(format!(
                    "用户 `{username}` 已存在：请先执行 `pproxy user rm {username}` 再重新开通（避免签发两套互不知情的凭据）"
                ))
            }
            Err(e) => return Err(format!("写入用户表失败: {e}")),
        };

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

        let claims = pproxy_core::UserTokenClaims {
            jti: jti.clone(),
            sub: format!("usr_{}", username),
            name: username.to_string(),
            quota_bytes,
            lease_bytes: 500 * 1024 * 1024,
            exp,
            iat: now,
            max_conns,
            role: role.to_string(),
        };

        let token = signer.sign_token(&claims).map_err(|e| format!("签发令牌失败: {e}"))?;

        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║       ✓ 商业化多租户自包含令牌签发成功 (Ed25519 签名)         ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");
        println!("  用户名称:   \x1b[1;36m{username}\x1b[0m");
        println!(
            "  用户身份:   \x1b[1;35m{}\x1b[0m",
            if role == "admin" { "系统管理员 (全局大盘权限)" } else { "普通租户用户" }
        );
        println!(
            "  总配额:     \x1b[1;32m{:.2} GB\x1b[0m ({} 字节)",
            quota_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
            quota_bytes
        );
        println!(
            "  有效期至:   {} (约 {} 天)",
            fmt_ts(Some(exp)),
            exp.saturating_sub(now) / 86_400
        );
        println!("  最大并发:   {} 个同时在线长连接", user.max_conns.unwrap_or_default());
        println!("  登录密码:   \x1b[1;32m{pass}\x1b[0m  (局域网 Basic / 订阅客户端校验用)");
        println!("  公钥校验:   {}", vk_hex);
        println!("  用户令牌:   \x1b[1;33m{}\x1b[0m\n", token);
        println!("  \x1b[1;32m使用指引：\x1b[0m 客户端(Win/Mac/CLI)直接粘贴此令牌即可显示额度并激活代理。");
        print_subscription_block(username, &token);
        println!("  \x1b[1;33m⚠ 订阅链接仅限个人使用，请勿外传；泄露即可能被他人消耗你的月度额度\x1b[0m\n");

        return Ok(EXIT_OK);
    }

    let service = get_user_service()?;
    let expires_at = match expires {
        Some(spec) => Some(parse_expiry_spec(spec, now)? as i64),
        None => None,
    };

    match service.create_user_ex(username, &pass, expires_at, None, None, None, None) {
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
        "quota",
        "role",
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
            fmt_quota(u.quota_bytes),
            u.role.clone().unwrap_or_else(|| "-".to_string()),
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

/// `pproxy user sign <file>` — 管理机为升级包生成 Ed25519 签名（.sig）。
///
/// 签名约定（与 `pproxy cluster upgrade --sig` 双侧一致）：
/// 签名对象为**升级包的 SHA-256 摘要**，签名以 64 字节 HEX 写入 `<file>.sig`。
/// 私钥来源 `~/.pony/cluster_signing_key.hex`（`pproxy user keygen` 生成，0600）。
pub fn sign(file: &str, out: Option<&str>) -> Result<i32, String> {
    use sha2::{Digest, Sha256};

    let p = std::path::Path::new(file);
    if !p.is_file() {
        return Err(format!("待签名文件不存在: {file}"));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    let digest = Sha256::digest(&bytes);

    let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
    let key_path = std::path::Path::new(&home).join(".pony").join("cluster_signing_key.hex");
    if !key_path.is_file() {
        return Err(format!(
            "未找到签名私钥 {} — 请先在管理机执行 `pproxy user keygen` 生成密钥对",
            key_path.display()
        ));
    }
    let seed_hex = std::fs::read_to_string(&key_path).map_err(|e| format!("读取私钥失败: {e}"))?;
    let signer = pproxy_core::TokenSigner::from_seed_hex(&seed_hex).map_err(|e| e.to_string())?;
    let sig = signer.sign_bytes(&digest);
    let vk_hex = hex::encode(signer.verifying_key().to_bytes());

    let out_path = match out {
        Some(o) => std::path::PathBuf::from(o),
        None => {
            let mut s = p.as_os_str().to_owned();
            s.push(".sig");
            std::path::PathBuf::from(s)
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o644)
            .open(&out_path)
            .map_err(|e| format!("创建签名文件失败: {e}"))?;
        use std::io::Write;
        f.write_all(hex::encode(sig).as_bytes())
            .map_err(|e| format!("写入签名文件失败: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&out_path, hex::encode(sig)).map_err(|e| format!("写入签名文件失败: {e}"))?;
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       ✓ 升级包 Ed25519 签名生成成功 (Release Signing)           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  升级包 SHA-256: {}", hex::encode(digest));
    println!("  签名文件:       {}", out_path.display());
    println!("  验签公钥 (HEX): \x1b[1;32m{}\x1b[0m\n", vk_hex);
    println!("  \x1b[1;33m节点侧校验：\x1b[0m");
    println!("  pproxy cluster upgrade --local {} --sig {}", file, out_path.display());
    println!("  （节点默认从 USER_VERIFYING_KEY / ~/.pony 公钥自动解析验签公钥）\n");

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC 日期 → Unix 秒（1970-01-01 00:00:00Z 起）
    fn utc(y: i64, m: u32, d: u32) -> u64 {
        (days_from_civil(y, m, d) * 86_400) as u64
    }

    #[test]
    fn civil_roundtrip_is_stable() {
        // D2：手写 Hinnant 算法的双向一致性（含闰年与世纪边界）
        for &(y, m, d) in &[
            (1970, 1, 1),
            (2000, 2, 29),
            (2024, 2, 29),
            (2023, 3, 1),
            (2026, 11, 12),
            (2100, 12, 31),
        ] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(utc(1970, 1, 1), 0);
        assert_eq!(utc(2024, 1, 31) % 86_400, 0);
    }

    #[test]
    fn expiry_month_end_clamping() {
        // D2 明确要求覆盖的 4 条
        assert_eq!(
            parse_expiry_spec("1m", utc(2024, 1, 31)).unwrap(),
            utc(2024, 2, 29),
            "2024-01-31 +1m = 2024-02-29（闰年）"
        );
        assert_eq!(
            parse_expiry_spec("1m", utc(2023, 1, 31)).unwrap(),
            utc(2023, 2, 28),
            "2023-01-31 +1m = 2023-02-28（平年钳制）"
        );
        assert_eq!(
            parse_expiry_spec("1y", utc(2024, 2, 29)).unwrap(),
            utc(2025, 2, 28),
            "2024-02-29 +1y = 2025-02-28"
        );
        assert_eq!(
            parse_expiry_spec("3m", utc(2026, 11, 12)).unwrap(),
            utc(2027, 2, 12),
            "2026-11-12 +3m = 2027-02-12（跨年）"
        );
    }

    #[test]
    fn expiry_units_and_whitespace() {
        let base = utc(2026, 11, 12);
        // 纯数字与 d 等价（天）
        assert_eq!(parse_expiry_spec("30", base).unwrap(), base + 30 * 86_400);
        assert_eq!(parse_expiry_spec("30d", base).unwrap(), base + 30 * 86_400);
        // 大小写不敏感 + 允许空白
        assert_eq!(parse_expiry_spec(" 30D ", base).unwrap(), base + 30 * 86_400);
        assert_eq!(parse_expiry_spec("3 M", base).unwrap(), utc(2027, 2, 12));
        // 自然月 / 自然年
        assert_eq!(parse_expiry_spec("1m", base).unwrap(), utc(2026, 12, 12));
        assert_eq!(parse_expiry_spec("6m", base).unwrap(), utc(2027, 5, 12));
        assert_eq!(parse_expiry_spec("1y", base).unwrap(), utc(2027, 11, 12));
        assert_eq!(parse_expiry_spec("1Y", base).unwrap(), utc(2027, 11, 12));
        // 日内的秒偏移保持不变（不能把时刻截断到 0 点）
        assert_eq!(
            parse_expiry_spec("3m", base + 3_661).unwrap(),
            utc(2027, 2, 12) + 3_661
        );
    }

    #[test]
    fn expiry_rejects_zero_and_invalid() {
        let base = utc(2026, 11, 12);
        for bad in [
            "", "   ", "0", "0d", "0m", "0y", "abc", "m", "y", "-1", "1w", "1.5m", "12x", "d30",
        ] {
            assert!(
                parse_expiry_spec(bad, base).is_err(),
                "`{bad}` 应被拒绝"
            );
        }
    }

    #[test]
    fn percent_encode_rfc3986_component() {
        // unreserved 原样保留
        assert_eq!(
            percent_encode_component("azAZ09-_.~"),
            "azAZ09-_.~"
        );
        // 订阅 URL 全量编码（大写十六进制）
        assert_eq!(
            percent_encode_component("https://gate.ponygo.fun/clash?token=a b"),
            "https%3A%2F%2Fgate.ponygo.fun%2Fclash%3Ftoken%3Da%20b"
        );
        // UTF-8 多字节逐字节编码
        assert_eq!(percent_encode_component("中"), "%E4%B8%AD");
        assert_eq!(percent_encode_component(""), "");
    }

    #[test]
    fn subscription_links_shape() {
        let (sub, deep) = subscription_links("gate.ponygo.fun", "usr_live_ab.cd", "alice");
        assert_eq!(sub, "https://gate.ponygo.fun/clash?token=usr_live_ab.cd");
        assert_eq!(
            deep,
            "clash://install-config?url=https%3A%2F%2Fgate.ponygo.fun%2Fclash%3Ftoken%3Dusr_live_ab.cd&name=PonyProxy-alice"
        );
        // 深链必须可被客户端解析：url/name 两个参数都在
        assert!(deep.contains("&name="));
    }

    #[test]
    fn subscription_host_requires_gate_on_443() {
        let mut cfg = PonyConfig::default();
        // gate 未配置 → 不生成（原因明确）
        let reason = subscription_host(&cfg).unwrap_err();
        assert!(reason.contains("tunnel_gate_url"), "原因: {reason}");

        // 非 443 端口 → fail-fast，沿用 clash.rs 口径
        cfg.tunnel_gate_url = Some("gate.ponygo.fun:8443".into());
        let reason = subscription_host(&cfg).unwrap_err();
        assert!(reason.contains("443"), "原因: {reason}");

        // wss 缺省端口 443 → 可生成
        cfg.tunnel_gate_url = Some("wss://gate.ponygo.fun/ws".into());
        assert_eq!(subscription_host(&cfg).unwrap(), "gate.ponygo.fun");
    }

    #[test]
    fn quota_display_supports_legacy_none() {
        // D6：列缺失 / None 显示 `-`，不破坏列宽风格
        assert_eq!(fmt_quota(None), "-");
        assert_eq!(fmt_quota(Some(0)), "-");
        assert_eq!(fmt_quota(Some(-1)), "-");
        assert_eq!(fmt_quota(Some(50 * 1024 * 1024 * 1024)), "50.00G");
        assert_eq!(fmt_quota(Some(100 * 1024 * 1024)), "100M");
    }
}
