//! `pproxy clash` / `pproxy config export clash`
//! 一键生成 Clash Meta 完整配置、保存本地、生成订阅 URL 并输出终端二维码。

use std::path::PathBuf;

use crate::client::AdminClient;
use crate::cmd::serve::get_local_lan_ip;
use crate::cmd::service::tokio_block;
use crate::config::{self, PonyConfig};
use crate::EXIT_OK;

/// 生成标准 Clash Meta / Mihomo 配置文件 YAML
///
/// B015：`gate_host` 为 Some 时生成 **隧道直连模式**（走 gate 的 wss:// 端点，
/// server/HOST/SNI 自动对准 gate 域名——域名轮换后重新生成订阅即生效，无需手改）；
/// None 时保持局域网 http 模式（现状行为）。
pub fn generate_clash_yaml(server_ip: &str, port: u16, token: Option<&str>) -> String {
    generate_clash_yaml_mode(server_ip, port, token, None)
}

/// B015：隧道模式（gate 域名直连）。gate_host 形如 "gate.example.com"（不带协议/路径）。
/// 生成的 proxy 走 ws+tls：server=gate 域名、sni=同域名、ws-opts path=/ws、
/// 鉴权经 ws-opts headers Authorization: Bearer <token>（对齐 gate worker /ws 协议）。
pub fn generate_clash_yaml_tunnel(gate_host: &str, token: Option<&str>) -> String {
    generate_clash_yaml_mode(gate_host, 443, token, Some(gate_host))
}

