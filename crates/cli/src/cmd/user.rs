//! `pproxy user ...` — 用户管理（Basic Auth 用户名/密码 CRUD 与一键连接口令生成）。

use std::path::PathBuf;
use std::sync::Arc;

use pproxy_core::store::{default_db_path, UserRow};
use pproxy_core::user::{UserError, UserService};
use pproxy_core::Store;

use crate::cmd::gate_admin;
use crate::config::{self, load, PonyConfig};
use crate::render::{fmt_ts, human_bytes, Table};
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

/// 自然月/年/短周期时长解析（D1、D5）：`30`、`30d` = N 天；`1m`/`3m`/`6m` = N 个自然月；
/// `1y` = 12 个自然月；`30s`/`5min`/`2h` = N 秒/分钟/小时（短周期联调与到期验收用）。
/// `m` 已被自然月占用，故分钟必须写 `min`/`minute`，避免静默改变既有含义。
/// 大小写不敏感、允许前后与数字/单位间空白；`0` 与非法输入返回 Err。
/// 返回绝对 Unix 秒；月运算按 UTC 且月末溢出钳制到目标月最后一天（D2）。
pub fn parse_expiry_spec(spec: &str, now: u64) -> Result<u64, String> {
    const UNITS: &str = "30 / 30d / 1m(自然月) / 1y / 30s / 5min / 2h";
    let s = spec.trim().to_lowercase();
    if s.is_empty() {
        return Err(format!("无效的有效期：内容为空（支持 {UNITS}）"));
    }
    let split_at = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num_part, unit_part) = s.split_at(split_at);
    let unit = unit_part.trim();
    if num_part.is_empty() {
        return Err(format!("无效的有效期: `{spec}`（支持 {UNITS}）"));
    }
    let n: u64 = num_part
        .parse()
        .map_err(|e| format!("无效的有效期数值: {e}"))?;
    if n == 0 {
        return Err("无效的有效期：数量必须大于 0".to_string());
    }
    match unit {
        // 纯数字与 d 同为自然日
        "" | "d" => add_secs(now, n, 86_400, "天数过大"),
        "s" | "sec" | "secs" | "second" | "seconds" => add_secs(now, n, 1, "秒数过大"),
        "min" | "mins" | "minute" | "minutes" => add_secs(now, n, 60, "分钟数过大"),
        "h" | "hr" | "hrs" | "hour" | "hours" => add_secs(now, n, 3_600, "小时数过大"),
        "m" => add_months_utc(now, n),
        "y" => add_months_utc(now, n.checked_mul(12).ok_or("有效期溢出（年数过大）")?),
        other => Err(format!(
            "无效的有效期单位: `{other}`（支持 d = 天 / m = 自然月 / y = 自然年 / \
             s = 秒 / min = 分钟 / h = 小时）"
        )),
    }
}

