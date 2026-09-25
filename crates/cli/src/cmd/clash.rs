//! `pproxy clash` / `pproxy config export clash`
//! 一键生成 Clash Meta 完整配置、保存本地、生成订阅 URL 并输出终端二维码。

use std::path::PathBuf;

use crate::client::AdminClient;
use crate::cmd::serve::get_local_lan_ip;
use crate::cmd::service::tokio_block;
use crate::config::{self, PonyConfig};
use crate::EXIT_OK;

/// 生成标准 Clash Meta / Mihomo 配置文件 YAML
pub fn generate_clash_yaml(server_ip: &str, port: u16, token: Option<&str>) -> String {
    let auth_section = match token {
        Some(t) if !t.is_empty() => format!("    username: \"{t}\"\n    password: \"{t}\"\n"),
        _ => String::new(),
    };

    format!(
        r#"# ================================================================
#  Pony Proxy — Clash Meta / Mihomo 移动端与客户端智能代理配置
# ================================================================
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
ipv6: false

proxies:
  - name: "Pony-Proxy"
    type: http
    server: {server_ip}
    port: {port}
{auth_section}
proxy-groups:
  - name: "PROXY"
    type: select
    url: "http://cp.cloudflare.com/generate_204"
    interval: 300
    proxies:
      - "Pony-Proxy"
      - DIRECT

rules:
  # 1. 服务端公网与局域网直连保护（审查修复 P0-2：置顶优先，加 no-resolve 避免反向解析延迟）
  - DOMAIN-SUFFIX,ponygo.fun,DIRECT
  - IP-CIDR,127.0.0.0/8,DIRECT,no-resolve
  - IP-CIDR,172.16.0.0/12,DIRECT,no-resolve
  - IP-CIDR,192.168.0.0/16,DIRECT,no-resolve
  - IP-CIDR,10.0.0.0/8,DIRECT,no-resolve

  # 2. 系统与客户端探活直连保护（审查修复 P0-1：置顶于 PROXY 规则前，彻底防止探活偷跑代理流量）
  - DOMAIN,connectivitycheck.gstatic.com,DIRECT
  - DOMAIN,connectivitycheck.android.com,DIRECT
  - DOMAIN,clients3.google.com,DIRECT
  - DOMAIN,msftconnecttest.com,DIRECT
  - DOMAIN,captive.apple.com,DIRECT
  - DOMAIN,cp.cloudflare.com,DIRECT

  # 3. 核心海外大模型与 AI 平台
  - DOMAIN-SUFFIX,openai.com,PROXY
  - DOMAIN-SUFFIX,chatgpt.com,PROXY
  - DOMAIN-SUFFIX,oaistatic.com,PROXY
  - DOMAIN-SUFFIX,oaiusercontent.com,PROXY
  - DOMAIN-SUFFIX,anthropic.com,PROXY
  - DOMAIN-SUFFIX,claude.ai,PROXY
  - DOMAIN-SUFFIX,claudeusercontent.com,PROXY
  - DOMAIN-SUFFIX,deepmind.google,PROXY
  - DOMAIN-SUFFIX,perplexity.ai,PROXY
  - DOMAIN-SUFFIX,huggingface.co,PROXY
  # Google 与 Android / 开发者服务
  - DOMAIN-SUFFIX,google.com,PROXY
  - DOMAIN-SUFFIX,googleapis.com,PROXY
  - DOMAIN-SUFFIX,gstatic.com,PROXY
  - DOMAIN-SUFFIX,googleusercontent.com,PROXY
  - DOMAIN-SUFFIX,android.com,PROXY
  - DOMAIN-SUFFIX,golang.org,PROXY
  # 影音流媒体 (YouTube)
  - DOMAIN-SUFFIX,youtube.com,PROXY
  - DOMAIN-SUFFIX,googlevideo.com,PROXY
  - DOMAIN-SUFFIX,ytimg.com,PROXY
  - DOMAIN-SUFFIX,youtu.be,PROXY
  # 社交与通讯 (X / Twitter / Telegram)
  - DOMAIN-SUFFIX,x.com,PROXY
  - DOMAIN-SUFFIX,twitter.com,PROXY
  - DOMAIN-SUFFIX,twimg.com,PROXY
  - DOMAIN-SUFFIX,t.co,PROXY
  - DOMAIN-SUFFIX,telegram.org,PROXY
  - DOMAIN-SUFFIX,t.me,PROXY
  - DOMAIN-SUFFIX,telegram.me,PROXY
  - DOMAIN-SUFFIX,telegra.ph,PROXY
  # 开发者平台与通用知识库
  - DOMAIN-SUFFIX,github.com,PROXY
  - DOMAIN-SUFFIX,githubusercontent.com,PROXY
  - DOMAIN-SUFFIX,github.io,PROXY
  - DOMAIN-SUFFIX,githubassets.com,PROXY
  - DOMAIN-SUFFIX,gitlab.com,PROXY
  - DOMAIN-SUFFIX,docker.com,PROXY
  - DOMAIN-SUFFIX,docker.io,PROXY
  - DOMAIN-SUFFIX,stackoverflow.com,PROXY
  - DOMAIN-SUFFIX,wikipedia.org,PROXY
  - DOMAIN-SUFFIX,wikimedia.org,PROXY
  # 国内直连与 GEOIP 兜底
  - DOMAIN-SUFFIX,cn,DIRECT
  - GEOIP,CN,DIRECT
  # 兜底规则：未匹配项默认直连，国内网络裸奔顺畅
  - MATCH,DIRECT
"#
    )
}

