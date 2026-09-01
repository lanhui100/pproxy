//! CONNECT 隧道网关（强制 Basic Auth / Token 鉴权 + Gatekeeper 防爆破 + WS 隧道中继 + TunnelPool 池化）。
//!
//! 修复红队 Blocker-01 隐患：杜绝任何未鉴权的 CONNECT 免密白嫖出口。
//! 修复红队 3 缺陷：采用单循环 tokio::select! 消除半开死锁，支持 Ping/Pong 保活。
//! 协议对齐（M6 / gate worker）：WS Upgrade 不绑定目标，通过首帧 Text JSON {"host":..., "port":...}
//! 声明目标并等待 {"ok": true}，支持待命连接池（TunnelPool）将冷建连延迟压到 1 RTT。

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use pproxy_core::gatekeeper::AuthGatekeeper;
use pproxy_core::token::TokenService;
use pproxy_core::user::UserService;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue as WsHeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::auth::parse_basic_auth;

/// gate 隧道端点（WS↔TCP 桥）：部署于 gate.ponyjob.top/ws。
const GATE_WS_URL: &str = "wss://gate.ponyjob.top/ws";

/// 单次 WS 建连超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 首帧等待超时。
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// 网络类失败重试：总尝试 2 次。
const MAX_ATTEMPTS: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_millis(400);

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsTx = SplitSink<WsStream, Message>;
type WsRx = SplitStream<WsStream>;

/// 默认开箱即用的白名单（覆盖主流 AI 模型 API、OAuth 认证和代码平台）。
pub const DEFAULT_ALLOWLIST: &[&str] = &[
    "google.com",
    "googleapis.com",
    "gstatic.com",
    "googleusercontent.com",
    "accounts.google.com",
    "goog",
    "g.co",
    "openai.com",
    "chatgpt.com",
    "oaistatic.com",
    "oaiusercontent.com",
    "anthropic.com",
    "claude.ai",
    "github.com",
    "githubusercontent.com",
];

/// 数据面隧道配置。
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub gate_url: String,
    pub token: String,
    pub allowlist: Vec<String>,
}

impl TunnelConfig {
    pub fn build(gate_url: &str, token: &str, custom_allowlist: Option<&[&str]>) -> Self {
        let mut allowlist = DEFAULT_ALLOWLIST
            .iter()
            .map(|s| normalize_host(s))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if let Some(custom) = custom_allowlist {
            for item in custom {
                let s = normalize_host(item);
                if !s.is_empty() && !allowlist.contains(&s) {
                    allowlist.push(s);
                }
            }
        }
        Self {
            gate_url: gate_url.trim().to_string(),
            token: token.trim().to_string(),
            allowlist,
        }
    }

    pub fn from_pool_config_and_env(pool_config: &pproxy_core::PoolConfig) -> Option<Self> {
        let env_url = std::env::var("PPROXY_TUNNEL_GATE_URL").ok();
        let env_token = std::env::var("PPROXY_TUNNEL_TOKEN").ok();
        let env_allowlist = std::env::var("PPROXY_TUNNEL_ALLOWLIST").ok();

        let url = env_url.or_else(|| {
            pool_config.worker_url.as_ref().map(|w| {
                let clean = w
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/');
                let derived = format!("wss://{clean}/ws");
                if derived.starts_with("wss://edge.ponyjob.top")
                    || derived.starts_with("ws://edge.ponyjob.top")
                {
                    GATE_WS_URL.to_string()
                } else {
                    derived
                }
            })
        });
        let token = env_token.or_else(|| pool_config.worker_secret.clone());

        if let (Some(u), Some(t)) = (url, token) {
            let custom = env_allowlist.as_deref().map(|s| {
                s.split(',').map(str::trim).collect::<Vec<_>>()
            });
            Some(Self::build(&u, &t, custom.as_deref()))
        } else {
            None
        }
    }
}

/// 待命 WS 会话。
struct IdleSession {
    tx: WsTx,
    rx: WsRx,
    born: tokio::time::Instant,
}

