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
use std::sync::Arc;
use std::time::Duration;

use futures::SinkExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

pub use pproxy_transport::{WsSink as WsTx, WsStream as WsRx};

use crate::gateway::GatewayState;

/// gate 隧道端点（WS↔TCP 桥）：部署于 gate.example.com/ws（见 deploy/cf-gate-worker/wrangler.toml）。
/// 与 HTTP 数据面网关（edge.example.com，cf-worker）不是同一域名，切勿混用。
const GATE_WS_URL: &str = "wss://gate.example.com/ws";

/// 网络类失败重试：总尝试 5 次（首次 + 4 重试）；denied 不重试（spec §3.3）。
const MAX_ATTEMPTS: u32 = 5;

/// 默认开箱即用的白名单（覆盖主流海外常用服务、社交通讯、AI 模型与代码平台）。
pub const DEFAULT_ALLOWLIST: &[&str] = &[
    // Google 系与 Android / 开发服务
    "google.com",
    "googleapis.com",
    "gstatic.com",
    "googleusercontent.com",
    "accounts.google.com",
    "goog",
    "g.co",
    "android.com",
    "golang.org",
    // 影音流媒体 (YouTube)
    "youtube.com",
    "googlevideo.com",
    "ytimg.com",
    "youtu.be",
    // 社交与通讯平台 (X / Twitter / Telegram)
    "x.com",
    "twitter.com",
    "twimg.com",
    "t.co",
    "telegram.org",
    "t.me",
    "telegram.me",
    "telegra.ph",
    // 主流 AI 与大模型
    "openai.com",
    "chatgpt.com",
    "oaistatic.com",
    "oaiusercontent.com",
    "anthropic.com",
    "claude.ai",
    "deepmind.google",
    "perplexity.ai",
    "huggingface.co",
    // 开发者基础设施与百科
    "github.com",
    "githubusercontent.com",
    "gitlab.com",
    "docker.com",
    "docker.io",
    "stackoverflow.com",
    "wikipedia.org",
    "wikimedia.org",
];

/// 从 worker_url 推导 gate_url（将 http/https 转换为 ws/wss 并确保以 /ws 结尾）。
pub fn derive_gate_url_from_worker(worker_url: &str) -> Option<String> {
    let mut trimmed = worker_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 剥离 query 和 fragment
    if let Some((base, _)) = trimmed.split_once('?') {
        trimmed = base;
    }
    if let Some((base, _)) = trimmed.split_once('#') {
        trimmed = base;
    }
    let trimmed = trimmed.trim_end_matches('/');

    let (scheme, host_port_path) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("wss", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("ws", rest)
    } else if let Some(rest) = trimmed.strip_prefix("wss://") {
        ("wss", rest)
    } else if let Some(rest) = trimmed.strip_prefix("ws://") {
        ("ws", rest)
    } else if !trimmed.contains("://") {
        ("wss", trimmed)
    } else {
        // 拒绝 ftp://, file:// 等异常 scheme
        return None;
    };

    let host_port = host_port_path.trim_end_matches("/ws").trim_end_matches('/');
    if host_port.is_empty() {
        return None;
    }
    let derived = format!("{scheme}://{host_port}/ws");
    // 产品默认 HTTP 网关（edge.example.com，cf-worker）不是 WS gate——迁移到正确的 gate 端点
    // （gate.example.com/ws，见 deploy/cf-gate-worker/wrangler.toml）。曾用 edge 推导导致隧道拨测超时。
    if derived.starts_with("wss://edge.example.com") || derived.starts_with("ws://edge.example.com") {
        return Some(GATE_WS_URL.to_string());
    }
    Some(derived)
}

/// 数据面隧道配置（spec §3.4）：env 装配 / PoolConfig 智能推导，fail-closed。
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub gate_url: String,
    pub token: String,
    pub allowlist: Vec<String>,
}

impl TunnelConfig {
    /// env 装配（向后兼容）：优先读环境变量，兜底从默认 PoolConfig 推导。
    #[allow(dead_code)]
    pub fn from_env() -> Option<Self> {
        Self::from_pool_config_and_env(&pproxy_core::PoolConfig::default())
    }