/// `now + n * unit_secs`，任一乘法或加法溢出即报错（不静默饱和）。
fn add_secs(now: u64, n: u64, unit_secs: u64, overflow_hint: &str) -> Result<u64, String> {
    let secs = n
        .checked_mul(unit_secs)
        .ok_or_else(|| format!("有效期溢出（{overflow_hint}）"))?;
    now.checked_add(secs)
        .ok_or_else(|| "有效期溢出（时间戳超出范围）".to_string())
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

/// 机密展示口径：默认遮挡，`show_secrets` 为真时明文。
/// 密码、用户令牌都走这里，避免截图/录屏时把凭据带出去。
fn secret_display(value: &str, show_secrets: bool) -> String {
    if show_secrets {
        format!("\x1b[1;32m{value}\x1b[0m")
    } else {
        "\x1b[2m已遮挡\x1b[0m（创建时加 --show-secrets 才会显示）".to_string()
    }
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

/// 打印订阅地址、一键导入链接与二维码。
/// 拿不到可用主机时只说明原因，不算失败：用户已开通，退出码仍为 EXIT_OK。
fn print_subscription_block(username: &str, token: &str) {
    let host = match config::load().map_err(|e| e.to_string()).and_then(|cfg| subscription_host(&cfg)) {
        Ok(host) => host,
        Err(reason) => {
            println!("  ⚠ 暂时无法生成订阅地址：{reason}");
            return;
        }
    };
    let (sub_url, deeplink) = subscription_links(&host, token, username);
    println!();
    println!("┌─ Clash 订阅 ────────────────────────────────────────────────");
    println!("│  订阅地址:  \x1b[1;32m{sub_url}\x1b[0m");
    println!("│  一键导入:  \x1b[1;32m{deeplink}\x1b[0m");
    println!("└─────────────────────────────────────────────────────────────");
    match crate::cmd::clash::render_qr(&sub_url) {
        Ok(qr) => {
            println!("  手机扫码导入（对准下方二维码）：");
            for line in qr.lines() {
                println!("  {line}");
            }
        }
        Err(e) => println!("  ⚠ 二维码生成失败：{e}"),
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

/// 有效期剩余时长的人类可读标签：秒级起算，避免短周期显示成「0 天」。
fn duration_label(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{} 天", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{} 小时", secs / 3_600)
    } else if secs >= 60 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
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
    show_secrets: bool,
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
        let (signer, _vk_hex) = if key_path.exists() {
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
        println!("║                       ✓ 用户开通成功                           ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");
        println!("  用户名:     \x1b[1;36m{username}\x1b[0m");
        println!(
            "  身份:       {}",
            if role == "admin" { "管理员（可查看全局大盘）" } else { "普通用户" }
        );
        println!(
            "  每月流量:   \x1b[1;32m{}\x1b[0m",
            crate::render::human_bytes(quota_bytes)
        );
        println!(
            "  有效期至:   {}（{}）",
            fmt_ts(Some(exp)),
            duration_label(exp.saturating_sub(now))
        );
        println!("  同时在线:   最多 {} 台设备", user.max_conns.unwrap_or_default());
        println!("  登录密码:   {}", secret_display(&pass, show_secrets));
        println!("  用户令牌:   {}", secret_display(&token, show_secrets));
        println!();
        println!("  \x1b[1;32m三步上手\x1b[0m");
        println!("    1. 手机安装 Clash（Android）或 Shadowrocket（iOS）");
        println!("    2. 打开 App，扫描下方二维码导入订阅");
        println!("    3. 打开「代理」开关，即可正常上网");
        println!();
        println!("  \x1b[1;33m扫码前请先关闭手机上正在运行的其他 VPN / 代理\x1b[0m，否则订阅下载会失败。");
        print_subscription_block(username, &token);
        println!("  \x1b[1;33m⚠ 下方订阅地址内含上面的用户令牌，等同账号凭据；请勿转发他人，泄露会被人消耗你的每月流量\x1b[0m\n");

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
            println!("║                       ✓ 用户开通成功                           ║");
            println!("╚════════════════════════════════════════════════════════════════╝\n");
            println!("  用户名:     \x1b[1;36m{}\x1b[0m", user.username);
            if let Some(exp) = user.expires_at {
                println!("  有效期至:   {}", fmt_ts(Some(exp as u64)));
            }
            println!("  登录密码:   {}", secret_display(&pass, show_secrets));
            println!();
            if show_secrets {
                println!("┌─ 一键连接地址 ─────────────────────────────────────────────");
                println!("│  \x1b[1;32m{pproxy_uri}\x1b[0m");
                println!("│  \x1b[1;32m{http_uri}\x1b[0m");
                println!("└─────────────────────────────────────────────────────────────");
            }
            println!("  手动填写（手机 Wi-Fi 代理设置）：");
            println!("    服务器: {host_port}");
            println!("    用户名: {}", user.username);
            if show_secrets {
                println!("    密码:   {pass}");
            }
            println!();
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

/// 网关用量账本快照（`POST /api/admin/usage` 响应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLedger {
    pub cycle: String,
    pub next_reset: u64,
    pub days_left: u64,
    /// `(uid, used_bytes)`，按 uid 排序。uid 与令牌 `sub` 同构（`usr_<name>`）。
    pub users: Vec<(String, u64)>,
}

impl UsageLedger {
    /// 某 uid 本周期已用字节（账本无记录 = 0）。
    pub fn used_of(&self, uid: &str) -> u64 {
        self.users
            .iter()
            .find(|(k, _)| k == uid)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }
}

/// 解析网关用量响应。`users` 缺失即报错——结构对不上时宁可失败，
/// 也不让「查询没接通」伪装成「该用户没用量」。
pub fn parse_usage_ledger(json: &str) -> Result<UsageLedger, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("用量响应不是合法 JSON: {e}"))?;
    let users_obj = v
        .get("users")
        .and_then(|u| u.as_object())
        .ok_or_else(|| "用量响应缺少 users 对象".to_string())?;
    let mut users: Vec<(String, u64)> = users_obj
        .iter()
        .map(|(k, val)| (k.clone(), val.as_u64().unwrap_or(0)))
        .collect();
    users.sort();
    Ok(UsageLedger {
        cycle: v
            .get("cycle")
            .and_then(|c| c.as_str())
            .unwrap_or("-")
            .to_string(),
        next_reset: v.get("next_reset").and_then(|t| t.as_u64()).unwrap_or(0),
        days_left: v.get("days_left").and_then(|d| d.as_u64()).unwrap_or(0),
        users,
    })
}

/// 会员状态判定（顺序即优先级）：停用 > 未开会员 > 令牌过期 > 额度耗尽 > 正常。
pub fn membership_state(
    disabled: bool,
    quota: Option<u64>,
    used: u64,
    expires_at: Option<u64>,
    now: u64,
) -> &'static str {
    if disabled {
        "disabled"
    } else if quota.is_none() {
        "未开会员"
    } else if expires_at.is_some_and(|e| e <= now) {
        "令牌过期"
    } else if quota.is_some_and(|q| q > 0 && used >= q) {
        "额度耗尽"
    } else {
        "正常"
    }
}