/// 待命隧道池：CONNECT 到达前预建 WS 会话，将冷建连 RTT 从 ~5 RTT 压至 1 RTT。
pub struct TunnelPool {
    cfg: TunnelConfig,
    idle: std::sync::Mutex<Vec<IdleSession>>,
    notify: tokio::sync::Notify,
    size: usize,
}

const POOL_SIZE: usize = 2;
const IDLE_TTL: Duration = Duration::from_secs(30);
const REFILL_INTERVAL: Duration = Duration::from_secs(2);
const REFILL_BACKOFF: Duration = Duration::from_secs(5);

impl TunnelPool {
    pub fn new(cfg: TunnelConfig) -> Arc<Self> {
        Self::with_size(cfg, POOL_SIZE)
    }

    pub fn with_size(cfg: TunnelConfig, size: usize) -> Arc<Self> {
        let pool = Arc::new(Self {
            cfg,
            idle: std::sync::Mutex::new(Vec::new()),
            notify: tokio::sync::Notify::new(),
            size,
        });
        tokio::spawn(Self::maintain(Arc::clone(&pool)));
        pool
    }

    #[cfg(test)]
    pub fn disabled(cfg: TunnelConfig) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            idle: std::sync::Mutex::new(Vec::new()),
            notify: tokio::sync::Notify::new(),
            size: 0,
        })
    }

    #[cfg(test)]
    pub fn idle_len(&self) -> usize {
        self.idle.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn config(&self) -> &TunnelConfig {
        &self.cfg
    }

    pub fn checkout(&self) -> Option<(WsTx, WsRx)> {
        let mut guard = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        while let Some(s) = guard.pop() {
            if s.born.elapsed() < IDLE_TTL {
                drop(guard);
                self.notify.notify_one();
                return Some((s.tx, s.rx));
            }
        }
        None
    }

    async fn maintain(pool: Arc<Self>) {
        loop {
            {
                let mut guard = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                guard.retain(|s| s.born.elapsed() < IDLE_TTL);
            }
            let need = pool
                .size
                .saturating_sub(pool.idle.lock().unwrap_or_else(|p| p.into_inner()).len());
            let mut healthy = true;
            for _ in 0..need {
                match connect_ws(&pool.cfg).await {
                    Ok((tx, rx)) => {
                        pool.idle.lock().unwrap_or_else(|p| p.into_inner()).push(IdleSession {
                            tx,
                            rx,
                            born: tokio::time::Instant::now(),
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "tunnel pool refill failed");
                        healthy = false;
                        break;
                    }
                }
            }
            let wait = if healthy { REFILL_INTERVAL } else { REFILL_BACKOFF };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = pool.notify.notified() => {}
            }
        }
    }
}

/// 提取 CONNECT 请求头中的认证信息并校验。
pub fn verify_connect_credentials(
    head: &str,
    users: Option<&UserService>,
    tokens: &TokenService,
) -> bool {
    let mut basic_auth = None;
    let mut token_header = None;

    for line in head.lines() {
        let line = line.trim();
        if line.to_ascii_lowercase().starts_with("proxy-authorization:") {
            if let Some((_, val)) = line.split_once(':') {
                basic_auth = parse_basic_auth(val);
            }
        } else if line.to_ascii_lowercase().starts_with("x-pony-token:") {
            if let Some((_, val)) = line.split_once(':') {
                token_header = Some(val.trim().to_string());
            }
        }
    }

    // 1. 优先校验 Basic Auth (username, password)
    if let Some((username, password)) = basic_auth {
        if let Some(user_service) = users {
            if user_service.verify_user(&username, &password).is_some() {
                return true;
            }
        }
        // 如果用户名匹配 token，也允许作为 token 校验
        if tokens.verify(&username).is_ok() || tokens.verify(&password).is_ok() {
            return true;
        }
        return false;
    }

    // 2. 其次校验 X-Pony-Token
    if let Some(tok) = token_header {
        if tokens.verify(&tok).is_ok() {
            return true;
        }
    }

    false
}

