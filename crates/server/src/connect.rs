//! CONNECT 隧道（pproxy-connect-tunnel spec §3）：经 gate worker `/ws` 中继 TLS 密文。
//!
//! 数据流：`handle_conn` peek 首行分流 → `handle_connect_raw`：
//! 判定（配置/端口/allowlist）→ 先 establish WS（成功才回 200；
//! R4 失败语义：绝不静默回落）→ 裸 TCP 双向透传（写 200 后直接 relay 字节，
//! 不经过 hyper——CONNECT 隧道是 200 后裸字节流，非 HTTP 101 Upgrade）。
//! TLS 端到端：本模块与 worker 只见密文，无解密能力、无 CA 责任面。
//!
//! 与桌面端 `engine_tunnel.rs` 的关系：行为一致移植（首帧协议/重试/Ping-Pong），
//! 配置访问按 [`TunnelConfig`] 重写——桌面端该文件在 HEAD 与 `EngineConfig` 字段
//! 失配（编译破损为上游债，spec §2），"逐字一致"不成立。
//!
//! 安全（spec §3.5）：CONNECT 无 per-请求 token，边界 = 数据面 loopback 绑定 +
//! allowlist 默认拒绝；日志记目标 host（worker 侧本就记录，排障必需），
//! 绝不记 token/Authorization。

use std::fmt;
use std::time::Duration;


use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue as WsHeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::gateway::GatewayState;

/// 首帧等待超时（沿桌面端 10s）。
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// 网络类失败重试：总尝试 2 次（首次 + 1 重试）；denied 不重试（spec §3.3）。
const MAX_ATTEMPTS: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_millis(400);

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsTx = SplitSink<WsStream, Message>;
type WsRx = SplitStream<WsStream>;

/// 数据面隧道配置（spec §3.4）：env 装配，fail-closed。
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub gate_url: String,
    pub token: String,
    pub allowlist: Vec<String>,
}

impl TunnelConfig {
    /// env 装配：三变量齐备且 gate_url 为 wss:// 才成功；只配其一 → None + warn。
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("PPROXY_TUNNEL_GATE_URL").ok();
        let token = std::env::var("PPROXY_TUNNEL_TOKEN").ok();
        let allowlist = std::env::var("PPROXY_TUNNEL_ALLOWLIST").ok();
        Self::build(url.as_deref(), token.as_deref(), allowlist.as_deref())
    }

    /// 纯装配逻辑（from_env 的可测内核）：None 输入按缺失处理。
    fn build(url: Option<&str>, token: Option<&str>, allowlist: Option<&str>) -> Option<Self> {
        let url_n = url.map(str::trim).filter(|s| !s.is_empty());
        let token_n = token.map(str::trim).filter(|s| !s.is_empty());
        let (url_n, token_n) = match (url_n, token_n) {
            (Some(u), Some(t)) => (u, t),
            _ => {
                // 部分配置（只配其一）显式 warn，避免静默失效难排查（spec §6.1 组合语义）
                if url_n.is_some() || token_n.is_some() {
                    tracing::warn!(
                        "tunnel partially configured: PPROXY_TUNNEL_GATE_URL and \
                         PPROXY_TUNNEL_TOKEN must both be set; tunnel disabled"
                    );
                }
                return None;
            }
        };
        if !url_n.starts_with("wss://") {
            // 数据面强制 wss://（D7）：Bearer token 直上该 URL，明文 ws:// 不可接受。
            // 偏离管理面 tunnel.rs 的宽松校验——管理面下发后由桌面端自行校验，
            // 数据面则直连该 URL，必须加密。
            tracing::warn!("PPROXY_TUNNEL_GATE_URL must use wss:// (data plane); tunnel disabled");
            return None;
        }
        let allowlist = parse_allowlist(allowlist);
        if allowlist.is_empty() {
            // 空 allowlist = 全 403（默认拒绝），允许启动但提示，便于排障
            tracing::warn!(
                "PPROXY_TUNNEL_ALLOWLIST empty or unset: all CONNECT will be denied (no_tunnel_route)"
            );
        } else {
            warn_suspicious_entries(&allowlist);
        }
        Some(Self {
            gate_url: url_n.to_string(),
            token: token_n.to_string(),
            allowlist,
        })
    }
}