/// 已用 / 配额的百分比（一位小数）。配额缺失或为 0 时返回 `None`。
pub fn usage_percent(quota: Option<u64>, used: u64) -> Option<String> {
    let q = quota.filter(|q| *q > 0)?;
    let pct10 = used.saturating_mul(1000) / q; // 千分比，避免浮点误差
    Some(format!("{}.{}%", pct10 / 10, pct10 % 10))
}

/// 一行展示单元：本机用户行（可能缺失）与网关账本行的合并视图。
struct UsageView {
    name: String,
    quota: Option<u64>,
    used: u64,
    expires_at: Option<u64>,
    disabled: bool,
    orphan: bool,
}

fn fetch_usage_ledger(username: Option<&str>) -> Result<UsageLedger, String> {
    let body = match username {
        Some(name) => serde_json::json!({ "uid": format!("usr_{name}") }).to_string(),
        None => "{}".to_string(),
    };
    let reply = gate_admin::admin_post("/api/admin/usage", &body)?;
    if reply.status != 200 {
        return Err(format!(
            "网关返回 HTTP {}（{}）",
            reply.status,
            reply.body.trim()
        ));
    }
    parse_usage_ledger(&reply.body)
}

/// `pproxy user usage [username]`：把本机用户表（配额/到期/启用态）
/// 与网关真实用量账本（同一事实源，`subscription-userinfo` 的 download 与之同源）合并展示。
///
/// 只读：不触碰令牌、不写账本。管理通道不可用时失败退出，而非打印 0 误导运维。
pub fn usage(username: Option<&str>) -> Result<i32, String> {
    let service = get_user_service()?;
    let rows = service.list_users();
    let ledger = fetch_usage_ledger(username)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let known: Vec<&UserRow> = match username {
        Some(name) => rows.iter().filter(|u| u.username == name).collect(),
        None => rows.iter().collect(),
    };
    if let Some(name) = username {
        if known.is_empty() {
            println!("⚠ 本机用户表无 '{name}'：仅展示网关账本口径（配额/到期未知）");
        }
    }

    let mut views: Vec<UsageView> = known
        .iter()
        .map(|u| UsageView {
            name: u.username.clone(),
            quota: u.quota_bytes.map(|q| q.max(0) as u64),
            used: ledger.used_of(&format!("usr_{}", u.username)),
            expires_at: u.expires_at.map(|v| v.max(0) as u64),
            disabled: u.disabled,
            orphan: false,
        })
        .collect();

    // 账本里有、用户表里没有的租户（历史令牌/已删但未清零）也要现形，否则流量会「消失」
    let mut covered: std::collections::HashSet<String> = known
        .iter()
        .map(|u| format!("usr_{}", u.username))
        .collect();
    if let Some(name) = username {
        covered.insert(format!("usr_{name}"));
    }
    for (uid, used) in &ledger.users {
        if covered.contains(uid) {
            continue;
        }
        views.push(UsageView {
            name: uid.strip_prefix("usr_").unwrap_or(uid).to_string(),
            quota: None,
            used: *used,
            expires_at: None,
            disabled: false,
            orphan: true,
        });
    }

    println!(
        "账单周期: {}（下次刷新 {}，剩 {} 天） | 通道: {}",
        ledger.cycle,
        fmt_ts(Some(ledger.next_reset)),
        ledger.days_left,
        gate_admin::channel_label()
    );

    if views.is_empty() {
        println!("暂无用户，也无用量记录。");
        return Ok(EXIT_OK);
    }

    let mut t = Table::new(&[
        "username",
        "quota",
        "used",
        "remaining",
        "usage",
        "expires_at",
        "state",
    ]);
    let mut exhausted = 0usize;
    for v in &views {
        let remaining = v.quota.map(|q| q.saturating_sub(v.used));
        let state = membership_state(v.disabled, v.quota, v.used, v.expires_at, now);
        if state == "额度耗尽" {
            exhausted += 1;
        }
        let name = if v.orphan {
            format!("{} (无本机记录)", v.name)
        } else {
            v.name.clone()
        };
        t.push(vec![
            name,
            fmt_quota(v.quota.map(|q| q as i64)),
            human_bytes(v.used),
            remaining.map(human_bytes).unwrap_or_else(|| "-".to_string()),
            usage_percent(v.quota, v.used).unwrap_or_else(|| "-".to_string()),
            fmt_ts(v.expires_at),
            state.to_string(),
        ]);
    }
    print!("{}", t.render());

    if let Some(name) = username {
        if let Some(v) = views.iter().find(|v| v.name == name) {
            if let Some(q) = v.quota {
                println!(
                    "\n{}: 已用 {} / 配额 {}（剩 {}）",
                    v.name,
                    human_bytes(v.used),
                    human_bytes(q),
                    human_bytes(q.saturating_sub(v.used))
                );
                if v.used >= q {
                    println!("  额度已耗尽：网关 CONNECT 返回 403 quota_exceeded，待下个周期刷新");
                }
            }
        }
    } else if exhausted > 0 {
        println!("\n⚠ {exhausted} 个用户额度已耗尽（网关侧 CONNECT/CONNECT-8443 均已熔断）");
    }
    Ok(EXIT_OK)
}