/// 处理裸 CONNECT 隧道请求（带鉴权阻断与池化加速）。
pub async fn handle_connect_raw(
    client_ip: IpAddr,
    head: &str,
    leftover: Vec<u8>,
    mut stream: TcpStream,
    tunnel_pool: Option<Arc<TunnelPool>>,
    users: Option<Arc<UserService>>,
    tokens: Arc<TokenService>,
    gatekeeper: Arc<AuthGatekeeper>,
) {
    // 1. 防爆破门禁检查（Lockout Check）
    if let Err(_msg) = gatekeeper.check(&client_ip) {
        tracing::warn!(%client_ip, "connect rejected: IP locked by AuthGatekeeper");
        let _ = stream
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\n\
                  content-type: application/json\r\n\
                  content-length: 59\r\n\r\n\
                  {\"error\":\"ip_temporarily_locked_due_to_auth_failures\"}",
            )
            .await;
        return;
    }

    // 2. 强制身份鉴权（MANDATORY AUTHENTICATION - 堵死 Blocker-01）
    let is_auth_ok = verify_connect_credentials(head, users.as_deref(), &tokens);
    if !is_auth_ok {
        gatekeeper.record_failure(client_ip);
        tracing::warn!(%client_ip, "connect denied: missing or invalid authentication credentials");
        let _ = stream
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  proxy-authenticate: Basic realm=\"Pony Proxy\"\r\n\
                  content-type: application/json\r\n\
                  content-length: 43\r\n\r\n\
                  {\"error\":\"proxy_authentication_required\"}",
            )
            .await;
        return;
    }

    // 鉴权通过，重置失败计数
    gatekeeper.record_success(&client_ip);

    // 3. 从 CONNECT 首行解析 host:port
    let (host, port) = match parse_connect_head(head) {
        Some(v) => v,
        None => {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nx-pproxy-reason: bad_target\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
            return;
        }
    };

    let Some(pool) = tunnel_pool.as_ref() else {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: tunnel_not_configured\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    };

    if port != 443 {
        tracing::info!(host = %host, port, "connect denied: port not allowed");
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: port_not_allowed\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    }

    if !allowlist_match(&host, &pool.config().allowlist) {
        tracing::info!(host = %host, "connect denied: no tunnel route");
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: no_tunnel_route\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    }

    // 4. Establish WebSocket Tunnel (优先池化 checkout，失败或重试走全新建连)
    for attempt in 0..MAX_ATTEMPTS {
        let pooled_session = if attempt == 0 { pool.checkout() } else { None };
        let used_pool = pooled_session.is_some();
        let result = match pooled_session {
            Some((tx, rx)) => bind_target(tx, rx, &host, port).await,
            None => establish(pool.config(), &host, port).await,
        };
        match result {
            Ok((ws_tx, ws_rx)) => {
                tracing::info!(host = %host, attempt, pooled = used_pool, "tunnel established successfully");
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                relay(stream, leftover, ws_tx, ws_rx).await;
                return;
            }
            Err(e) => {
                let retryable = matches!(e, EstablishError::Network(_));
                tracing::warn!(host = %host, attempt, pooled = used_pool, error = %e, "tunnel establish failed");
                if !retryable || attempt + 1 >= MAX_ATTEMPTS {
                    break;
                }
                // 若本次失败来自待命池死会话，立即降级全新建连，无需等待 400ms 退避
                if !used_pool {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }

    // 5. 失败写 502
    let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nx-pproxy-reason: tunnel_failed\r\ncontent-type: application/json\r\ncontent-length: 25\r\n\r\n{\"error\":\"tunnel_failed\"}").await;
}

pub fn parse_connect_head(head: &str) -> Option<(String, u16)> {
    let first_line = head.lines().next()?.trim();
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    if !method.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    let target = parts.next()?;
    split_host_port(target)
}

fn split_host_port(authority: &str) -> Option<(String, u16)> {
    if let Some(stripped) = authority.strip_prefix('[') {
        let (h, rest) = stripped.split_once(']')?;
        let port = rest.strip_prefix(':').and_then(|p| p.parse().ok())?;
        return Some((h.to_string(), port));
    }
    let (host, port_str) = authority.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    let host = normalize_host(host);
    if host.is_empty() {
        return None;
    }
    Some((host, port))
}

pub fn allowlist_match(host: &str, allowlist: &[String]) -> bool {
    let h = normalize_host(host);
    if h.is_empty() {
        return false;
    }
    allowlist.iter().any(|e| suffix_match(&h, &normalize_host(e)))
}

fn normalize_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    let h = h.trim_start_matches('*').trim_start_matches('.');
    let mut h = h.to_string();
    while h.ends_with('.') {
        h.pop();
    }
    h
}

fn suffix_match(host: &str, entry: &str) -> bool {
    if entry.is_empty() || !host.ends_with(entry) {
        return false;
    }
    let rest = &host[..host.len() - entry.len()];
    rest.is_empty() || rest.ends_with('.')
}

#[derive(Debug)]
pub enum EstablishError {
    Handshake(String),
    Network(String),
    Denied(String),
}

impl fmt::Display for EstablishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake(s) => write!(f, "handshake rejected: {s}"),
            Self::Network(s) => write!(f, "network error: {s}"),
            Self::Denied(s) => write!(f, "denied: {s}"),
        }
    }
}

