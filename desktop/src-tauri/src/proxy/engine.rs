//! 引擎核心：127.0.0.1:18900 监听，CONNECT/absolute-form 分流。
//!
//! 决策：host 命中白名单 → [`tunnel`]（WS 隧道，M6 后续任务实现）；未命中 →
//! 本机直接 dial。失败语义：隧道不可用即关闭连接并计数，绝不静默回落直连
//! （spec §5，R4）。
//! T1: watch 通道热更新，handle_conn 与 /pac 每请求 clone 后立即 drop 再 matches/generate_pac，不持锁跨 await。
#![deny(clippy::await_holding_lock)]

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::pac;
use super::whitelist;

/// 远端上游代理（方案 B chained 模式）：白名单流量经该 HTTP 代理 CONNECT 出网。
///
/// 不派生 `Debug`：密码会以明文出现在任何 `{:?}` 里（含 panic 回溯与日志）。
#[derive(Clone, PartialEq)]
pub struct Upstream {
    /// host:port（不含 scheme）
    pub host: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("host", &self.host)
            .field("username", &self.username)
            // host/username 保留以便排障，password 一律脱敏
            .field("password", &"***")
            .finish()
    }
}

/// 引擎配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub listen_addr: String,
    pub whitelist: watch::Receiver<Vec<String>>,
    pub mode: watch::Receiver<pac::ProxyMode>,
    /// 隧道凭据热更新通道 (url, token)
    pub tunnel: watch::Receiver<(Option<String>, Option<String>)>,
    /// 远端上游代理热更新通道（chained 模式；Some 时优先于 WS gate）
    pub upstream: watch::Receiver<Option<Upstream>>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        // 创建空 watch channel 供测试使用
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((None, None));
        let (_utx, urx) = watch::channel(None);
        EngineConfig {
            listen_addr: "127.0.0.1:18900".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
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
    /// 隧道出网按出口归账的字节/连接计数：CF gate 与 Vercel gate 分别统计，
    /// 供「Cloudflare / Vercel 用量」展示；直连与 chained 上游不消耗两家额度，不计入。
    pub cf_up: AtomicU64,
    pub cf_down: AtomicU64,
    pub cf_reqs: AtomicU64,
    pub vercel_up: AtomicU64,
    pub vercel_down: AtomicU64,
    pub vercel_reqs: AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
}

pub type SharedStats = Arc<EngineStats>;

