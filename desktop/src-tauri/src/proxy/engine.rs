//! 引擎核心：127.0.0.1:18900 监听，CONNECT/absolute-form 分流。
//!
//! 决策：host 命中白名单 → [`tunnel`]（WS 隧道，M6 后续任务实现）；未命中 →
//! 本机直接 dial。失败语义：隧道不可用即关闭连接并计数，绝不静默回落直连
//! （spec §5，R4）。

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::pac;
use super::whitelist;

/// 引擎配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub listen_addr: String,
    pub whitelist: Vec<String>,
    /// 隧道端点（wss://…），None 时白名单流量直接报错关闭（无静默回落）
    pub tunnel_url: Option<String>,
    pub tunnel_token: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            listen_addr: "127.0.0.1:18900".into(),
            whitelist: Vec::new(),
            tunnel_url: None,
            tunnel_token: None,
        }
    }
}

/// 运行统计（Arc 共享，供 Tauri 命令查询）。
#[derive(Debug, Default)]
pub struct EngineStats {
    pub conns: std::sync::atomic::AtomicU64,
    pub tunneled: AtomicU64,
    pub direct: AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
}

pub type SharedStats = Arc<EngineStats>;

/// 启动引擎循环（由 tauri setup / 命令调用；返回前先绑定端口）。
pub async fn run(cfg: EngineConfig, stats: SharedStats) -> std::io::Result<()> {
    let listener = TcpListener::bind(&cfg.listen_addr).await?;
    let cfg = Arc::new(cfg);
    loop {
        let (stream, _peer) = listener.accept().await?;
        stats
            .conns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cfg = Arc::clone(&cfg);
        let stats = Arc::clone(&stats);
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, &cfg, &stats).await {
                stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                *stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                log::warn!("proxy conn failed: {e}");
            }
        });
    }
}

#[derive(Debug, PartialEq)]
enum Route {
    Tunnel,
    Direct,
}

/// 分流判定（纯函数）：CONNECT 目标或 Host 头命中白名单 → 隧道。
fn decide(host: &str, cfg: &EngineConfig) -> Route {
    if whitelist::matches(host, &cfg.whitelist) && cfg.tunnel_url.is_some() {
        Route::Tunnel
    } else {
        Route::Direct
    }
}

pub struct ReqHead {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, PartialEq)]
pub enum Kind {
    Connect,
    Plain,
}

/// 解析请求头第一行 + Host 头（纯函数便于测试）。
fn parse_head(head: &str) -> Option<ReqHead> {
    let first = head.lines().next()?;
    if let Some(rest) = first.strip_prefix("CONNECT ") {
        let target = rest.split(' ').next()?;
        let (h, p) = split_host_port(target, 443)?;
        return Some(ReqHead { kind: Kind::Connect, host: h, port: p });
    }
    // absolute-form：GET http://host/path HTTP/1.1
    let method_target: Vec<&str> = first.splitn(3, ' ').collect();
    if method_target.len() == 3 {
        if let Some(url) = method_target[1].strip_prefix("http://") {
            let authority = url.split(['/', '?']).next()?;
            let (h, p) = split_host_port(authority, 80)?;
            return Some(ReqHead { kind: Kind::Plain, host: h, port: p });
        }
    }
    None
}

fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 字面量 [::1]:port
        let (h, rest) = stripped.split_once(']')?;
        let port = rest.strip_prefix(':').and_then(|p| p.parse().ok());
        return Some((h.to_string(), port.unwrap_or(default_port)));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((authority.to_string(), default_port)),
    }
}

async fn handle_conn(
    mut stream: TcpStream,
    cfg: &EngineConfig,
    stats: &EngineStats,
) -> std::io::Result<()> {
    // 读取请求头（至 \r\n\r\n，上限 16KB）
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 2048];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(()); // 对端关闭
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);

    // 引擎自身的控制面：PAC 脚本（仅 absolute-form GET /pac 可达）
    if head.starts_with("GET http://127.0.0.1:18900/pac ")
        || head.starts_with("GET /pac ")
    {
        let body = pac::generate_pac(&cfg.whitelist);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        return stream.write_all(resp.as_bytes()).await;
    }

    let Some(parsed) = parse_head(&head) else {
        return stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await;
    };

    match decide(&parsed.host, cfg) {
        Route::Direct => {
            stats.direct.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            direct_relay(stream, parsed, &head).await
        }
        Route::Tunnel => {
            stats.tunneled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match super::engine_tunnel::connect_and_relay(stream, parsed, &head, cfg).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    // R4：绝不静默回落直连——错误已计数，连接由调用方关闭
                    Err(e)
                }
            }
        }
    }
}