impl std::error::Error for EstablishError {}

/// 仅完成 TCP+TLS+WS Upgrade（Bearer token 认证，不携带目标 host）。
/// 支持逗号分隔多个 Gate 端点（如 wss://gate1,wss://gate2），按顺序故障转移。
pub async fn connect_ws(cfg: &TunnelConfig) -> Result<(WsTx, WsRx), EstablishError> {
    let endpoints: Vec<&str> = cfg
        .gate_url
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut last_err = None;
    for endpoint in endpoints {
        let req_result = endpoint.into_client_request();
        let mut req = match req_result {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(EstablishError::Handshake(format!("bad gate url '{endpoint}': {e}")));
                continue;
            }
        };

        if let Ok(val) = WsHeaderValue::from_str(&format!("Bearer {}", cfg.token)) {
            req.headers_mut().insert("authorization", val);
        }

        match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(req)).await {
            Ok(Ok((ws_stream, _))) => {
                return Ok(ws_stream.split());
            }
            Ok(Err(e)) => {
                last_err = Some(EstablishError::Network(format!("{endpoint}: {e}")));
            }
            Err(_) => {
                last_err = Some(EstablishError::Network(format!("{endpoint}: ws connect timeout")));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| EstablishError::Network("no valid gate endpoints configured".into())))
}

/// 在已建立的 WS 会话上声明目标（首帧 Text JSON {"host":..., "port":...} → 等 {"ok":true}）。
pub async fn bind_target(
    mut tx: WsTx,
    mut rx: WsRx,
    host: &str,
    port: u16,
) -> Result<(WsTx, WsRx), EstablishError> {
    let first = serde_json::json!({ "host": host, "port": port }).to_string();
    tx.send(Message::Text(first))
        .await
        .map_err(|e| EstablishError::Network(e.to_string()))?;

    let deadline = tokio::time::Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.next())
            .await
            .map_err(|_| EstablishError::Network("first-frame timeout".into()))?
            .ok_or_else(|| EstablishError::Network("closed before ok".into()))?
            .map_err(|e| EstablishError::Network(e.to_string()))?;

        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t)
                    .map_err(|_| EstablishError::Network("bad first-frame json".into()))?;
                if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                    return Ok((tx, rx));
                }
                let reason = v
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("denied")
                    .to_string();
                return Err(EstablishError::Denied(reason));
            }
            Message::Ping(p) => {
                tx.send(Message::Pong(p))
                    .await
                    .map_err(|e| EstablishError::Network(e.to_string()))?;
            }
            Message::Close(c) => return Err(EstablishError::Network(format!("closed: {c:?}"))),
            _ => {}
        }
    }
}