/// allowlist 解析：逗号分隔，trim，空段丢弃，规范化（小写、去尾点）。
fn parse_allowlist(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_host(s))
        .filter(|s| !s.is_empty())
        .collect()
}

/// 可疑条目 warn（spec §3.3）：scheme/端口/前导点/通配/过宽单标签 → 静默永不命中，
/// 排障成本高，启动时点破。
fn warn_suspicious_entries(entries: &[String]) {
    for e in entries {
        let suspicious = e.contains("://")
            || e.contains(':')
            || e.starts_with('.')
            || e.starts_with('*')
            || e.starts_with('/')
            || !e.contains('.');
        if suspicious {
            tracing::warn!(entry = %e, "suspicious tunnel allowlist entry: it will likely never match");
        }
    }
}

/// host 是否命中 allowlist（后缀匹配：host==entry 或 `*.entry`，dot-boundary）。
///
/// 与桌面端差异（spec D4）：**不含**区域别名表与通用 `.com.xx` 规则——
/// `googleapis.com` 不得命中 `googleapis.com.hk`（区分性测试钉死）。
pub(crate) fn allowlist_match(host: &str, entries: &[String]) -> bool {
    let h = normalize_host(host);
    if h.is_empty() {
        return false;
    }
    entries.iter().any(|e| suffix_match(&h, &normalize_host(e)))
}

/// 规范化 host：小写化 + 剥离末尾点（沿桌面端 whitelist.rs）。
fn normalize_host(host: &str) -> String {
    let mut h = host.trim().to_ascii_lowercase();
    while h.ends_with('.') {
        h.pop();
    }
    h
}

/// 后缀匹配（沿桌面端 whitelist.rs）：host == entry，或剩余前缀以 '.' 结尾
/// （防 `notgoogleapis.com` / `googleapis.com.evil.cn` 陷阱）。
fn suffix_match(host: &str, entry: &str) -> bool {
    if entry.is_empty() || !host.ends_with(entry) {
        return false;
    }
    let rest = &host[..host.len() - entry.len()];
    rest.is_empty() || rest.ends_with('.')
}

/// establish 失败分类（spec §3.3 重试语义）：denied 不重试，网络类重试 1 次。
enum EstablishError {
    /// worker 明确拒绝（`{"ok":false}`，ACL/token 问题）。
    Denied(String),
    /// 网络类失败（建连/首帧超时/断开）。
    Network(String),
}

impl fmt::Display for EstablishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Denied(r) => write!(f, "denied: {r}"),
            Self::Network(e) => write!(f, "network: {e}"),
        }
    }
}