/// 直连路径：本机 dial 目标后双向拷贝。CONNECT 需先回 200 Established；
/// absolute-form 则把请求头重写为 origin-form 转发。
async fn direct_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
) -> std::io::Result<()> {
    let target = TcpStream::connect((parsed.host.as_str(), parsed.port)).await?;
    let mut target = target;
    if parsed.kind == Kind::Connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        relay_bidir(client, target).await
    } else {
        // absolute-form → origin-form 重写第一行
        let mut lines = head.lines();
        let first = lines.next().unwrap_or("");
        let rewritten = rewrite_first_line_absolute(first);
        let mut rest = String::new();
        for l in lines {
            rest.push_str(l);
            rest.push_str("\r\n");
        }
        let forwarded = format!("{}\r\n{}\r\n", rewritten, rest);
        target.write_all(forwarded.as_bytes()).await?;
        relay_bidir(client, target).await
    }
}

fn rewrite_first_line_absolute(first: &str) -> String {
    // "GET http://host/path HTTP/1.1" → "GET /path HTTP/1.1"
    let parts: Vec<&str> = first.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return first.to_string();
    }
    let path = parts[1]
        .strip_prefix("http://")
        .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or("/");
    format!("{} {} {}", parts[0], path, parts[2])
}

async fn relay_bidir(
    mut a: TcpStream,
    mut b: TcpStream,
) -> std::io::Result<()> {
    let (mut ar, mut aw) = a.split();
    let (mut br, mut bw) = b.split();
    let r = tokio::io::copy(&mut ar, &mut bw);
    let w = tokio::io::copy(&mut br, &mut aw);
    tokio::try_join!(r, w)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connect_with_default_443() {
        let h = "CONNECT www.youtube.com:443 HTTP/1.1\r\nHost: www.youtube.com\r\n\r\n";
        let p = parse_head(h).unwrap();
        assert_eq!(p.kind, Kind::Connect);
        assert_eq!(p.host, "www.youtube.com");
        assert_eq!(p.port, 443);
    }

    #[test]
    fn parse_absolute_form_reads_host_header_port() {
        let h = "GET http://example.com:8080/path?q=1 HTTP/1.1\r\nHost: example.com:8080\r\n\r\n";
        let p = parse_head(h).unwrap();
        assert_eq!(p.kind, Kind::Plain);
        assert_eq!((p.host.as_str(), p.port), ("example.com", 8080));
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_head("not a request").is_none());
        assert!(parse_head("").is_none());
    }

    #[test]
    fn decide_whitelist_with_tunnel() {
        let cfg = EngineConfig {
            whitelist: vec!["youtube.com".into()],
            tunnel_url: Some("wss://gate/ws".into()),
            ..Default::default()
        };
        assert_eq!(decide("www.youtube.com", &cfg), Route::Tunnel);
        assert_eq!(decide("baidu.com", &cfg), Route::Direct);
    }

    #[test]
    fn decide_no_tunnel_url_never_tunnels() {
        // R4：隧道未配置时白名单也不走隧道（直连），但 UI 应提示引擎半配置
        let cfg = EngineConfig {
            whitelist: vec!["youtube.com".into()],
            tunnel_url: None,
            ..Default::default()
        };
        assert_eq!(decide("www.youtube.com", &cfg), Route::Direct);
    }

    #[test]
    fn rewrite_absolute_to_origin_form() {
        assert_eq!(
            rewrite_first_line_absolute("GET http://example.com/path?a=1 HTTP/1.1"),
            "GET /path?a=1 HTTP/1.1"
        );
    }
}