/// 级联清零用量账本（`user rm` 专用）：删除身份后若留着用量，同名重建会
/// 继承历史消耗并一创建即超额（B063）。失败必须可观测——返回 `false`。
fn cascade_reset_usage(username: &str) -> bool {
    let uid = format!("usr_{username}");
    let body = serde_json::json!({ "uid": uid }).to_string();
    match gate_admin::admin_post("/api/admin/reset-usage", &body) {
        Ok(reply) if reply.status == 200 => {
            let before = serde_json::from_str::<serde_json::Value>(&reply.body)
                .ok()
                .and_then(|v| v.get("before").and_then(|b| b.as_u64()))
                .unwrap_or(0);
            println!(
                "  用量清零: \x1b[1;32m✓\x1b[0m {} → 0（渠道: {}）",
                human_bytes(before),
                gate_admin::channel_label()
            );
            true
        }
        Ok(reply) => {
            println!(
                "  \x1b[1;31m✗ 用量清零失败：网关返回 HTTP {}（{}）\x1b[0m",
                reply.status,
                reply.body.trim()
            );
            false
        }
        Err(e) => {
            println!("  \x1b[1;31m✗ 用量清零失败：{e}\x1b[0m");
            false
        }
    }
}

/// 级联撤销的标识符选择（B061）：优先 `jti`（精确，不连坐同名重建）；
/// 缺失 jti 的 legacy 行回落网关 `sub`（`usr_<name>`），返回 `coarse=true` 由调用方告警。
pub fn revocation_identifiers(user: &UserRow) -> (Vec<String>, bool) {
    match user.jti.as_deref().map(str::trim) {
        Some(jti) if !jti.is_empty() => (vec![jti.to_string()], false),
        _ => (vec![format!("usr_{}", user.username)], true),
    }
}