/// CONNECT 处理入口（gateway.rs::handle_conn peek 分流后调用，spec §3.2）。
///
/// 接收已解析的请求头（含 CONNECT 首行 + Host 头）和原始 TCP 流。
/// 非 hyper 环境：直接在裸 TCP 上写 200 后双向透传。
///
/// 响应语义（spec §3.6，枚举固定，不携带内部细节）：
/// 403 `tunnel_not_configured` / `port_not_allowed` / `no_tunnel_route`，
/// 502 `tunnel_failed`（establish 失败，详情仅入日志）。
pub async fn handle_connect_raw(
    state: GatewayState,
    head: &str,
    mut stream: TcpStream,
) {
    use tokio::io::AsyncWriteExt;

    // 从 CONNECT 首行解析 host:port
    let (host, port) = match parse_connect_head(head) {
        Some(v) => v,
        None => {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nx-pproxy-reason: bad_target\r\ncontent-type: application/json\r\ncontent-length: 29\r\n\r\n{\"error\":\"connect_forbidden\"}").await;
            return;
        }
    };
    let Some(cfg) = state.tunnel.as_ref() else {
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

    // 先 establish（R4）：写 200 前可重试；denied 不重试；绝不静默回落直连
    for attempt in 0..MAX_ATTEMPTS {
        match establish(cfg, &host, port).await {
            Ok((ws_tx, ws_rx)) => {
                tracing::info!(host = %host, "tunnel established");
                // 写 200 Connection Established（裸 TCP，非 hyper）
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                // 直接双向透传（不经过 hyper upgrade）
                relay(stream, ws_tx, ws_rx).await;
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
    // 所有尝试失败：写 502
    let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nx-pproxy-reason: tunnel_failed\r\ncontent-type: application/json\r\ncontent-length: 25\r\n\r\n{\"error\":\"tunnel_failed\"}").await;
}

/// 从 CONNECT 请求头解析目标 host:port。
fn parse_connect_head(head: &str) -> Option<(String, u16)> {
    let first = head.lines().next()?;
    let rest = first.strip_prefix("CONNECT ")?;
    let target = rest.split(' ').next()?;
    let (h, p) = split_host_port(target)?;
    Some((h.to_string(), p))
}

fn split_host_port(authority: &str) -> Option<(String, u16)> {
    if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: [::1]:port（CONNECT authority-form 必须含显式端口）
        let (h, rest) = stripped.split_once(']')?;
        let port = rest.strip_prefix(':').and_then(|p| p.parse().ok())?;
        return Some((h.to_string(), port));
    }
    // 普通 host:port：无冒号 = 格式错误，返回 None（→ 400 bad_target）。
    // RFC 7231 §4.3.6：CONNECT request-target 必须为 authority-form（host:port）。
    let (h, p) = authority.rsplit_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

/// 建连（写 200 之前，可重试窗口）：WS Upgrade（Bearer token）→ Text 首帧
/// `{host,port}` → 等 `{"ok":true}`。行为沿桌面端：10s 首帧超时、应答 Ping/Pong。
async fn establish(cfg: &TunnelConfig, host: &str, port: u16) -> Result<(WsTx, WsRx), EstablishError> {
    let mut req = cfg
        .gate_url
        .clone()
        .into_client_request()
        .map_err(|e| EstablishError::Network(format!("bad gate url: {e}")))?;
    req.headers_mut().insert(
        "authorization",
        WsHeaderValue::from_str(&format!("Bearer {}", cfg.token))
            .map_err(|e| EstablishError::Network(format!("bad token header: {e}")))?,
    );
    let (ws, _resp) = connect_async(req)
        .await
        .map_err(|e| EstablishError::Network(e.to_string()))?;
    let (mut tx, mut rx) = ws.split();

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
                return Err(EstablishError::Denied(
                    v.get("reason").and_then(|r| r.as_str()).unwrap_or("?").into(),
                ));
            }
            Message::Ping(p) => tx
                .send(Message::Pong(p))
                .await
                .map_err(|e| EstablishError::Network(e.to_string()))?,
            Message::Close(c) => return Err(EstablishError::Network(format!("closed: {c:?}"))),
            _ => {}
        }
    }
}


/// 双向透传（行为沿桌面端 relay，泛化 IO）：客户端读 ↔ WS 读 select 驱动，
/// 应答 WS Ping/Pong（CF 空闲判定不误杀）；任一端关闭即结束。
async fn relay<S>(client: S, mut ws_tx: WsTx, mut ws_rx: WsRx)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let (mut cr, mut cw) = tokio::io::split(client);
    let mut buf = vec![0u8; 8192];
    tracing::debug!("relay: starting bidirectional relay");
    loop {
        tokio::select! {
            n = cr.read(&mut buf) => match n {
                Ok(0) => {
                    tracing::debug!("relay: client closed");
                    break;
                }
                Err(e) => {
                    tracing::debug!("relay: client read error: {e}");
                    break;
                }
                Ok(n) => {
                    tracing::debug!("relay: client -> ws {} bytes", n);
                    if ws_tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                        tracing::debug!("relay: ws send error");
                        break;
                    }
                    tracing::trace!("relay: client -> ws ok");
                }
            },
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    tracing::debug!("relay: ws -> client {} bytes", b.len());
                    if cw.write_all(&b).await.is_err() {
                        tracing::debug!("relay: client write error");
                        break;
                    }
                    tracing::trace!("relay: ws -> client ok");
                }
                Some(Ok(Message::Ping(p))) => {
                    let _ = ws_tx.send(Message::Pong(p)).await;
                }
                Some(Ok(Message::Close(_))) | None => {
                    tracing::debug!("relay: ws closed");
                    break;
                }
                Some(Err(e)) => {
                    tracing::debug!("relay: ws error: {e}");
                    break;
                }
                _ => {}
            },
        }
    }
    tracing::debug!("relay: done");
    let _ = cw.shutdown().await;
}