/// 启动引擎循环（由 tauri setup / 命令调用；返回前先绑定端口）。
pub async fn run(cfg: EngineConfig, stats: SharedStats) -> std::io::Result<()> {
    let listener = TcpListener::bind(&cfg.listen_addr).await?;
    let cfg = Arc::new(cfg);
    loop {
        let (stream, _) = listener.accept().await?;
        stats
            .conns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cfg_clone = cfg.clone();
        let stats_clone = stats.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, &cfg_clone, &stats_clone).await {
                stats_clone
                    .errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut g) = stats_clone.last_error.lock() {
                    *g = Some(e.to_string());
                }
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

/// 分流判定（纯函数）：根据工作模式（白名单/全局）决定路由。
/// 严禁在隧道未配置时静默回落 Direct（R4 契约）；本地地址与 Bypass 管理面强制 Direct。
fn decide(host: &str, whitelist_snapshot: &[String], mode: pac::ProxyMode) -> Route {
    let norm = whitelist::normalize_host(host);
    // 防回环：本地与 Bypass 管理面强制 Direct
    let bypass_set = pac::collect_bypass_hosts();
    if norm == "localhost" || norm == "127.0.0.1" || norm == "::1" || bypass_set.contains(&norm) {
        return Route::Direct;
    }
    match mode {
        pac::ProxyMode::Global => Route::Tunnel,
        pac::ProxyMode::Whitelist => {
            if whitelist::matches(&norm, whitelist_snapshot) {
                Route::Tunnel
            } else {
                Route::Direct
            }
        }
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
        // IPv6 字面量 [::1]:port 或 [::1]
        let (h, rest) = stripped.split_once(']')?;
        if rest.is_empty() {
            return Some((h.to_string(), default_port));
        }
        let port_str = rest.strip_prefix(':')?;
        let port: u16 = port_str.parse().ok()?;
        return Some((h.to_string(), port));
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

    // 引擎自身的控制面：PAC 脚本（放行 /pac 或 /pac?t=...）
    let is_pac_req = if let Some(first) = head.lines().next() {
        let path = first.strip_prefix("GET ").and_then(|r| r.split_whitespace().next()).unwrap_or("");
        path == "/pac"
            || path.starts_with("/pac?")
            || path.starts_with("http://127.0.0.1:18900/pac")
            || path.starts_with("http://localhost:18900/pac")
    } else {
        false
    };

    if is_pac_req {
        let wl_snapshot = {
            let guard = cfg.whitelist.borrow();
            guard.clone()
        };
        let mode_snapshot = *cfg.mode.borrow();
        let body = pac::generate_pac(&wl_snapshot, mode_snapshot);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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

    // T1: decide 前先 clone whitelist & mode snapshot 并 drop
    let wl_snapshot = {
        let g = cfg.whitelist.borrow();
        g.clone()
    };
    let mode_snapshot = *cfg.mode.borrow();
    let route = decide(&parsed.host, &wl_snapshot, mode_snapshot);
    match route {
        Route::Direct => {
            stats.direct.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            direct_relay(stream, parsed, &head).await
        }
        Route::Tunnel => {
            stats.tunneled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let upstream = cfg.upstream.borrow().clone();
            let result = match upstream {
                Some(u) => super::engine_upstream::connect_and_relay(stream, parsed, &head, &u).await,
                None => super::engine_tunnel::connect_and_relay(stream, parsed, &head, cfg, stats).await,
            };
            match result {
                Ok(()) => Ok(()),
                Err(e) => {
                    stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    *stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(e.to_string());
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
    let target = match TcpStream::connect((parsed.host.as_str(), parsed.port)).await {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("502 Bad Gateway: direct dial {} failed: {}", parsed.host, e);
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                msg.len(),
                msg
            );
            let _ = client.write_all(resp.as_bytes()).await;
            return Err(e);
        }
    };
    let mut target = target;
    if parsed.kind == Kind::Connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        relay_bidir(client, target).await
    } else {
        let forwarded = rebuild_request(head);
        target.write_all(forwarded.as_bytes()).await?;
        relay_bidir(client, target).await
    }
}

/// 重写明文请求：首行 absolute-form → origin-form，其余头原样保留（含结尾空行与 body）。
///
/// 严禁用 `head.lines()` 逐行重建：`lines()` 会把结尾的 `\r\n\r\n` 拆出一个额外空元素，
/// 重建后变成 `...\r\n\r\n\r\n`，明文 POST 的 body 会被这 2 字节前缀破坏。
/// 这里用 `split_once("\r\n")` 分离首行与剩余，剩余原样拼回（同 engine_upstream::rebuild_request）。
fn rebuild_request(head: &str) -> String {
    // 退化输入（无 CRLF）时补一个空行，保证请求头完整
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, "\r\n\r\n"));
    format!("{}\r\n{}", rewrite_first_line_absolute(first), rest)
}

fn rewrite_first_line_absolute(first: &str) -> String {
    let parts: Vec<&str> = first.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return first.to_string();
    }
    let after_scheme = parts[1].strip_prefix("http://").unwrap_or(parts[1]);
    let path_start = after_scheme.find(['/', '?']);
    let path = match path_start {
        Some(i) if after_scheme.as_bytes()[i] == b'?' => {
            format!("/{}", &after_scheme[i..])
        }
        Some(i) => after_scheme[i..].to_string(),
        None => "/".to_string(),
    };
    format!("{} {} {}", parts[0], path, parts[2])
}

/// 双向透传（单循环双方向，半关闭语义正确）。
///
/// 设计要点（吸取两版教训）：
/// - **禁止 `try_join!`**：一端关闭而另一端保持连接（keep-alive）时永远等不到 EOF，任务泄漏；
/// - **禁止 `select!` 取消另一方向**：被取消的 copy 已读未写的数据会静默丢弃，响应被截断；
/// - 正确做法：单一循环里同时监听两端读，单端 EOF 时对该方向对端写半区发 FIN（半关闭），
///   另一端继续转发直到也 EOF，两端都 EOF 才退出。
pub(crate) async fn relay_bidir(
    a: TcpStream,
    b: TcpStream,
) -> std::io::Result<()> {
    let (mut ar, mut aw) = a.into_split();
    let (mut br, mut bw) = b.into_split();
    let mut a_eof = false;
    let mut b_eof = false;
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];
    loop {
        if a_eof && b_eof {
            break;
        }
        tokio::select! {
            n = ar.read(&mut buf_a), if !a_eof => {
                let n = n?;
                if n == 0 {
                    a_eof = true;
                    let _ = bw.shutdown().await;
                } else {
                    bw.write_all(&buf_a[..n]).await?;
                }
            }
            n = br.read(&mut buf_b), if !b_eof => {
                let n = n?;
                if n == 0 {
                    b_eof = true;
                    let _ = aw.shutdown().await;
                } else {
                    aw.write_all(&buf_b[..n]).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_whitelist(list: Vec<String>, tunnel: Option<String>) -> EngineConfig {
        let (_tx, rx) = watch::channel(list);
        let (_ttx, trx) = watch::channel((tunnel, Some("mock-token".into())));
        EngineConfig { whitelist: rx, tunnel: trx, ..Default::default() }
    }

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
        let cfg = cfg_with_whitelist(vec!["youtube.com".into()], Some("wss://gate/ws".into()));
        let snap = cfg.whitelist.borrow().clone();
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Whitelist), Route::Tunnel);
        assert_eq!(decide("baidu.com", &snap, pac::ProxyMode::Whitelist), Route::Direct);
    }

    #[test]
    fn decide_global_mode_tunnels_all() {
        let cfg = cfg_with_whitelist(vec![], Some("wss://gate/ws".into()));
        let snap = cfg.whitelist.borrow().clone();
        assert_eq!(decide("baidu.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
        assert_eq!(decide("google.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
    }

    #[test]
    fn decide_no_tunnel_url_still_routes_to_tunnel_to_prevent_silent_fallback() {
        let cfg = cfg_with_whitelist(vec!["youtube.com".into()], None);
        let snap = cfg.whitelist.borrow().clone();
        // R4 契约：白名单/全局目标必须 Route::Tunnel（进入建连阶段后因无配置而关闭连接，绝不回落 Direct 裸连）
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Whitelist), Route::Tunnel);
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
        // 本地回环与 Bypass 目标仍必须走 Direct 防自锁
        assert_eq!(decide("localhost", &snap, pac::ProxyMode::Global), Route::Direct);
        assert_eq!(decide("127.0.0.1", &snap, pac::ProxyMode::Global), Route::Direct);
    }

    #[test]
    fn rebuild_request_keeps_single_blank_line_and_body() {
        // F2：重建后不得多出 CRLF，否则明文 POST 的 body 被 2 字节前缀破坏
        assert_eq!(
            rebuild_request("POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"),
            "POST /y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
        assert_eq!(
            rebuild_request("GET http://x/y?a=1 HTTP/1.1\r\nHost: x\r\n\r\n"),
            "GET /y?a=1 HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }

    #[test]
    fn upstream_debug_redacts_password() {
        // F3：任何 {:?} 都不得输出明文密码
        let u = Upstream {
            host: "proxy.example.com:8899".into(),
            username: "alice".into(),
            password: "super-secret-pw".into(),
        };
        let shown = format!("{u:?}");
        assert!(!shown.contains("super-secret-pw"), "密码泄露: {shown}");
        assert!(shown.contains("***"), "密码位应脱敏: {shown}");
        assert!(shown.contains("proxy.example.com:8899") && shown.contains("alice"), "host/username 应保留便于排障: {shown}");
    }

    #[test]
    fn engine_config_debug_does_not_leak_upstream_password() {
        // EngineConfig 派生 Debug 会委托给 Upstream 的手写实现，逐层确认不泄露
        let (_tx, rx) = watch::channel(Some(Upstream {
            host: "h:1".into(),
            username: "u".into(),
            password: "top-secret".into(),
        }));
        let cfg = EngineConfig { upstream: rx, ..Default::default() };
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("top-secret"), "密码泄露: {shown}");
    }

    #[test]
    fn rewrite_absolute_to_origin_form() {
        assert_eq!(
            rewrite_first_line_absolute("GET http://example.com/path?a=1 HTTP/1.1"),
            "GET /path?a=1 HTTP/1.1"
        );
        assert_eq!(
            rewrite_first_line_absolute("GET http://example.com?a=1 HTTP/1.1"),
            "GET /?a=1 HTTP/1.1"
        );
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::sync::Arc;

    async fn socket_pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let c = TcpStream::connect(addr).await.unwrap();
        let s = l.accept().await.unwrap().0;
        (c, s)
    }

    #[tokio::test]
    async fn direct_relay_end_to_end_echo() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = echo.accept().await {
                    tokio::spawn(async move {
                        let (mut r, mut w) = s.into_split();
                        tokio::io::copy(&mut r, &mut w).await.ok();
                    });
                }
            }
        });

        let cfg = EngineConfig::default(); // 空白名单 → 全直连
        let stats = Arc::new(EngineStats::default());
        let (mut client, srv) = socket_pair().await;
        let st2 = Arc::clone(&stats);
        tokio::spawn(async move {
            handle_conn(srv, &cfg, &st2).await.ok();
        });

        let mut buf = [0u8; 256];
        client
            .write_all(format!("CONNECT {echo_addr} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let n = client.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 Connection Established"));
        client.write_all(b"PING").await.unwrap();
        loop {
            let n = client.read(&mut buf).await.unwrap();
            if n == 0 {
                panic!("echo 连接提前关闭");
            }
            if &buf[..n] == b"PING" {
                break;
            }
        }
        assert_eq!(
            stats.direct.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "direct 计数应+1"
        );
    }

    #[tokio::test]
    async fn r4_tunnel_failure_never_silently_falls_back() {
        let (_tx, rx) = watch::channel(vec!["www.youtube.com".into()]);
        let (_ttx, trx) = watch::channel((Some("ws://127.0.0.1:9/unreachable".into()), Some("mock-token".into())));
        let cfg = EngineConfig {
            whitelist: rx,
            tunnel: trx,
            ..Default::default()
        };
        let stats = Arc::new(EngineStats::default());
        let (client, srv) = socket_pair().await;
        let mut client = client;
        let st2 = Arc::clone(&stats);
        tokio::spawn(async move {
            handle_conn(srv, &cfg, &st2).await.ok();
        });
        client
            .write_all(b"CONNECT www.youtube.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let n = client.read(&mut buf).await.unwrap_or(0);
        let body = String::from_utf8_lossy(&buf[..n]);
        assert!(
            !body.contains("200 Connection Established"),
            "隧道失败不得回 200 Established"
        );
        assert!(
            n == 0 || body.contains("denied") || body.contains("tunnel"),
            "失败语义应为显式错误/关闭，实际: {body}"
        );
        assert_eq!(
            stats.errors.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "错误计数应+1"
        );
    }
}