/// 本机撤销表落盘（`~/.pony/revoked_tokens.txt`，本机 gate 启动时加载的兜底真源）。
fn append_local_revocation(identifier: &str) -> Result<PathBuf, String> {
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
    writeln!(file, "{identifier}").map_err(|e| format!("写入撤销列表失败: {e}"))?;
    Ok(path)
}

/// 撤销一组标识符：本机落盘 + 网关热推送，任一环节失败即 `false`。
/// `reason` 只用于回显（「删除用户」/「禁用用户」/「手动撤销」）。
fn enforce_revocation(identifiers: &[String], reason: &str) -> bool {
    let mut all_ok = true;
    for id in identifiers {
        let report = gate_admin::push_revoke(id);
        let mark = if report.ok { "\x1b[1;32m✓\x1b[0m" } else { "\x1b[1;31m✗\x1b[0m" };
        println!("  撤销({reason}): {mark} {id} → {}", report.channel);
        if report.ok {
            if let Err(e) = append_local_revocation(id) {
                all_ok = false;
                println!("  \x1b[1;33m⚠ 远端已生效，但本机撤销表落盘失败：{e}\x1b[0m");
            }
        } else {
            all_ok = false;
            println!("  \x1b[1;33m⚠ 网关未生效：{}\x1b[0m", report.detail);
        }
    }
    all_ok
}

/// 删除/停用后的级联撤销。返回退出码：撤销未生效时非零，便于脚本与 CI 断言。
fn cascade_revoke(user: Option<&UserRow>, username: &str, reason: &str) -> i32 {
    let (identifiers, coarse) = match user {
        Some(u) => revocation_identifiers(u),
        None => (vec![format!("usr_{username}")], true),
    };
    if coarse {
        println!(
            "  \x1b[1;33m⚠ 该用户无令牌 jti 记录，按用户名 {} 撤销；日后同名重建需先从撤销表摘除\x1b[0m",
            identifiers[0]
        );
    }
    if enforce_revocation(&identifiers, reason) {
        println!("  \x1b[1;32m✓ 已签发令牌在网关立即失效（新握手返回 401）\x1b[0m");
        EXIT_OK
    } else {
        println!(
            "  \x1b[1;31m✗ 撤销未完成：令牌可能仍在有效期内可用。修复管理通道后执行 `pproxy user revoke {username}`\x1b[0m"
        );
        EXIT_FAILURE
    }
}

