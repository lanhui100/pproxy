//! `pproxy init --interactive`（或缺少参数时自动进入交互模式）：引导用户
//! 获取并填入 CF Token、Vercel Token 等凭据，生成完整配置。
//!
//! 设计原则：用户只需按指引从平台复制 token，其余全自动化。

use std::io::{self, BufRead, Write};

use crate::config::{self, PonyConfig};

/// 交互式初始化引导。
pub fn run_interactive(force: bool) -> Result<i32, String> {
    let path = config::config_path().map_err(|e| e.to_string())?;
    if path.exists() && !force {
        eprintln!("config already exists: {} (use --force to overwrite)", path.display());
        eprintln!("或者运行 'pproxy init --interactive --force' 重新初始化");
        return Ok(1);
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║       Pony Proxy — 初始化向导                   ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("本向导将引导你完成以下配置：");
    println!("  1. 管理面服务器地址 + Admin Token");
    println!("  2. Cloudflare API Token（用于部署 CF Worker）");
    println!("  3. Vercel Token（用于部署 Vercel 函数）");
    println!("  4. 隧道令牌（用于 Gate Worker 认证）");
    println!();
    println!("按 Ctrl+C 随时退出，已输入内容不会保存。\n");

    // 1. Server URL
    let server = prompt(
        "管理面服务器地址",
        "例如: http://192.168.1.100:8900 或 https://api.ponyjob.top:8900",
        "http://127.0.0.1:8900",
    );

    // 2. Admin Token
    println!();
    println!("┌─ Admin Token ──────────────────────────────────");
    println!("│ Admin Token 是管理 API 的访问凭据。");
    println!("│ 首次启动 pproxy-server 时会在日志中打印一次：");
    println!("│   journalctl -u pproxy | grep 'admin token'");
    println!("│ 也可以通过环境变量 PPROXY_ADMIN_TOKEN 预先注入。");
    println!("└─────────────────────────────────────────────────");
    let admin_token = prompt_secret("Admin Token", "");

    // 3. CF API Token
    println!();
    println!("┌─ Cloudflare API Token ─────────────────────────");
    println!("│ 用于自动部署 CF Worker（edge.ponyjob.top）。");
    println!("│ 获取步骤：");
    println!("│   1. 打开 https://dash.cloudflare.com/profile/api-tokens");
    println!("│   2. 点击「Create Token」→「Create Custom Token」");
    println!("│   3. 权限：");
    printf("│      - Workers: Edit");
    println!("│      - Account: Account Settings (Read)");
    println!("│   4. Account Resources: 选择你的账号");
    println!("│   5. 创建后复制 Token（以 cfat_ 开头）");
    println!("│ 也可以使用 CF_API_TOKEN 环境变量（优先级更高）。");
    println!("└─────────────────────────────────────────────────");
    let cf_token = prompt_secret("CF API Token (留空跳过)", "");
    let cf_account_tag = if !cf_token.is_empty() {
        println!();
        println!("┌─ CF Account Tag ────────────────────────────────");
        println!("│ 可在 https://dash.cloudflare.com 右侧「Account ID」找到。");
        println!("└─────────────────────────────────────────────────");
        prompt("CF Account Tag (留空跳过)", "例如: 6f0f4f7f6dfe8ee82061083589fcb212", "")
    } else {
        String::new()
    };

    // 4. Vercel Token
    println!();
    println!("┌─ Vercel Token ──────────────────────────────────");
    println!("│ 用于自动部署 Vercel 函数（vedge.ponyjob.top）。");
    println!("│ 获取步骤：");
    println!("│   1. 打开 https://vercel.com/account/tokens");
    println!("│   2. 点击「Create Token」");
    println!("│   3. 名称随意，Scope 选你的项目或全账号");
    println!("│   4. 创建后复制 Token（以 vcp_ 开头）");
    println!("│ 也可以使用 VERCEL_TOKEN 环境变量（优先级更高）。");
    println!("└─────────────────────────────────────────────────");
    let vercel_token = prompt_secret("Vercel Token (留空跳过)", "");

    // 5. Tunnel Token
    println!();
    println!("┌─ 隧道令牌 (Tunnel Token) ──────────────────────");
    println!("│ 用于 Gate Worker 认证（桌面端隧道功能）。");
    println!("│ 如不部署 Gate Worker 可跳过。");
    println!("│ 隧道令牌需要自行生成一个安全的随机字符串：");
    println!("│   openssl rand -hex 32");
    println!("│ 或使用密码管理器生成的随机密码。");
    println!("└─────────────────────────────────────────────────");
    println!("│ 注意：此令牌的 SHA-256 哈希值需要设置为 wrangler secret：");
    println!("│   echo -n '<your-token>' | sha256sum");
    println!("│   npx wrangler secret put TUNNEL_TOKEN_HASH");
    println!("└─────────────────────────────────────────────────");
    let tunnel_token = prompt_secret("Tunnel Token (留空跳过)", "");

    // 6. PROXY_SECRET
    println!();
    println!("┌─ PROXY_SECRET（上游共享密钥）────────────────────");
    println!("│ 服务器与 CF Worker / Vercel 之间的共享密钥。");
    println!("│ 可以用随机字符串，但所有组件必须一致。");
    println!("│ 留空将自动生成一个 64 字符的随机 hex 串。");
    println!("└─────────────────────────────────────────────────");
    let proxy_secret_default = generate_random_hex(64);
    let proxy_secret = prompt_secret("PROXY_SECRET (留空自动生成)", &proxy_secret_default);
    let proxy_secret = if proxy_secret.is_empty() {
        proxy_secret_default
    } else {
        proxy_secret
    };

    // 7. 保存配置
    let cfg = PonyConfig {
        server: server.clone(),
        admin_token: admin_token.clone(),
        data_plane: None,
        cf_token: if cf_token.is_empty() { None } else { Some(cf_token.clone()) },
        cf_account_tag: if cf_account_tag.is_empty() { None } else { Some(cf_account_tag.clone()) },
        vercel_token: if vercel_token.is_empty() { None } else { Some(vercel_token.clone()) },
        tunnel_token: if tunnel_token.is_empty() { None } else { Some(tunnel_token.clone()) },
        proxy_secret: Some(proxy_secret.clone()),
    };

    let written = config::save(&cfg).map_err(|e| e.to_string())?;
    println!();
    println!("✓ 配置已写入: {}", written.display());

    // 8. 输出 env 文件参考
    println!();
    println!("┌─ 环境变量文件参考 (.pproxy.env) ────────────────");
    println!("│ 如需通过 systemd EnvironmentFile 注入，可创建以下文件：");
    println!("│");
    let mut env_out = String::new();
    if !cf_token.is_empty() {
        env_out.push_str(&format!("PPROXY_CF_API_TOKEN={}\n", cf_token));
    }
    if !cf_account_tag.is_empty() {
        env_out.push_str(&format!("PPROXY_CF_ACCOUNT_TAG={}\n", cf_account_tag));
    }
    if !vercel_token.is_empty() {
        env_out.push_str(&format!("PPROXY_VERCEL_TOKEN={}\n", vercel_token));
    }
    if !tunnel_token.is_empty() {
        env_out.push_str("PPROXY_TUNNEL_GATE_URL=wss://gate.ponyjob.top/ws\n");
        env_out.push_str(&format!("PPROXY_TUNNEL_TOKEN={}\n", tunnel_token));
    }
    for line in env_out.lines() {
        println!("│   {}", line);
    }
    println!("│");
    println!("│ 保存到 /home/USER/pproxy/.pproxy.env 并 chmod 600");
    println!("└─────────────────────────────────────────────────");

    // 9. 下一步指引
    println!();
    println!("┌─ 下一步操作指引 ────────────────────────────────");
    println!("│");
    let has_deploy_tokens = !cf_token.is_empty() || !vercel_token.is_empty();
    if has_deploy_tokens {
        println!("│ 运行以下命令部署上游服务：");
        if !cf_token.is_empty() {
            println!("│   pproxy deploy cf-worker    # 部署 CF Worker");
        }
        if !vercel_token.is_empty() {
            println!("│   pproxy deploy vercel       # 部署 Vercel 函数");
        }
        if !tunnel_token.is_empty() {
            println!("│   pproxy deploy gate         # 部署 Gate Worker");
        }
        println!("│   pproxy deploy all          # 部署全部");
        println!("│");
    }
    println!("│ 管理服务状态：");
    println!("│   pproxy status               # 查看服务状态");
    println!("│   pproxy doctor               # 全路由体检");
    println!("│");
    println!("│ 路由管理：");
    println!("│   pproxy route list           # 查看路由列表");
    println!("│   pproxy route add <name> <host>  # 添加路由");
    println!("│");
    println!("│ Token 管理：");
    println!("│   pproxy token create <name>  # 创建数据面 token");
    println!("│   pproxy token list           # 列出 token");
    println!("└─────────────────────────────────────────────────");
    println!();

    Ok(0)
}

/// 提示用户输入文本。
fn prompt(label: &str, hint: &str, default: &str) -> String {
    let mut input = String::new();
    print!("\n  {} \n  └ 提示: {} \n  └ 默认: {} \n  > ", label, hint, default);
    io::stdout().flush().ok();
    io::stdin().lock().read_line(&mut input).ok();
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() { default.to_string() } else { trimmed }
}

/// 提示用户输入密码类内容（不回显，但 Rust 标准库不支持静默输入，
/// 这里用明文输入 + 提示注意安全）。
fn prompt_secret(label: &str, default: &str) -> String {
    let mut input = String::new();
    print!("\n  {} \n  > ", label);
    io::stdout().flush().ok();
    io::stdin().lock().read_line(&mut input).ok();
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() { default.to_string() } else { trimmed }
}

fn printf(s: &str) {
    print!("{}", s);
    io::stdout().flush().ok();
}

fn generate_random_hex(len: usize) -> String {
    use rand::Rng;
    let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
    hex::encode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_random_hex_length_and_hex_chars() {
        for len in [8, 16, 32, 64] {
            let s = generate_random_hex(len);
            assert_eq!(s.len(), len * 2, "hex encoding doubles length");
            assert!(s.chars().all(|c| c.is_ascii_hexdigit()), "all hex chars");
        }
    }

    #[test]
    fn generate_random_hex_not_constant() {
        let a = generate_random_hex(16);
        let b = generate_random_hex(16);
        // 极低概率碰撞，两次相同概率约 2^-128 ≈ 不可达
        assert_ne!(a, b, "random hex should differ between calls");
    }
}