/// 统一实现：tunnel_host 为 Some 时输出 ws+tls 隧道 proxy；否则 http 局域网 proxy。
fn generate_clash_yaml_mode(
    server: &str,
    port: u16,
    token: Option<&str>,
    tunnel_host: Option<&str>,
) -> String {
    let auth_section = match token {
        Some(t) if !t.is_empty() => format!("    username: \"{t}\"\n    password: \"{t}\"\n"),
        _ => String::new(),
    };

    let (proxy_block, headers_block) = match tunnel_host {
        Some(host) => {
            // 隧道直连：ws+tls，SNI/HOST 对准 gate 域名，token 走 Bearer
            let proxy = format!(
                "  - name: \"Pony-Tunnel\"\n    type: ws\n    server: {host}\n    port: 443\n    tls: true\n    sni: {host}\n    skip-cert-verify: false\n    ws-opts:\n      path: /ws\n      headers:\n        Authorization: \"Bearer {}\"\n",
                token.unwrap_or("")
            );
            (proxy, String::new())
        }
        None => {
            let proxy = format!(
                "  - name: \"Pony-Proxy\"\n    type: http\n    server: {server}\n    port: {port}\n{auth_section}"
            );
            (proxy, String::new())
        }
    };
    let _ = headers_block;

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
{proxy_block}proxy-groups:
  - name: "PROXY"
    type: select
    url: "http://cp.cloudflare.com/generate_204"
    interval: 300
    proxies:
      - "Pony-Tunnel"
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

/// 从配置提取 gate 隧道域名（B015）：
/// 支持 "wss://host[:port]/ws"、"wss://host[:port]"、"host[:port]" 三种形态，
/// 返回纯 host（去掉端口/协议/路径）；多端点取第一个；无配置返回 None。
fn gate_host_from_config(cfg: &PonyConfig) -> Option<String> {
    let url = cfg.tunnel_gate_url.as_deref()?.trim();
    let first = url.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }
    // 去协议前缀
    let without_scheme = first
        .trim_start_matches("wss://")
        .trim_start_matches("ws://")
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // 去路径（/ws 等）
    let hostport = without_scheme.split('/').next().unwrap_or("").trim();
    // 去端口：IPv6 "[::1]:443" → "::1"（保留括号内）；普通 "host:port" → "host"
    let host = if hostport.starts_with('[') {
        hostport
            .split_once(']')
            .map(|(h, _)| h.trim_start_matches('[').to_string())
            .unwrap_or_else(|| hostport.to_string())
    } else {
        hostport.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
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

    // B015：若配置了 gate 隧道域名，订阅自动对准 gate（域名轮换后重新生成即生效）。
    // 提取 wss://host[:port]/ws 或 host:port 中的 host；无配置回落局域网模式。
    let gate_host = gate_host_from_config(cfg);

    // 自动确定局域网 IP
    let lan_ip = match lan_ip_arg {
        Some(ip) => ip.to_string(),
        None => get_local_lan_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
    };

    let token = resolve_or_create_token(cfg, token_arg);
    let token_str = token.as_deref().unwrap_or("<未配置-请运行 pproxy token create 生成>");

    // 1. 生成 YAML 内容（gate 隧道模式或局域网模式）
    let yaml = match gate_host.as_deref() {
        Some(host) => generate_clash_yaml_tunnel(host, token.as_deref()),
        None => generate_clash_yaml(&lan_ip, port, token.as_deref()),
    };

    // 2. 保存到 ~/.pony/clash.yaml
    if let Ok(path) = default_clash_file_path() {
        let _ = config::secure_write_file(&path, yaml.as_bytes());
    }

    let subscription_url = match gate_host.as_deref() {
        Some(host) => format!("wss://{host}/ws"),
        None => format!("http://{lan_ip}:{port}/clash.yaml"),
    };

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
    match gate_host.as_deref() {
        Some(host) => {
            println!("\x1b[1;36m💡 隧道模式：直连 gate 域名 {host}（HOST/SNI 已自动对准）\x1b[0m");
            println!("\x1b[1;36m   gate 域名轮换后重新执行本命令生成新订阅，无需手改配置\x1b[0m");
            println!("\x1b[1;36m   需服务端已配置 PPROXY_TUNNEL_GATE_URL（gate 隧道端点）\x1b[0m\n");
        }
        None => {
            println!("\x1b[1;36m💡 提示: 请确保电脑端已运行网关服务: pproxy serve --lan\x1b[0m");
            println!("\x1b[1;36m       且手机与电脑连接在同一个 WiFi 局域网内。\x1b[0m\n");
        }
    }

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

    #[test]
    fn test_generate_clash_yaml_tunnel_aligns_gate_host() {
        // B015：隧道模式 server/SNI/HOST 自动对准 gate 域名，Bearer 鉴权走 ws-opts headers
        let yaml = generate_clash_yaml_tunnel("gate.ponygo.fun", Some("tok_123"));
        assert!(yaml.contains("type: ws"), "隧道模式是 ws 代理");
        assert!(yaml.contains("server: gate.ponygo.fun"));
        assert!(yaml.contains("port: 443"));
        assert!(yaml.contains("sni: gate.ponygo.fun"), "SNI 对准 gate 域名");
        assert!(yaml.contains("path: /ws"), "对齐 gate /ws 路径");
        assert!(yaml.contains("Authorization: \"Bearer tok_123\""), "token 走 Bearer");
        assert!(!yaml.contains("username:"), "隧道模式不用 http 基本认证");
    }

    #[test]
    fn test_gate_host_from_config_parses_endpoint_shapes() {
        // B015：域名轮换后重新生成订阅即生效，无需手改 HOST——解析三种端点形态
        let mut cfg = PonyConfig::default();
        cfg.tunnel_gate_url = Some("wss://gate.ponygo.fun/ws".into());
        assert_eq!(gate_host_from_config(&cfg).as_deref(), Some("gate.ponygo.fun"));

        cfg.tunnel_gate_url = Some("wss://gate2.ponygo.fun".into());
        assert_eq!(gate_host_from_config(&cfg).as_deref(), Some("gate2.ponygo.fun"));

        cfg.tunnel_gate_url = Some("gate3.ponygo.fun:443".into());
        assert_eq!(gate_host_from_config(&cfg).as_deref(), Some("gate3.ponygo.fun"));

        // 多端点取第一个（桌面端 vgate 在前/逗号分隔）
        cfg.tunnel_gate_url = Some("wss://gate4.ponygo.fun/ws,wss://backup.ponygo.fun/ws".into());
        assert_eq!(gate_host_from_config(&cfg).as_deref(), Some("gate4.ponygo.fun"));

        cfg.tunnel_gate_url = None;
        assert_eq!(gate_host_from_config(&cfg), None, "无配置回落局域网模式");

        cfg.tunnel_gate_url = Some("  ".into());
        assert_eq!(gate_host_from_config(&cfg), None, "空白配置视为未配置");
    }
}