/// 将文本内容渲染为终端 Unicode 字符二维码
pub fn render_qr(content: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(content.as_bytes()).map_err(|e| format!("生成二维码失败: {e}"))?;
    let qr = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Dark)
        .light_color(qrcode::render::unicode::Dense1x2::Light)
        .build();
    Ok(qr)
}

/// 获取 clash.yaml 存储路径（~/.pony/clash.yaml）
pub fn default_clash_file_path() -> Result<PathBuf, String> {
    let home = config::home_dir().map_err(|e| e.to_string())?;
    Ok(home.join(".pony").join("clash.yaml"))
}

/// 尝试从现有的 clash.yaml 中提取已有的 token
fn extract_token_from_existing_file() -> Option<String> {
    let path = default_clash_file_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("password:") {
            let pass = rest.trim().trim_matches('"').trim_matches('\'');
            if !pass.is_empty() {
                return Some(pass.to_string());
            }
        }
    }
    None
}

/// 生成或获取一个可用的 Token
fn resolve_or_create_token(cfg: &PonyConfig, token_arg: Option<&str>) -> Option<String> {
    if let Some(t) = token_arg {
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    // 优先复用已有 clash.yaml 中写入的 token
    if let Some(existing) = extract_token_from_existing_file() {
        return Some(existing);
    }

    // 尝试通过管理面 API 自动生成一个手机专用 token
    if let Ok(http) = AdminClient::new(&cfg.server, &cfg.admin_token) {
        let created = tokio_block(async {
            http.create_token("phone-clash", None).await.ok()
        });
        if let Some(tok_row) = created {
            return Some(tok_row.token);
        }
    }

    None
}

pub fn run(
    cfg: &PonyConfig,
    token_arg: Option<&str>,
    lan_ip_arg: Option<&str>,
    port_arg: Option<u16>,
    url_only: bool,
) -> Result<i32, String> {
    let port = port_arg.unwrap_or(8899);

    // 自动确定局域网 IP
    let lan_ip = match lan_ip_arg {
        Some(ip) => ip.to_string(),
        None => get_local_lan_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
    };

    let token = resolve_or_create_token(cfg, token_arg);
    let token_str = token.as_deref().unwrap_or("<未配置-请运行 pproxy token create 生成>");

    // 1. 生成 YAML 内容
    let yaml = generate_clash_yaml(&lan_ip, port, token.as_deref());

    // 2. 保存到 ~/.pony/clash.yaml
    if let Ok(path) = default_clash_file_path() {
        let _ = config::secure_write_file(&path, yaml.as_bytes());
    }

    let subscription_url = format!("http://{lan_ip}:{port}/clash.yaml");

    if url_only {
        println!("{subscription_url}");
        return Ok(EXIT_OK);
    }

    // 3. 输出终端二维码与使用指南
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║            Pony Proxy 手机 Clash Meta 配置一键导入              ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    println!("\x1b[1;33m【方式一：手机扫码导入（最便捷）】\x1b[0m");
    println!("打开手机 Clash Meta（或 Flclash / 小火箭）-> 配置 -> 新建配置 -> \x1b[1;36m扫描二维码\x1b[0m：\n");

    match render_qr(&subscription_url) {
        Ok(qr) => {
            println!("{qr}\n");
        }
        Err(e) => {
            eprintln!("(二维码生成失败: {e})\n");
        }
    }

    println!("\x1b[1;33m【方式二：从 URL 导入】\x1b[0m");
    println!("在手机 Clash 中选择「从 URL 导入」，填入以下订阅链接：");
    println!("  \x1b[1;32m{subscription_url}\x1b[0m\n");

    if let Ok(file_path) = default_clash_file_path() {
        println!("\x1b[1;33m【方式三：从本地文件导入】\x1b[0m");
        println!("配置文件已生成至本机：");
        println!("  \x1b[1;34m{}\x1b[0m\n", file_path.display());
    }

    println!("\x1b[1;33m【方式四：手机手动配置 HTTP 代理】\x1b[0m");
    println!("若不使用 Clash，可直接在手机 WiFi 设置中配置手动代理：");
    println!("  服务器主机: \x1b[1;32m{lan_ip}\x1b[0m");
    println!("  端口:       \x1b[1;32m{port}\x1b[0m");
    println!("  代理密码:   \x1b[1;32m{token_str}\x1b[0m\n");

    println!("────────────────────────────────────────────────────────────────");
    println!("\x1b[1;36m💡 提示: 请确保电脑端已运行网关服务: pproxy serve --lan\x1b[0m");
    println!("\x1b[1;36m       且手机与电脑连接在同一个 WiFi 局域网内。\x1b[0m\n");

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_clash_yaml_contains_expected_fields() {
        let yaml = generate_clash_yaml("192.168.1.100", 8899, Some("tok_123"));
        assert!(yaml.contains("server: 192.168.1.100"));
        assert!(yaml.contains("port: 8899"));
        assert!(yaml.contains("username: \"tok_123\""));
        assert!(yaml.contains("password: \"tok_123\""));
        assert!(yaml.contains("DOMAIN-SUFFIX,openai.com,PROXY"));
        assert!(yaml.contains("GEOIP,CN,DIRECT"));
    }

    #[test]
    fn test_render_qr_valid() {
        let res = render_qr("http://127.0.0.1:8899/clash.yaml");
        assert!(res.is_ok());
        let s = res.unwrap();
        assert!(!s.is_empty());
    }
}
