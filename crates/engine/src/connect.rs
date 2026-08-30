//! CONNECT 隧道网关（强制 Basic Auth / Token 鉴权 + Gatekeeper 防爆破 + WS 隧道中继）。
//!
//! 修复红队 Blocker-01 隐患：杜绝任何未鉴权的 CONNECT 免密白嫖出口。
//! 修复红队 3 缺陷：采用单循环 tokio::select! 消除半开死锁，支持 Ping/Pong 保活。

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
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>();
        if let Some(custom) = custom_allowlist {
            for item in custom {
                let s = item.trim().to_lowercase();
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
                let clean = w.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
                format!("wss://{clean}/ws")
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

/// 处理裸 CONNECT 隧道请求（带鉴权阻断）。
pub async fn handle_connect_raw(
    client_ip: IpAddr,
    head: &str,
    leftover: Vec<u8>,
    mut stream: TcpStream,
    tunnel_cfg: Option<Arc<TunnelConfig>>,
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

    let Some(cfg) = tunnel_cfg.as_ref() else {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: tunnel_not_configured\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    };

    if port != 443 {
        tracing::info!(host = %host, port, "connect denied: port not allowed");
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: port_not_allowed\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    }

    if !allowlist_match(&host, &cfg.allowlist) {
        tracing::info!(host = %host, "connect denied: no tunnel route");
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nx-pproxy-reason: no_tunnel_route\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
        return;
    }

    // 4. Establish WebSocket Tunnel
    for attempt in 0..MAX_ATTEMPTS {
        match establish(cfg, &host, port).await {
            Ok((ws_tx, ws_rx)) => {
                tracing::info!(host = %host, "tunnel established successfully");
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                relay(stream, leftover, ws_tx, ws_rx).await;
                return;
            }
            Err(e) => {
                let retryable = matches!(e, EstablishError::Network(_));
                tracing::warn!(host = %host, attempt, error = %e, "tunnel establish failed");
                if !retryable || attempt + 1 >= MAX_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(RETRY_DELAY).await;
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
    let (host, port_str) = target.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    let host = host.trim_matches('[').trim_matches(']').to_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, port))
}

pub fn allowlist_match(host: &str, allowlist: &[String]) -> bool {
    let host = host.to_lowercase();
    for rule in allowlist {
        let rule = rule.to_lowercase();
        if host == rule {
            return true;
        }
        if host.ends_with(&format!(".{rule}")) {
            return true;
        }
    }
    false
}

#[derive(Debug)]
enum EstablishError {
    Handshake(String),
    Network(String),
    Protocol(String),
    Timeout,
}

impl fmt::Display for EstablishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake(s) => write!(f, "handshake rejected: {s}"),
            Self::Network(s) => write!(f, "network error: {s}"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
            Self::Timeout => write!(f, "first frame timed out"),
        }
    }
}

async fn establish(
    cfg: &TunnelConfig,
    host: &str,
    port: u16,
) -> Result<(WsTx, WsRx), EstablishError> {
    let mut req = cfg
        .gate_url
        .as_str()
        .into_client_request()
        .map_err(|e| EstablishError::Handshake(e.to_string()))?;

    req.headers_mut().insert(
        "authorization",
        WsHeaderValue::from_str(&format!("Bearer {}", cfg.token))
            .map_err(|e| EstablishError::Handshake(e.to_string()))?,
    );
    req.headers_mut().insert(
        "x-target-host",
        WsHeaderValue::from_str(host).map_err(|e| EstablishError::Handshake(e.to_string()))?,
    );
    req.headers_mut().insert(
        "x-target-port",
        WsHeaderValue::from_str(&port.to_string())
            .map_err(|e| EstablishError::Handshake(e.to_string()))?,
    );

    let (ws_stream, _) = connect_async(req)
        .await
        .map_err(|e| EstablishError::Network(e.to_string()))?;

    let (ws_tx, ws_rx) = ws_stream.split();

    // 等待首帧 `{"type":"connected"}`
    let mut ws_rx = ws_rx;
    let first = tokio::time::timeout(FIRST_FRAME_TIMEOUT, ws_rx.next()).await;
    match first {
        Ok(Some(Ok(Message::Text(txt)))) => {
            if txt.contains("\"connected\"") {
                Ok((ws_tx, ws_rx))
            } else {
                Err(EstablishError::Protocol(txt))
            }
        }
        Ok(Some(Ok(msg))) => Err(EstablishError::Protocol(format!("unexpected frame: {msg:?}"))),
        Ok(Some(Err(e))) => Err(EstablishError::Network(e.to_string())),
        Ok(None) => Err(EstablishError::Network("ws closed immediately".into())),
        Err(_) => Err(EstablishError::Timeout),
    }
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
        assert!(!allowlist_match("evil-openai.com", &allowlist));
        assert!(!allowlist_match("google.com", &allowlist));
    }
}