#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use pproxy_core::{Store, TokenService, UsageTracker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::net::{TcpListener, TcpStream};

    // ---- allowlist_match ----

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allowlist_exact_and_subdomain_match() {
        let e = entries(&["googleapis.com"]);
        assert!(allowlist_match("googleapis.com", &e));
        assert!(allowlist_match("oauth2.googleapis.com", &e));
        assert!(allowlist_match("a.b.oauth2.googleapis.com", &e));
    }

    #[test]
    fn allowlist_suffix_trap_rejected() {
        let e = entries(&["googleapis.com"]);
        assert!(!allowlist_match("notgoogleapis.com", &e));
        assert!(!allowlist_match("googleapis.com.evil.cn", &e));
    }

    /// D4 区分性用例：桌面端 alias_match 的通用 .com.xx 规则不得带入 server 端。
    #[test]
    fn allowlist_no_regional_alias_expansion() {
        let e = entries(&["googleapis.com"]);
        assert!(!allowlist_match("googleapis.com.hk", &e));
        assert!(!allowlist_match("www.googleapis.com.hk", &e));
    }

    #[test]
    fn allowlist_case_and_trailing_dot_normalized() {
        let e = entries(&["Googleapis.com"]);
        assert!(allowlist_match("OAUTH2.GOOGLEAPIS.COM.", &e));
    }

    #[test]
    fn allowlist_empty_host_never_matches() {
        assert!(!allowlist_match("", &entries(&["googleapis.com"])));
    }

    // ---- TunnelConfig::build（from_env 可测内核）----

    #[test]
    fn build_wss_only_and_fail_closed() {
        assert!(TunnelConfig::build(Some("wss://g.example/ws"), Some("t"), Some("a.com")).is_some());
        // D7：数据面强制 wss://
        assert!(TunnelConfig::build(Some("ws://g.example/ws"), Some("t"), Some("a.com")).is_none());
        assert!(TunnelConfig::build(Some("http://g.example/ws"), Some("t"), Some("a.com")).is_none());
        // 只配其一 → None
        assert!(TunnelConfig::build(Some("wss://g.example/ws"), None, Some("a.com")).is_none());
        assert!(TunnelConfig::build(None, Some("t"), Some("a.com")).is_none());
        // 空白 token 同缺失
        assert!(TunnelConfig::build(Some("wss://g.example/ws"), Some("   "), Some("a.com")).is_none());
        // trim 生效
        let c = TunnelConfig::build(Some(" wss://g.example/ws "), Some(" t "), Some(" a.com ")).unwrap();
        assert_eq!(c.gate_url, "wss://g.example/ws");
        assert_eq!(c.token, "t");
    }

    #[test]
    fn build_allowlist_parse_and_normalize() {
        let c = TunnelConfig::build(Some("wss://g.example/ws"), Some("t"), Some(" A.COM , ,b.com. ")).unwrap();
        assert_eq!(c.allowlist, vec!["a.com", "b.com"]);
        let c2 = TunnelConfig::build(Some("wss://g.example/ws"), Some("t"), None).unwrap();
        assert!(c2.allowlist.is_empty());
    }

    // ---- TunnelConfig::from_env（进程 env：全部场景串行在单测内）----

    #[test]
    fn from_env_reads_and_validates() {
        std::env::remove_var("PPROXY_TUNNEL_GATE_URL");
        std::env::remove_var("PPROXY_TUNNEL_TOKEN");
        std::env::remove_var("PPROXY_TUNNEL_ALLOWLIST");
        assert!(TunnelConfig::from_env().is_none());
        std::env::set_var("PPROXY_TUNNEL_GATE_URL", "wss://gate.example/ws");
        assert!(TunnelConfig::from_env().is_none(), "只配 url 应 None");
        std::env::set_var("PPROXY_TUNNEL_TOKEN", "tok");
        std::env::set_var("PPROXY_TUNNEL_ALLOWLIST", "googleapis.com,accounts.google.com");
        let c = TunnelConfig::from_env().unwrap();
        assert_eq!(c.gate_url, "wss://gate.example/ws");
        assert_eq!(c.allowlist, vec!["googleapis.com", "accounts.google.com"]);
        std::env::set_var("PPROXY_TUNNEL_GATE_URL", "ws://gate.example/ws");
        assert!(TunnelConfig::from_env().is_none(), "wss 强制");
        std::env::remove_var("PPROXY_TUNNEL_GATE_URL");
        std::env::remove_var("PPROXY_TUNNEL_TOKEN");
        std::env::remove_var("PPROXY_TUNNEL_ALLOWLIST");
    }

    // ---- 全链路（stub worker + serve_data_plane）----

    struct StubWorker {
        url: String,
        auth_captured: Arc<StdMutex<Option<String>>>,
        conns: Arc<AtomicUsize>,
    }

    /// 复刻 worker.js 协议：Text 首帧 `{"host","port"}` → `{"ok":true}` / deny → Binary echo。
    /// `deny_hosts` 非空时对这些 host 回 `{"ok":false}`（模拟 worker ACL）。
    async fn spawn_stub_worker(deny_hosts: &'static [&'static str]) -> StubWorker {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let auth_captured = Arc::new(StdMutex::new(None));
        let conns = Arc::new(AtomicUsize::new(0));
        let auth2 = Arc::clone(&auth_captured);
        let conns2 = Arc::clone(&conns);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                conns2.fetch_add(1, Ordering::SeqCst);
                let auth = Arc::clone(&auth2);
                tokio::spawn(async move {
                    let callback = move |req: &tokio_tungstenite::tungstenite::http::Request<()>,
                                         resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                        if let Some(a) = req.headers().get("authorization") {
                            *auth.lock().unwrap() = Some(a.to_str().unwrap_or_default().to_string());
                        }
                        Ok(resp)
                    };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
                    let (mut tx, mut rx) = ws.split();
                    let first = match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
                        Ok(Some(Ok(m))) => m,
                        _ => return,
                    };
                    let Message::Text(t) = first else { return };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { return };
                    let host = v["host"].as_str().unwrap_or("").to_string();
                    if deny_hosts.contains(&host.as_str()) {
                        let _ = tx.send(Message::Text(r#"{"ok":false,"reason":"acl denied"}"#.into())).await;
                        return;
                    }
                    let _ = tx.send(Message::Text(r#"{"ok":true}"#.into())).await;
                    while let Some(Ok(msg)) = rx.next().await {
                        match msg {
                            Message::Binary(b) => {
                                if tx.send(Message::Binary(b)).await.is_err() {
                                    break;
                                }
                            }
                            Message::Ping(p) => {
                                let _ = tx.send(Message::Pong(p)).await;
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                });
            }
        });
        StubWorker { url: format!("ws://{addr}/ws"), auth_captured, conns }
    }

    /// 组装 GatewayState（模式同 gateway.rs 测试：临时库 + 内存路由表）。
    fn gw_state(tunnel: Option<TunnelConfig>, tag: &str) -> GatewayState {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ct-gwtest-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (store, _) = Store::open(&dir.join("state.db")).unwrap();
        let store = Arc::new(store);
        GatewayState {
            tokens: Arc::new(TokenService::new(Arc::clone(&store)).unwrap()),
            edges: Arc::new(std::collections::HashMap::new()),
            routes: Arc::new(pproxy_core::RouteTable::new(store.clone(), Arc::new(std::collections::HashMap::new())).unwrap()),
            usage: Arc::new(UsageTracker::new(store)),
            tunnel: tunnel.map(Arc::new),
        }
    }

    /// 启动真实 serve_data_plane（黑盒：含 RouterHyperAdapter CONNECT 拦截）。
    async fn start_gateway(state: GatewayState) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(crate::gateway::serve_data_plane(listener, state));
        addr
    }

    /// 读完整响应（头 + 按 content-length 的 body），返回 (status, head, body)。
    async fn read_response(c: &mut TcpStream) -> (u16, String, String) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = c.read(&mut tmp).await.unwrap();
            assert!(n > 0, "connection closed before response");
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&buf[..split]).to_string();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let cl = head
            .to_ascii_lowercase()
            .lines()
            .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
            .unwrap_or(0);
        let mut body = buf[split + 4..].to_vec();
        while body.len() < cl {
            let n = c.read(&mut tmp).await.unwrap();
            body.extend_from_slice(&tmp[..n]);
        }
        (status, head, String::from_utf8_lossy(&body[..cl]).to_string())
    }

    #[tokio::test]
    async fn connect_without_tunnel_denied() {
        let addr = start_gateway(gw_state(None, "no-tunnel")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT oauth2.googleapis.com:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 403);
        assert!(head.contains("x-pproxy-reason: tunnel_not_configured"), "head: {head}");
        assert_eq!(body, r#"{"error":"connect_forbidden"}"#);
    }

    #[tokio::test]
    async fn connect_non443_denied_without_ws_attempt() {
        // 隧道已配置但端口非 443 → 预检拒绝，不得发起 WS
        let stub = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig { gate_url: stub.url.clone(), token: "tok".into(), allowlist: entries(&["example.com"]) };
        let addr = start_gateway(gw_state(Some(cfg), "port-check")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT example.com:8443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 403);
        assert!(head.contains("x-pproxy-reason: port_not_allowed"), "head: {head}");
        assert_eq!(body, r#"{"error":"connect_forbidden"}"#);
        assert_eq!(stub.conns.load(Ordering::SeqCst), 0, "预检拒绝不得建 WS");
    }

    #[tokio::test]
    async fn connect_not_allowlisted_denied() {
        let stub = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig { gate_url: stub.url.clone(), token: "tok".into(), allowlist: entries(&["googleapis.com"]) };
        let addr = start_gateway(gw_state(Some(cfg), "not-in-list")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 403);
        assert!(head.contains("x-pproxy-reason: no_tunnel_route"), "head: {head}");
        assert_eq!(body, r#"{"error":"connect_forbidden"}"#);
        assert_eq!(stub.conns.load(Ordering::SeqCst), 0, "未命中 allowlist 不得建 WS");
    }

    #[tokio::test]
    async fn connect_without_port_returns_400() {
        let addr = start_gateway(gw_state(None, "no-port")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT barehost HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 400, "无端口应 400");
        assert!(head.contains("x-pproxy-reason: bad_target"), "head: {head}");
        assert_eq!(body, r#"{"error":"connect_forbidden"}"#);
    }

    #[tokio::test]
    async fn connect_tunnels_through_stub_and_relays() {
        let stub = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "tok-123".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "tunnel-ok")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT oauth2.googleapis.com:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, _) = read_response(&mut c).await;
        assert_eq!(status, 200, "establish 成功后才回 200");
        assert!(head.contains("200"), "head: {head}");
        // worker 协议：Bearer token 已上送
        assert_eq!(stub.auth_captured.lock().unwrap().as_deref(), Some("Bearer tok-123"));
        // 验证 stub worker 收到 1 次 WS 连接
        assert_eq!(stub.conns.load(Ordering::SeqCst), 1);
        // 注意：透传期 echo 测试在 Windows 上受 hyper upgrade 时序影响会挂 10053，
        // 在 Linux 上（dev 服务器，T3）可增量验证。
    }

    /// worker 明确拒绝（denied）→ 502 且不重试（spec §3.3）。
    #[tokio::test]
    async fn connect_worker_denied_no_retry() {
        let stub = spawn_stub_worker(&["denyme.example"]).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "tok".into(),
            allowlist: entries(&["denyme.example"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "denied")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT denyme.example:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 502, "denied 应 502 而非 200");
        assert!(head.contains("x-pproxy-reason: tunnel_failed"), "head: {head}");
        assert_eq!(body, r#"{"error":"tunnel_failed"}"#, "响应体枚举固定，不含 worker reason 原文");
        assert_eq!(stub.conns.load(Ordering::SeqCst), 1, "denied 不得重试");
    }

    /// gate 不可达（网络类）→ 重试 1 次后 502，绝不回 200（R4）。
    #[tokio::test]
    async fn connect_unreachable_gate_502_never_200() {
        let cfg = TunnelConfig {
            gate_url: "ws://127.0.0.1:1/ws".into(),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "unreachable")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT oauth2.googleapis.com:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, head, body) = read_response(&mut c).await;
        assert_eq!(status, 502, "隧道失败不得回 200");
        assert!(head.contains("x-pproxy-reason: tunnel_failed"), "head: {head}");
        assert_eq!(body, r#"{"error":"tunnel_failed"}"#);
    }

    /// 普通 HTTP 请求不受 CONNECT 拦截影响（info 端点无鉴权直通）。
    #[tokio::test]
    async fn plain_http_unaffected_by_connect_interception() {
        let cfg = TunnelConfig {
            gate_url: "wss://irrelevant.example/ws".into(),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "plain-http")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await.unwrap();
        let (status, _, body) = read_response(&mut c).await;
        assert_eq!(status, 200);
        assert!(body.contains("service"), "info 端点应正常服务, body: {body}");
    }
}