pub fn rm(username: &str) -> Result<i32, String> {
    let service = get_user_service()?;
    // 先取快照：删除后 jti 无从查起，撤销就断了线索（B061）
    let snapshot = service.get_user(username);
    match service.delete_user(username) {
        Ok(true) => {
            println!("✓ 已删除用户: {username}");
            // 两条级联缺一不可：令牌撤销（断网）+ 用量清零（同名重建从 0 起算，B063）
            let revoked = cascade_revoke(snapshot.as_ref(), username, "删除用户");
            let cleared = cascade_reset_usage(username);
            Ok(if revoked == EXIT_OK && cleared {
                EXIT_OK
            } else {
                EXIT_FAILURE
            })
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
    let snapshot = service.get_user(username);
    match service.set_disabled(username, true) {
        Ok(true) => {
            println!("✓ 已禁用用户: {username}");
            Ok(cascade_revoke(snapshot.as_ref(), username, "禁用用户"))
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
            println!(
                "  \x1b[1;33m注意：禁用时撤销的令牌不会自动复通，需重新签发（`user rm` 后重建或 `user renew`）\x1b[0m"
            );
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

    // 本机落盘 + 网关热推送（通道解析集中在 gate_admin：env 或 ~/.pony/gate_admin.conf）
    let path = append_local_revocation(&jti_or_sub)?;
    let report = gate_admin::push_revoke(&jti_or_sub);

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       ✓ 用户令牌已成功废止撤销 (Revocation Enforced)           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  已废止目标: \x1b[1;31m{}\x1b[0m", display_target);
    println!("  撤销标识符: {}", jti_or_sub);
    println!("  落盘路径:   {}", path.display());
    println!(
        "  网关热更新: \x1b[1;32m{}\x1b[0m ({})",
        if report.ok { "已生效 ✓" } else { "未生效 ✗" },
        report.channel
    );
    if !report.ok {
        println!("  失败原因:   \x1b[1;31m{}\x1b[0m", report.detail);
    }
    println!(
        "\n  \x1b[1;32m生效说明：\x1b[0m 该令牌后续发起的 WS 握手、Profile 查询与 CONNECT 将被网关立即返回 401。"
    );
    println!("  \x1b[1;33m边界：\x1b[0m 撤销只拦新握手，已在传输中的长连接不中断（靠连接超时自然收敛）。\n");

    Ok(if report.ok { EXIT_OK } else { EXIT_FAILURE })
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
    fn expiry_subday_units() {
        // D5：分钟级到期验收（`1m` 仍是自然月，分钟必须写 min，防止静默改义）
        let base = utc(2026, 11, 12);
        assert_eq!(parse_expiry_spec("30s", base).unwrap(), base + 30);
        assert_eq!(parse_expiry_spec("1min", base).unwrap(), base + 60);
        assert_eq!(parse_expiry_spec("5 min", base).unwrap(), base + 300);
        assert_eq!(parse_expiry_spec("2h", base).unwrap(), base + 7_200);
        assert_eq!(parse_expiry_spec("1Hr", base).unwrap(), base + 3_600);
        // 分钟不等于自然月：两者相差 2 天以上（30 天/自然月）
        assert!(parse_expiry_spec("1min", base).unwrap() < parse_expiry_spec("1m", base).unwrap());
        // 溢出必须报错而不是饱和
        assert!(parse_expiry_spec(&format!("{}s", u64::MAX), base).is_err());
        assert!(parse_expiry_spec("0min", base).is_err());
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
    fn secret_display_hides_by_default() {
        let hidden = secret_display("s3cret-token", false);
        assert!(hidden.contains("已遮挡"));
        assert!(!hidden.contains("s3cret-token"), "默认不得在屏幕上出现明文凭据");

        let shown = secret_display("s3cret-token", true);
        assert!(shown.contains("s3cret-token"));
        assert!(!shown.contains("已遮挡"));
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

    fn row(username: &str, jti: Option<&str>) -> UserRow {
        UserRow {
            id: 1,
            username: username.to_string(),
            password_hash: String::new(),
            created_at: 0,
            disabled: false,
            expires_at: None,
            last_used_at: None,
            quota_bytes: None,
            jti: jti.map(str::to_string),
            max_conns: None,
            role: None,
        }
    }

    #[test]
    fn revocation_prefers_jti_and_falls_back_to_sub() {
        // 有 jti → 精确撤销，不连坐同名重建（B061 验收口径）
        let (ids, coarse) = revocation_identifiers(&row("alice", Some("tok-0123456789abcdef")));
        assert_eq!(ids, vec!["tok-0123456789abcdef".to_string()]);
        assert!(!coarse);

        // legacy 行（无 jti / 空白 jti）→ 回落 sub，并标记粗粒度
        for jti in [None, Some(""), Some("   ")] {
            let (ids, coarse) = revocation_identifiers(&row("bob", jti));
            assert_eq!(ids, vec!["usr_bob".to_string()]);
            assert!(coarse, "jti={jti:?} 应回落粗粒度");
        }
    }

    #[test]
    fn usage_ledger_parses_gateway_response() {
        let json = r#"{
            "ok": true,
            "cycle": "2026-11",
            "billing_day": 1,
            "next_reset": 1796112000,
            "days_left": 18,
            "users": { "usr_test123": 5259375, "usr_alice": 0 }
        }"#;
        let ledger = parse_usage_ledger(json).unwrap();
        assert_eq!(ledger.cycle, "2026-11");
        assert_eq!(ledger.next_reset, 1796112000);
        assert_eq!(ledger.days_left, 18);
        assert_eq!(
            ledger.users,
            vec![
                ("usr_alice".to_string(), 0),
                ("usr_test123".to_string(), 5259375)
            ]
        );
        assert_eq!(ledger.used_of("usr_test123"), 5259375);
        // 账本无记录 = 0（不是查询失败）
        assert_eq!(ledger.used_of("usr_nobody"), 0);
    }

    #[test]
    fn usage_ledger_rejects_malformed_response() {
        // 结构对不上必须报错：否则「没接通」会伪装成「没用量」
        assert!(parse_usage_ledger("not json").is_err());
        assert!(parse_usage_ledger(r#"{"cycle":"2026-11"}"#).is_err());
    }

    #[test]
    fn membership_state_priority_is_stable() {
        let now = 1_000_000;
        // 停用优先于一切
        assert_eq!(membership_state(true, Some(10), 100, Some(now + 1), now), "disabled");
        // 无配额 = 未开会员
        assert_eq!(membership_state(false, None, 100, None, now), "未开会员");
        // 令牌先于额度到期
        assert_eq!(membership_state(false, Some(100), 100, Some(now), now), "令牌过期");
        // 用量达到配额即耗尽（含恰好相等）
        assert_eq!(membership_state(false, Some(100), 100, Some(now + 60), now), "额度耗尽");
        assert_eq!(membership_state(false, Some(100), 99, Some(now + 60), now), "正常");
    }

    #[test]
    fn duration_label_covers_subday_and_long_lived() {
        assert_eq!(duration_label(59), "59 秒");
        assert_eq!(duration_label(60), "1 分钟");
        assert_eq!(duration_label(3_600), "1 小时");
        assert_eq!(duration_label(86_400), "1 天");
        assert_eq!(duration_label(30 * 86_400), "30 天");
    }

    #[test]
    fn quota_line_uses_human_bytes_not_always_gigabytes() {
        // 回归：5M 额度曾在面向用户输出里被格式化成「0.00 GB」，用户以为是零额度
        assert_eq!(crate::render::human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(crate::render::human_bytes(50 * 1024 * 1024), "50.0 MB");
        assert_eq!(crate::render::human_bytes(50 * 1024 * 1024 * 1024), "50.0 GB");
    }

    #[test]
    fn usage_percent_keeps_one_decimal() {
        assert_eq!(usage_percent(Some(5_242_880), 5_259_375).as_deref(), Some("100.3%"));
        assert_eq!(usage_percent(Some(1000), 0).as_deref(), Some("0.0%"));
        assert_eq!(usage_percent(Some(1000), 1).as_deref(), Some("0.1%"));
        assert_eq!(usage_percent(Some(0), 5), None);
        assert_eq!(usage_percent(None, 5), None);
    }
}