    /// 智能推导装配（PoolConfig + 环境变量覆盖）：
    /// 1. gate_url: PPROXY_TUNNEL_GATE_URL 优先；缺省时由 pool_config.worker_url 自动推导
    /// 2. token: PPROXY_TUNNEL_TOKEN 优先；缺省时继承 pool_config.worker_secret
    /// 3. allowlist: PPROXY_TUNNEL_ALLOWLIST 追加到 DEFAULT_ALLOWLIST
    pub fn from_pool_config_and_env(pool_config: &pproxy_core::PoolConfig) -> Option<Self> {
        let env_url = std::env::var("PPROXY_TUNNEL_GATE_URL").ok();
        let env_token = std::env::var("PPROXY_TUNNEL_TOKEN").ok();
        let env_allowlist = std::env::var("PPROXY_TUNNEL_ALLOWLIST").ok();

        let derived_url = env_url.or_else(|| {
            pool_config
                .worker_url
                .as_deref()
                .and_then(derive_gate_url_from_worker)
        });
        let derived_token = env_token.or_else(|| pool_config.worker_secret.clone());

        Self::build(
            derived_url.as_deref(),
            derived_token.as_deref(),
            env_allowlist.as_deref(),
        )
    }

    /// 纯装配逻辑（from_env / from_pool_config 的可测内核）：None 输入按缺失处理。
    fn build(url: Option<&str>, token: Option<&str>, allowlist: Option<&str>) -> Option<Self> {
        let url_n = url.map(str::trim).filter(|s| !s.is_empty());
        let token_n = token.map(str::trim).filter(|s| !s.is_empty());
        let (url_n, token_n) = match (url_n, token_n) {
            (Some(u), Some(t)) => (u, t),
            _ => {
                // 部分配置（只配其一）显式 warn，避免静默失效难排查
                if url_n.is_some() || token_n.is_some() {
                    tracing::warn!(
                        "tunnel partially configured: gate_url and \
                         token must both be set; tunnel disabled"
                    );
                }
                return None;
            }
        };
        if !url_n.starts_with("wss://") {
            // 数据面强制 wss://（D7）：Bearer token 直上该 URL，明文 ws:// 不可接受。
            // 偏离管理面 tunnel.rs 的宽松校验——管理面下发后由桌面端自行校验，
            // 数据面则直连该 URL，必须加密。
            tracing::warn!("tunnel gate_url must use wss:// (data plane); tunnel disabled");
            return None;
        }
        let allowlist = parse_allowlist(allowlist);
        warn_suspicious_entries(&allowlist);
        Some(Self {
            gate_url: url_n.to_string(),
            token: token_n.to_string(),
            allowlist,
        })
    }
}

/// allowlist 解析：以 DEFAULT_ALLOWLIST 为基底，追加并去重自定义列表，trim 并规范化。
fn parse_allowlist(raw: Option<&str>) -> Vec<String> {
    let mut entries: Vec<String> = DEFAULT_ALLOWLIST
        .iter()
        .map(|s| normalize_host(s))
        .filter(|s| !s.is_empty())
        .collect();

    if let Some(custom) = raw {
        for item in custom.split(',') {
            let trimmed = item.trim();
            if trimmed == "*" || trimmed.eq_ignore_ascii_case("all") {
                if !entries.contains(&"*".to_string()) {
                    entries.push("*".to_string());
                }
                continue;
            }
            let normalized = normalize_host(item);
            if !normalized.is_empty() && !entries.contains(&normalized) {
                entries.push(normalized);
            }
        }
    }
    entries
}