/// 全新建连：connect_ws + bind_target。
pub async fn establish(
    cfg: &TunnelConfig,
    host: &str,
    port: u16,
) -> Result<(WsTx, WsRx), EstablishError> {
    let (tx, rx) = connect_ws(cfg).await?;
    bind_target(tx, rx, host, port).await
}

/// 双向中继：单循环 tokio::select! 保证任意一端断开立即整体释放，支持 WebSocket Ping/Pong 保活。
async fn relay(
    mut tcp: TcpStream,
    leftover: Vec<u8>,
    mut ws_tx: WsTx,
    mut ws_rx: WsRx,
) {
    let (mut tcp_rx, mut tcp_tx) = tcp.split();
    let mut buf = vec![0u8; 16384];

    if !leftover.is_empty() {
        if ws_tx.send(Message::Binary(leftover)).await.is_err() {
            let _ = tcp_tx.shutdown().await;
            return;
        }
    }

    loop {
        tokio::select! {
            n = tcp_rx.read(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            },
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Binary(bin))) => {
                    if tcp_tx.write_all(&bin).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    let _ = ws_tx.send(Message::Pong(p)).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }

    let _ = tcp_tx.shutdown().await;
    let _ = ws_tx.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    struct StubGate {
        url: String,
        auth_captured: Arc<std::sync::Mutex<Option<String>>>,
        target_captured: Arc<std::sync::Mutex<Option<(String, u64)>>>,
        conns: Arc<AtomicUsize>,
    }

    async fn spawn_stub_gate(ok: bool, reason: Option<&str>) -> StubGate {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let auth_captured = Arc::new(std::sync::Mutex::new(None));
        let target_captured = Arc::new(std::sync::Mutex::new(None));
        let conns = Arc::new(AtomicUsize::new(0));

        let auth_c = Arc::clone(&auth_captured);
        let target_c = Arc::clone(&target_captured);
        let conns_c = Arc::clone(&conns);
        let reason_s = reason.map(str::to_string);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                conns_c.fetch_add(1, Ordering::SeqCst);
                let auth_c = Arc::clone(&auth_c);
                let target_c = Arc::clone(&target_c);
                let reason_s = reason_s.clone();

                tokio::spawn(async move {
                    let callback = |req: &tokio_tungstenite::tungstenite::http::Request<()>,
                                    resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                        // 验证 Upgrade 不带 x-target-host / x-target-port
                        assert!(req.headers().get("x-target-host").is_none());
                        assert!(req.headers().get("x-target-port").is_none());
                        let auth = req.headers().get("authorization").map(|v| v.to_str().unwrap().to_string());
                        *auth_c.lock().unwrap() = auth;
                        Ok(resp)
                    };

                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };

                    while let Some(Ok(msg)) = ws.next().await {
                        match msg {
                            Message::Text(txt) => {
                                let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                                let host = v.get("host").and_then(|h| h.as_str()).unwrap().to_string();
                                let port = v.get("port").and_then(|p| p.as_u64()).unwrap();
                                *target_c.lock().unwrap() = Some((host, port));

                                if ok {
                                    ws.send(Message::Text(serde_json::json!({"ok": true}).to_string())).await.unwrap();
                                    // 回显二进制
                                    while let Some(Ok(bin_msg)) = ws.next().await {
                                        if let Message::Binary(b) = bin_msg {
                                            let _ = ws.send(Message::Binary(b)).await;
                                        } else if bin_msg.is_close() {
                                            break;
                                        }
                                    }
                                } else {
                                    ws.send(Message::Text(serde_json::json!({
                                        "ok": false,
                                        "reason": reason_s.as_deref().unwrap_or("denied")
                                    }).to_string())).await.unwrap();
                                    let _ = ws.close(None).await;
                                }
                                break;
                            }
                            Message::Ping(p) => {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });

        StubGate {
            url: format!("ws://{addr}/ws"),
            auth_captured,
            target_captured,
            conns,
        }
    }

    #[test]
    fn parse_connect_head_valid() {
        let head = "CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com:443\r\n\r\n";
        assert_eq!(
            parse_connect_head(head),
            Some(("api.openai.com".to_string(), 443))
        );
    }

    #[test]
    fn parse_connect_head_invalid() {
        assert_eq!(parse_connect_head("GET / HTTP/1.1\r\n\r\n"), None);
        assert_eq!(parse_connect_head("CONNECT invalid_host HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn allowlist_matching_rules() {
        let allowlist = vec!["openai.com".to_string(), "anthropic.com".to_string()];
        assert!(allowlist_match("openai.com", &allowlist));
        assert!(allowlist_match("api.openai.com", &allowlist));
        assert!(allowlist_match("chat.openai.com", &allowlist));
        assert!(!allowlist_match("notopenai.com", &allowlist));
        assert!(!allowlist_match("openai.com.evil.cn", &allowlist));
        assert!(!allowlist_match("google.com", &allowlist));
    }

    #[tokio::test]
    async fn establish_sends_first_frame_and_expects_ok() {
        let stub = spawn_stub_gate(true, None).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "secret-tok".into(),
            allowlist: vec!["openai.com".into()],
        };

        let result = establish(&cfg, "api.openai.com", 443).await;
        assert!(result.is_ok(), "establish should succeed on ok:true response: {result:?}");

        assert_eq!(
            stub.auth_captured.lock().unwrap().as_deref(),
            Some("Bearer secret-tok"),
            "Upgrade request must contain Bearer authorization"
        );
        assert_eq!(
            stub.target_captured.lock().unwrap().clone(),
            Some(("api.openai.com".to_string(), 443)),
            "First frame text JSON must declare host and port"
        );
    }

    #[tokio::test]
    async fn establish_handles_denied_reason() {
        let stub = spawn_stub_gate(false, Some("acl denied")).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "secret-tok".into(),
            allowlist: vec!["openai.com".into()],
        };

        let result = establish(&cfg, "evil.com", 443).await;
        match result {
            Err(EstablishError::Denied(reason)) => {
                assert_eq!(reason, "acl denied");
            }
            other => panic!("expected EstablishError::Denied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tunnel_pool_preconnects_and_binds_target() {
        let stub = spawn_stub_gate(true, None).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "pool-tok".into(),
            allowlist: vec!["google.com".into()],
        };
        let pool = TunnelPool::new(cfg);

        // 等待待命连接进入池
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_len() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "Pool failed to preconnect in 5s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(stub.conns.load(Ordering::SeqCst) >= 1);
        let (tx, rx) = pool.checkout().expect("session should be in pool");
        let bind_res = bind_target(tx, rx, "api.google.com", 443).await;
        assert!(bind_res.is_ok());

        assert_eq!(
            stub.target_captured.lock().unwrap().clone(),
            Some(("api.google.com".to_string(), 443))
        );
    }

    #[tokio::test]
    async fn dead_pooled_session_falls_back_to_fresh_establish() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = Arc::new(AtomicUsize::new(0));
        let conns2 = Arc::clone(&conns);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                conns2.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let callback = |_req: &tokio_tungstenite::tungstenite::http::Request<()>,
                                    resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                        Ok(resp)
                    };
                    // 第一次连接：upgrade 成功后立即关闭（模拟死会话）
                    if let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await {
                        let _ = ws.close(None).await;
                    }
                });
            }
        });

        let cfg = TunnelConfig {
            gate_url: format!("ws://{addr}/ws"),
            token: "tok".into(),
            allowlist: vec!["google.com".into()],
        };
        let pool = TunnelPool::new(cfg);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_len() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池化未预建会话");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(conns.load(Ordering::SeqCst) >= 1);

        let (tx, rx) = pool.checkout().expect("池中应有待命会话");
        let err = bind_target(tx, rx, "api.google.com", 443).await;
        assert!(
            matches!(err, Err(EstablishError::Network(_))),
            "死会话 bind 必须归类 Network 错误以触发无缝重试, got {err:?}"
        );
    }
}