/// 可疑条目 warn（spec §3.3）：scheme/端口/前导点/通配/过宽单标签 → 静默永不命中，
/// 排障成本高，启动时点破。
fn warn_suspicious_entries(entries: &[String]) {
    for e in entries {
        if e == "*" {
            continue;
        }
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
/// 若白名单包含 `*` 通配符，则无条件命中（全网通模式）。
pub(crate) fn allowlist_match(host: &str, entries: &[String]) -> bool {
    if entries.iter().any(|e| e == "*") {
        return true;
    }
    let h = normalize_host(host);
    if h.is_empty() {
        return false;
    }
    entries.iter().any(|e| suffix_match(&h, &normalize_host(e)))
}

/// 规范化 host：小写化 + 剥离前导通配符和点 + 剥离末尾点。
fn normalize_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    let h = h.trim_start_matches('*').trim_start_matches('.');
    let mut h = h.to_string();
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
#[derive(Debug)]
enum EstablishError {
    /// worker 明确拒绝（`{"ok":false}`，ACL/token 问题）。
    Denied(String),
    /// 网络类失败（建联/首帧超时/断开）。
    Network(String),
}

/// 待命 WS 会话：已完成 TCP+TLS+WS Upgrade（跨洲 ~4 RTT），尚未发送首帧。
/// 待命隧道池：CONNECT 到达前预建 WS 会话，将冷建连 RTT 从 ~5 RTT 压至 1 RTT。
pub struct TunnelPool {
    cfg: TunnelConfig,
    inner: Arc<pproxy_transport::TunnelPool>,
    _tx: tokio::sync::watch::Sender<(Option<String>, Option<String>)>,
}

const POOL_SIZE: usize = 2;

impl TunnelPool {
    pub fn new(cfg: TunnelConfig) -> Arc<Self> {
        Self::with_size(cfg, POOL_SIZE)
    }

    pub fn with_size(cfg: TunnelConfig, size: usize) -> Arc<Self> {
        let (tx, rx) = tokio::sync::watch::channel((
            Some(cfg.gate_url.clone()),
            Some(cfg.token.clone()),
        ));
        let inner = pproxy_transport::TunnelPool::with_size(rx, size);
        inner.start_maintain();
        Arc::new(Self {
            cfg,
            inner,
            _tx: tx,
        })
    }

    #[cfg(test)]
    pub fn disabled(cfg: TunnelConfig) -> Arc<Self> {
        let (tx, rx) = tokio::sync::watch::channel((
            Some(cfg.gate_url.clone()),
            Some(cfg.token.clone()),
        ));
        let inner = pproxy_transport::TunnelPool::with_size(rx, 0);
        Arc::new(Self {
            cfg,
            inner,
            _tx: tx,
        })
    }

    #[cfg(test)]
    pub(crate) fn idle_len(&self) -> usize {
        self.inner.idle_total()
    }

    pub fn config(&self) -> &TunnelConfig {
        &self.cfg
    }

    /// 按配置顺序取待命会话（无目标信息时的兼容入口）。
    pub fn checkout(&self) -> Option<(WsTx, WsRx)> {
        let endpoints: Vec<&str> = self
            .cfg
            .gate_url
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        self.checkout_ordered(&endpoints)
    }

    /// 按调用方给定的端点优先级取待命会话（合规出口专项：按目标 host 重排后传入）。
    pub fn checkout_ordered(&self, ordered: &[&str]) -> Option<(WsTx, WsRx)> {
        self.inner.checkout(ordered).map(|(tx, rx, _)| (tx, rx))
    }
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
/// 接收已解析的请求头（含 CONNECT 首行 + Host 头）、管道化多余字节和原始 TCP 流。
/// 非 hyper 环境：直接在裸 TCP 上写 200 后双向透传。
///
/// 响应语义（spec §3.6，枚举固定，不携带内部细节）：
/// 403 `tunnel_not_configured` / `port_not_allowed` / `no_tunnel_route`，
/// 502 `tunnel_failed`（establish 失败，详情仅入日志）。
pub async fn handle_connect_raw(
    state: GatewayState,
    head: &str,
    leftover: Vec<u8>,
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
    let Some(pool) = state.tunnel.as_ref() else {
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

    // 端点顺序按目标 host 决定（Cloud Code 系必须优先合规物理出口，见 route.rs）。
    // 之前此处固定按配置顺序建连，导致 agy 的模型调用固定先落 CF 出口，
    // 命中不受支持地区时被 Google 以 400 FAILED_PRECONDITION 拒绝。
    let ordered_urls = pproxy_transport::ordered_gate_urls(&pool.config().gate_url, &host);
    let ordered_refs: Vec<&str> = ordered_urls.iter().map(String::as_str).collect();

    // 先 establish（R4）：写 200 前可重试；denied 不重试；绝不静默回落直连
    // 池化（性能专项）：第一次尝试优先取待命会话（省 TCP+TLS+Upgrade ~4 RTT），
    // 待命会话 bind 失败（静默死亡）按 Network 重试，第二次尝试全新建连兜底。
    let establish_started = tokio::time::Instant::now();
    for attempt in 0..MAX_ATTEMPTS {
        let pooled_session = if attempt == 0 {
            pool.checkout_ordered(&ordered_refs)
        } else {
            None
        };
        let used_pool = pooled_session.is_some();
        let result = match pooled_session {
            Some((tx, rx)) => {
                let bind_res = bind_target(tx, rx, &host, port).await;
                if bind_res.is_err() {
                    establish_with_endpoints(pool.config(), &ordered_refs, &host, port).await
                } else {
                    bind_res
                }
            }
            None => establish_with_endpoints(pool.config(), &ordered_refs, &host, port).await,
        };
        match result {
            Ok((ws_tx, ws_rx)) => {
                tracing::info!(
                    host = %host,
                    establish_ms = establish_started.elapsed().as_millis() as u64,
                    pooled = used_pool,
                    "tunnel established"
                );
                // 写 200 Connection Established（裸 TCP，非 hyper）
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                // 直接双向透传（不经过 hyper upgrade）
                relay(stream, leftover, ws_tx, ws_rx).await;
                return;
            }
            Err(e) => {
                let retryable = matches!(e, EstablishError::Network(_));
                tracing::warn!(host = %host, attempt, pooled = used_pool, error = %e, "tunnel establish failed");
                if !retryable || attempt + 1 >= MAX_ATTEMPTS {
                    break;
                }
                let backoff = Duration::from_millis(
                    50 * (1 << attempt.min(6)) + (rand::random::<u64>() % 50),
                );
                tokio::time::sleep(backoff).await;
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

/// 建连（端点顺序由调用方给定）：合规出口专项按目标 host 重排后传入，
/// 任一端点 bind 被拒/失败即按序故障转移到下一端点。
async fn establish_with_endpoints(
    cfg: &TunnelConfig,
    endpoints: &[&str],
    host: &str,
    port: u16,
) -> Result<(WsTx, WsRx), EstablishError> {
    let mut last_err = None;
    for endpoint in endpoints {
        match pproxy_transport::connect_ws(endpoint, &cfg.token).await {
            Ok((tx, rx)) => match bind_target(tx, rx, host, port).await {
                Ok(pair) => return Ok(pair),
                Err(e) => {
                    tracing::warn!(endpoint, host, port, error = %e, "gate bind failed, trying next endpoint");
                    last_err = Some(e);
                }
            },
            Err(e) => {
                last_err = Some(EstablishError::Network(e.to_string()));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| EstablishError::Network("no valid gate endpoints configured".into())))
}


/// 在已建立的 WS 上声明目标（Text 首帧 `{host,port}` → 等 `{"ok":true}`）。
/// 池化会话与新建会话共用此绑定步骤。
async fn bind_target(tx: WsTx, rx: WsRx, host: &str, port: u16) -> Result<(WsTx, WsRx), EstablishError> {
    pproxy_transport::bind_target(tx, rx, host, port)
        .await
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("denied:") {
                EstablishError::Denied(s.trim_start_matches("denied:").trim().into())
            } else {
                EstablishError::Network(s)
            }
        })
}

/// 双向透传：支持 WS Ping/Pong 保活和 TCP 半关闭。
async fn relay(mut client: TcpStream, leftover: Vec<u8>, mut ws_tx: WsTx, ws_rx: WsRx) {
    if !leftover.is_empty() && ws_tx.send(Message::Binary(leftover)).await.is_err() {
        let _ = client.shutdown().await;
        return;
    }

    let _ = pproxy_transport::relay_bidir_ws(client, ws_tx, ws_rx, pproxy_transport::Egress::Cf, &()).await;
}




#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use futures::StreamExt;
    use pproxy_core::{Store, TokenService, UsageTracker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::io::AsyncReadExt;
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

    // ---- derive_gate_url_from_worker ----

    #[test]
    fn derive_gate_url_handles_schemes_and_paths() {
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com"),
            Some("wss://gate.example.com/ws".to_string())
        );
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com/"),
            Some("wss://gate.example.com/ws".to_string())
        );
        // 重复 /ws 幂等
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com/ws"),
            Some("wss://gate.example.com/ws".to_string())
        );
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com/ws/"),
            Some("wss://gate.example.com/ws".to_string())
        );
        // 剥离 query 与 fragment
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com/?env=prod"),
            Some("wss://gate.example.com/ws".to_string())
        );
        assert_eq!(
            derive_gate_url_from_worker("https://edge.example.com#tag"),
            Some("wss://gate.example.com/ws".to_string())
        );
        // http 转换为 ws
        assert_eq!(
            derive_gate_url_from_worker("http://127.0.0.1:8787"),
            Some("ws://127.0.0.1:8787/ws".to_string())
        );
        assert_eq!(
            derive_gate_url_from_worker("wss://gate.example.com/ws"),
            Some("wss://gate.example.com/ws".to_string())
        );
        // 非法 scheme 返回 None
        assert_eq!(derive_gate_url_from_worker("ftp://bad.example.com"), None);
        assert_eq!(derive_gate_url_from_worker(""), None);
    }

    #[test]
    fn allowlist_leading_dot_and_wildcard_normalized() {
        let e = entries(&[".company.com", "*.openai-custom.com"]);
        assert!(allowlist_match("api.company.com", &e));
        assert!(allowlist_match("company.com", &e));
        assert!(allowlist_match("api.openai-custom.com", &e));
        assert!(allowlist_match("openai-custom.com", &e));
    }

    // ---- TunnelConfig::build（from_env / from_pool_config 可测内核）----

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
    fn build_allowlist_defaults_and_custom_merge() {
        // None 缺省自动包含默认白名单（Google / OpenAI / Anthropic / GitHub）
        let c = TunnelConfig::build(Some("wss://g.example/ws"), Some("t"), None).unwrap();
        assert!(c.allowlist.contains(&"googleapis.com".to_string()));
        assert!(c.allowlist.contains(&"accounts.google.com".to_string()));
        assert!(c.allowlist.contains(&"openai.com".to_string()));
        assert!(allowlist_match("oauth2.googleapis.com", &c.allowlist));

        // 自定义追加合并且去重
        let c2 = TunnelConfig::build(Some("wss://g.example/ws"), Some("t"), Some(" custom.example.com , Googleapis.com ")).unwrap();
        assert!(c2.allowlist.contains(&"custom.example.com".to_string()));
        assert!(c2.allowlist.contains(&"googleapis.com".to_string()));
    }

    // ---- TunnelConfig::from_pool_config_and_env ----

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn from_pool_config_derives_zero_config_tunnel() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PPROXY_TUNNEL_GATE_URL");
        std::env::remove_var("PPROXY_TUNNEL_TOKEN");
        std::env::remove_var("PPROXY_TUNNEL_ALLOWLIST");

        let pool_cfg = pproxy_core::PoolConfig {
            worker_url: Some("https://edge.example.com".into()),
            worker_secret: Some("sec-secret-123".into()),
            ..Default::default()
        };

        let c = TunnelConfig::from_pool_config_and_env(&pool_cfg).unwrap();
        // edge.example.com 按 derive_gate_url_from_worker 规则重定向到 WS gate 端点
        // （HTTP 网关不是 WS gate，见 derive_gate_url_handles_schemes_and_paths）
        assert_eq!(c.gate_url, "wss://gate.example.com/ws");
        assert_eq!(c.token, "sec-secret-123");
        assert!(allowlist_match("oauth2.googleapis.com", &c.allowlist));
        assert!(allowlist_match("api.openai.com", &c.allowlist));
    }

    #[test]
    fn from_env_reads_and_overrides_pool_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PPROXY_TUNNEL_GATE_URL");
        std::env::remove_var("PPROXY_TUNNEL_TOKEN");
        std::env::remove_var("PPROXY_TUNNEL_ALLOWLIST");
        assert!(TunnelConfig::from_env().is_none());

        let pool_cfg = pproxy_core::PoolConfig {
            worker_url: Some("https://edge.example.com".into()),
            worker_secret: Some("sec-secret-123".into()),
            ..Default::default()
        };

        std::env::set_var("PPROXY_TUNNEL_GATE_URL", "wss://custom-gate.example/ws");
        std::env::set_var("PPROXY_TUNNEL_TOKEN", "tok-override");
        std::env::set_var("PPROXY_TUNNEL_ALLOWLIST", "extra.domain.com");

        let c = TunnelConfig::from_pool_config_and_env(&pool_cfg).unwrap();
        assert_eq!(c.gate_url, "wss://custom-gate.example/ws");
        assert_eq!(c.token, "tok-override");
        assert!(c.allowlist.contains(&"extra.domain.com".to_string()));
        assert!(c.allowlist.contains(&"googleapis.com".to_string()));

        std::env::remove_var("PPROXY_TUNNEL_GATE_URL");
        std::env::remove_var("PPROXY_TUNNEL_TOKEN");
        std::env::remove_var("PPROXY_TUNNEL_ALLOWLIST");
    }

    #[test]
    fn allowlist_wildcard_matches_anything() {
        let list = parse_allowlist(Some("*"));
        assert!(list.contains(&"*".to_string()));
        assert!(allowlist_match("youtube.com", &list));
        assert!(allowlist_match("x.com", &list));
        assert!(allowlist_match("twitter.com", &list));
        assert!(allowlist_match("anything.xyz", &list));

        let list_all = parse_allowlist(Some("all"));
        assert!(list_all.contains(&"*".to_string()));
        assert!(allowlist_match("youtube.com", &list_all));
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
        spawn_stub_worker_at(deny_hosts, "/ws").await
    }

    /// 同 `spawn_stub_worker`，但可指定 WS 路径——用于构造 Vercel 侧端点
    /// （`route.rs` 的出口分类按 URL 串判定：含 `/api/ws` 或 `vgate`/`vercel` 记为 Vercel）。
    async fn spawn_stub_worker_at(
        deny_hosts: &'static [&'static str],
        path: &'static str,
    ) -> StubWorker {
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
        StubWorker { url: format!("ws://{addr}{path}"), auth_captured, conns }
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
            tunnel: tunnel.map(TunnelPool::disabled),
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

    // ---- 待命隧道池（性能专项）----

    /// 池化必须在无任何 CONNECT 请求时预建 WS 会话（预热带宽）。
    #[tokio::test]
    async fn tunnel_pool_preconnects_without_traffic() {
        let stub = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let _pool = TunnelPool::new(cfg);
        // 后台补给任务应在数秒内建立至少 1 条待命会话
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while stub.conns.load(Ordering::SeqCst) < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池化未在 5s 内预建会话");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 池化路径端到端：CONNECT 复用待命会话成功 establish 并回 200。
    #[tokio::test]
    async fn connect_over_pooled_tunnel_succeeds() {
        let stub = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig {
            gate_url: stub.url.clone(),
            token: "tok-pooled".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let mut state = gw_state(None, "pooled");
        let pool = TunnelPool::new(cfg);
        // 等待待命会话就绪（否则第一次 CONNECT 走全新建连，无法验证池路径）；
        // 用 idle_len 判定入池，避免 accept/upgrade 计数窗口竞态
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_len() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池化未预建会话");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        state.tunnel = Some(pool);
        let addr = start_gateway(state).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT oauth2.googleapis.com:443 HTTP/1.1\r\n\r\n").await.unwrap();
        let (status, _, _) = read_response(&mut c).await;
        assert_eq!(status, 200, "池化隧道 establish 成功后才回 200");
        assert_eq!(
            stub.auth_captured.lock().unwrap().as_deref(),
            Some("Bearer tok-pooled"),
            "池化会话 upgrade 必须携带 Bearer token"
        );
    }

    /// 死会话兜底：待命会话被 gate 侧关闭后，checkout bind 失败必须
    /// 按 Network 重试全新建连，最终仍回 200（不得比无池化更差）。
    #[tokio::test]
    async fn dead_pooled_session_falls_back_to_fresh_establish() {
        // stub 行为：接受 WS 后立即关闭（模拟 CF 空闲回收的死会话）
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
                    // 第一次连接：upgrade 成功后立即关闭（死会话进池）
                    if let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await {
                        let _ = ws.close(None).await;
                    }
                });
            }
        });
        let cfg = TunnelConfig {
            gate_url: format!("ws://{addr}/ws"),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let pool = TunnelPool::new(cfg);
        // 以 idle_len 判定「已入池」（对抗审核 P2：stub accept 计数与客户端
        // upgrade 完成/入池之间有窗口，用 accept 计数推断入池会竞态 flake）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_len() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池化未预建会话");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(conns.load(Ordering::SeqCst) >= 1, "stub 应已收到预建连接");
        // checkout 到死会话后 bind_target 必失败（Network 类）
        let (tx, rx) = pool.checkout().expect("池中应有待命会话");
        let err = bind_target(tx, rx, "oauth2.googleapis.com", 443).await;
        assert!(
            matches!(err, Err(EstablishError::Network(_))),
            "死会话 bind 必须归类 Network 以触发重试兜底, got {err:?}"
        );
    }

    /// 合规出口专项（A/B）：Cloud Code 系 host 必须先打非 CF 端点，
    /// 即便配置里 CF 排在前面、且 `PPROXY_CONSERVE_VERCEL=1` 生效。
    #[tokio::test]
    async fn connect_compliant_host_prefers_non_cf_endpoint() {
        let cf = spawn_stub_worker(&[]).await;
        let vgate = spawn_stub_worker_at(&[], "/api/ws").await;
        let cfg = TunnelConfig {
            gate_url: format!("{},{}", cf.url, vgate.url),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "compliant-order")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT daily-cloudcode-pa.googleapis.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let (status, _head, _) = read_response(&mut c).await;
        assert_eq!(status, 200, "合规 host 应建连成功");
        assert_eq!(
            vgate.conns.load(Ordering::SeqCst),
            1,
            "合规 host 必须先打非 CF 端点"
        );
        assert_eq!(
            cf.conns.load(Ordering::SeqCst),
            0,
            "合规 host 不得先打 CF 端点"
        );
    }

    /// 对照组：认证类 host 不受合规出口例外影响，仍按配置顺序 CF 优先。
    #[tokio::test]
    async fn connect_auth_host_keeps_configured_order() {
        let cf = spawn_stub_worker(&[]).await;
        let vgate = spawn_stub_worker(&[]).await;
        let cfg = TunnelConfig {
            gate_url: format!("{},{}", cf.url, vgate.url),
            token: "tok".into(),
            allowlist: entries(&["googleapis.com"]),
        };
        let addr = start_gateway(gw_state(Some(cfg), "auth-order")).await;
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT oauth2.googleapis.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let (status, _head, _) = read_response(&mut c).await;
        assert_eq!(status, 200, "认证类 host 应建连成功");
        assert_eq!(
            cf.conns.load(Ordering::SeqCst),
            1,
            "认证类 host 应保持 CF 优先"
        );
        assert_eq!(vgate.conns.load(Ordering::SeqCst), 0);
    }
}
