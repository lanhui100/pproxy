//! Native Rust Gate Server for pproxy egress.
//!
//! Provides:
//! 1. WS-to-TCP tunnel bridge (at `/ws` and `/api/ws`) compatible with CF/Node Gate Worker:
//!    - Bearer token SHA-256 validation against `TUNNEL_TOKEN_HASH`
//!    - First frame: `{"host": "...", "port": 443}` -> ACL validation -> `{"ok": true}`
//!    - Bi-directional raw TCP stream relay with idle timeout
//! 2. HTTP/SSE reverse proxy (at `/api/proxy` and `/proxy`):
//!    - Header `x-proxy-secret` validation against `PROXY_SECRET`
//!    - Streaming fetch to `?url=...` with full SSE & chunked transfer preservation
//! 3. Diagnostics (`/debug`):
//!    - Reports token hash presence & length for alignment check

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, HeaderName, Request, Response, StatusCode},
    response::IntoResponse,
    routing::{any, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Clone)]
pub struct ServerConfig {
    pub tunnel_token_hash: String,
    pub proxy_secret: String,
    pub client: reqwest::Client,
    pub verifier: Option<Arc<pproxy_core::TokenVerifier>>,
    pub user_active_conns: Arc<dashmap::DashMap<String, usize>>,
    pub user_used_bytes: Arc<dashmap::DashMap<String, std::sync::atomic::AtomicU64>>,
    pub revoked_tokens: Arc<dashmap::DashSet<String>>, // 黑名单撤销表（存储已废止的 jti 或 sub）
    pub gate_admin_token: String, // 管理端点鉴权（POST /api/revoke 等），来源于 ENV GATE_ADMIN_TOKEN
}

/// 从 `~/.pony/revoked_tokens.txt` 加载既有撤销条目（幂等；文件缺失视为空）
pub fn load_revoked_tokens(map: &Arc<dashmap::DashSet<String>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let path = std::path::Path::new(&home).join(".pony").join("revoked_tokens.txt");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut n = 0;
    for line in content.lines() {
        let id = line.trim();
        if !id.is_empty() {
            map.insert(id.to_string());
            n += 1;
        }
    }
    tracing::info!(loaded = n, path = %path.display(), "revoked tokens loaded from disk");
}

pub fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn is_valid_host(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    let lower = h.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return false;
    }
    if lower.starts_with("10.")
        || lower.starts_with("127.")
        || lower.starts_with("169.254.")
        || lower.starts_with("192.168.")
        || lower.starts_with("0177.")
        || lower.starts_with("0x7f.")
        || lower == "::1"
        || lower == "[::1]"
    {
        return false;
    }
    if let Some(rest) = lower.strip_prefix("172.") {
        if let Some((second, _)) = rest.split_once('.') {
            if let Ok(num) = second.parse::<u8>() {
                if (16..=31).contains(&num) {
                    return false;
                }
            }
        }
    }
    true
}

#[derive(Deserialize)]
struct GateFirstFrame {
    host: String,
    port: u16,
}

pub fn build_router(cfg: Arc<ServerConfig>) -> Router {
    Router::new()
        .route("/debug", get(handle_debug))
        .route("/ws", get(handle_ws))
        .route("/api/ws", get(handle_ws))
        .route("/api/user/profile", get(handle_user_profile))
        .route("/api/user/revoke", post(handle_revoke))
        .route("/proxy", any(handle_proxy))
        .route("/api/proxy", any(handle_proxy))
        .route("/", get(handle_root))
        .with_state(cfg)
}

/// 管理端点：热更新撤销列表（受 `GATE_ADMIN_TOKEN` Bearer 鉴权）。
/// Body: `{"identifier": "usr_carol" | "usr_live_...jti" }`
async fn handle_revoke(
    State(cfg): State<Arc<ServerConfig>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // 管理端点鉴权（固定时间比较防时序侧信道）
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let expected = cfg.gate_admin_token.trim();
    if expected.is_empty()
        || presented.len() != expected.len()
        || !pproxy_core::user::constant_time_eq(presented.as_bytes(), expected.as_bytes())
    {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    #[derive(serde::Deserialize)]
    struct RevokeReq {
        identifier: String,
    }

    let req: RevokeReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad_request: expect {\"identifier\": \"...\"}").into_response(),
    };

    let identifier = req.identifier.trim().to_string();
    if identifier.is_empty() {
        return (StatusCode::BAD_REQUEST, "bad_request: empty identifier").into_response();
    }

    cfg.revoked_tokens.insert(identifier.clone());

    // 同步追加到本地撤销文件（幂等：文件锁 + append）
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let dir = std::path::Path::new(&home).join(".pony");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("revoked_tokens.txt")) {
        use std::io::Write;
        let _ = writeln!(f, "{identifier}");
    }

    tracing::warn!(identifier = %identifier, "token revoked via admin endpoint");
    (StatusCode::OK, serde_json::json!({ "ok": true, "identifier": identifier }).to_string()).into_response()
}

async fn handle_user_profile(
    State(cfg): State<Arc<ServerConfig>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = if let Some(stripped) = auth.strip_prefix("Bearer ") {
        stripped.trim()
    } else {
        ""
    };

    if !presented.starts_with("usr_live_") {
        return (StatusCode::UNAUTHORIZED, "Unauthorized: missing user token").into_response();
    }

    if let Some(ref verifier) = cfg.verifier {
        match verifier.verify_token(presented) {
            Ok(claims) => {
                if cfg.revoked_tokens.contains(&claims.jti) || cfg.revoked_tokens.contains(&claims.sub) {
                    return (StatusCode::UNAUTHORIZED, "Unauthorized: Token Revoked").into_response();
                }
                let used = cfg.user_used_bytes
                    .get(&claims.sub)
                    .map(|v| v.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(0);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let days_left = if claims.exp > now {
                    (claims.exp - now) / 86400
                } else {
                    0
                };
                let resp = serde_json::json!({
                    "code": 0,
                    "user": {
                        "sub": claims.sub,
                        "name": claims.name,
                        "status": if used >= claims.quota_bytes { "quota_exceeded" } else { "active" },
                        "quota_bytes": claims.quota_bytes,
                        "used_bytes": used,
                        "expire_at": claims.exp,
                        "days_left": days_left,
                        "max_conns": claims.max_conns
                    }
                });
                (StatusCode::OK, [("content-type", "application/json")], resp.to_string()).into_response()
            }
            Err(e) => (StatusCode::UNAUTHORIZED, format!("Unauthorized: {e}")).into_response(),
        }
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "Server verifier not configured").into_response()
    }
}

async fn handle_root() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        "Pony Gate Native Rust is running.\n",
    )
}

async fn handle_debug(State(cfg): State<Arc<ServerConfig>>) -> impl IntoResponse {
    let set = !cfg.tunnel_token_hash.is_empty();
    let len = cfg.tunnel_token_hash.trim().len();
    let body = serde_json::json!({
        "set": set,
        "len": len,
        "server": "pproxy-gate-server-rust"
    });
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        body.to_string(),
    )
}

async fn handle_ws(
    State(cfg): State<Arc<ServerConfig>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = if let Some(stripped) = auth.strip_prefix("Bearer ") {
        stripped.trim()
    } else {
        ""
    };

    // 1. 判断是否为多租户 User Token (以 usr_live_ 开头)
    let claims = if presented.starts_with("usr_live_") {
        if let Some(ref verifier) = cfg.verifier {
            match verifier.verify_token(presented) {
                Ok(c) => {
                    // 检查是否已被撤销 (Revocation Check)
                    if cfg.revoked_tokens.contains(&c.jti) || cfg.revoked_tokens.contains(&c.sub) {
                        tracing::warn!(uid = %c.sub, jti = %c.jti, "User Token has been revoked -> 401");
                        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized: Token Revoked").into_response();
                    }
                    Some(c)
                }
                Err(e) => {
                    tracing::warn!("User Token verification failed: {e}");
                    return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized: Invalid User Token").into_response();
                }
            }
        } else {
            tracing::warn!("User Token presented but server has no verifying key configured");
            return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized: Verifier not configured").into_response();
        }
    } else {
        None
    };

    // 2. 如果是多租户 Token，检查租约配额与并发（安全审查加固 SEC-P1-01：原子自增预占，堵死 TOCTOU 竞态）
    if let Some(ref c) = claims {
        // 检查累计配额是否超额
        let used = cfg.user_used_bytes
            .entry(c.sub.clone())
            .or_insert_with(|| std::sync::atomic::AtomicU64::new(0))
            .load(std::sync::atomic::Ordering::Relaxed);

        if used >= c.quota_bytes {
            tracing::warn!(uid = %c.sub, used = used, quota = c.quota_bytes, "User quota exceeded -> 402");
            return (StatusCode::PAYMENT_REQUIRED, HeaderMap::new(), "Payment Required: Quota Exceeded").into_response();
        }

        // 检查并发活跃连接数（最大为 c.max_conns，默认 3）
        let max_conns = c.max_conns;
        let mut conns_entry = cfg.user_active_conns.entry(c.sub.clone()).or_insert(0);
        if *conns_entry >= max_conns {
            tracing::warn!(uid = %c.sub, conns = *conns_entry, max = max_conns, "User max concurrent connections reached -> 429");
            return (StatusCode::TOO_MANY_REQUESTS, HeaderMap::new(), "Too Many Requests: Concurrent Connections Limit").into_response();
        }
        // 立即原子预占槽位，防止并发突发请求穿越 429 检查门禁
        *conns_entry += 1;
    } else {
        // 回退单口令兼容模式
        let expected_hash = cfg.tunnel_token_hash.trim().to_ascii_lowercase();
        if expected_hash.is_empty() || presented.is_empty() || sha256_hex(presented) != expected_hash {
            tracing::warn!("Unauthorized WS upgrade attempt");
            return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized").into_response();
        }
    }

    let cfg_clone = cfg.clone();
    ws.on_upgrade(move |socket| handle_ws_socket(socket, cfg_clone, claims))
}

async fn handle_ws_socket(
    socket: WebSocket,
    cfg: Arc<ServerConfig>,
    claims: Option<pproxy_core::UserTokenClaims>,
) {
    // 槽位已在 HTTP 阶段预占，此处构造 Guard 负责退出/中断时自动释放
    struct ConnGuard {
        sub: Option<String>,
        conns: Arc<dashmap::DashMap<String, usize>>,
    }
    impl ConnGuard {
        fn new_preoccupied(sub: Option<String>, conns: Arc<dashmap::DashMap<String, usize>>) -> Self {
            Self { sub, conns }
        }
    }
    impl Drop for ConnGuard {
        fn drop(&mut self) {
            if let Some(ref uid) = self.sub {
                if let Some(mut count) = self.conns.get_mut(uid) {
                    if *count > 0 {
                        *count -= 1;
                    }
                }
            }
        }
    }

    let _guard = ConnGuard::new_preoccupied(
        claims.as_ref().map(|c| c.sub.clone()),
        cfg.user_active_conns.clone(),
    );
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 1. Wait for first frame: {"host":"...","port":443}
    let first_msg = match tokio::time::timeout(Duration::from_secs(10), ws_receiver.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => {
            let _ = ws_sender.close().await;
            return;
        }
    };

    let target: GateFirstFrame = match serde_json::from_str(&first_msg) {
        Ok(t) => t,
        Err(_) => {
            let _ = ws_sender.close().await;
            return;
        }
    };

    if target.port != 443 || !is_valid_host(&target.host) {
        let denied = serde_json::json!({ "ok": false, "reason": "acl denied" }).to_string();
        let _ = ws_sender.send(Message::Text(denied)).await;
        let _ = ws_sender.close().await;
        return;
    }

    // 2. Connect to remote target TCP
    let addr = format!("{}:{}", target.host, target.port);
    let tcp_stream = match tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::error!(target = %addr, error = %e, "TCP connect failed");
            let err_resp = serde_json::json!({ "ok": false, "reason": "connect failed" }).to_string();
            let _ = ws_sender.send(Message::Text(err_resp)).await;
            let _ = ws_sender.close().await;
            return;
        }
        Err(_) => {
            let err_resp = serde_json::json!({ "ok": false, "reason": "connect timeout" }).to_string();
            let _ = ws_sender.send(Message::Text(err_resp)).await;
            let _ = ws_sender.close().await;
            return;
        }
    };

    // 3. Send ok acknowledgment
    let ok_resp = serde_json::json!({ "ok": true }).to_string();
    if ws_sender.send(Message::Text(ok_resp)).await.is_err() {
        return;
    }

    // 4. Bi-directional relay between WS binary frames and raw TCP stream with coordinated quota cancellation
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    let uid_up = claims.as_ref().map(|c| c.sub.clone());
    let quota_up = claims.as_ref().map(|c| c.quota_bytes);
    let bytes_map_up = cfg.user_used_bytes.clone();

    // 审查修复（P0）：使用 CancellationToken 协调双向流式熔断，任一侧超额立即通知另一侧中止
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_tx = cancel_token.clone();
    let cancel_rx = cancel_token.clone();

    let ws_to_tcp = async move {
        loop {
            tokio::select! {
                _ = cancel_tx.cancelled() => {
                    break;
                }
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Binary(bin))) => {
                            let len = bin.len() as u64;
                            if let Some(ref uid) = uid_up {
                                let total_used = if let Some(cell) = bytes_map_up.get(uid) {
                                    cell.fetch_add(len, std::sync::atomic::Ordering::Relaxed) + len
                                } else {
                                    0
                                };
                                if let Some(quota) = quota_up {
                                    if total_used >= quota {
                                        tracing::warn!(uid = %uid, "Quota exceeded in-flight (upload) -> triggering cancellation");
                                        cancel_tx.cancel();
                                        break;
                                    }
                                }
                            }
                            if tcp_write.write_all(&bin).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Ping(_))) => {}
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        _ => {}
                    }
                }
            }
        }
        let _ = tcp_write.shutdown().await;
    };

    let uid_down = claims.as_ref().map(|c| c.sub.clone());
    let quota_down = claims.as_ref().map(|c| c.quota_bytes);
    let bytes_map_down = cfg.user_used_bytes.clone();

    let tcp_to_ws = async move {
        let mut buf = vec![0u8; 16384];
        let mut exceeded = false;
        loop {
            tokio::select! {
                _ = cancel_rx.cancelled() => {
                    exceeded = true;
                    break;
                }
                read_res = tcp_read.read(&mut buf) => {
                    match read_res {
                        Ok(0) => break,
                        Ok(n) => {
                            let len = n as u64;
                            if let Some(ref uid) = uid_down {
                                let total_used = if let Some(cell) = bytes_map_down.get(uid) {
                                    cell.fetch_add(len, std::sync::atomic::Ordering::Relaxed) + len
                                } else {
                                    0
                                };
                                if let Some(quota) = quota_down {
                                    if total_used >= quota {
                                        tracing::warn!(uid = %uid, "Quota exceeded in-flight (download) -> cancelling and closing");
                                        cancel_rx.cancel();
                                        exceeded = true;
                                        break;
                                    }
                                }
                            }
                            if ws_sender.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        if exceeded {
            let _ = ws_sender.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4402,
                reason: "Quota Exceeded".into(),
            }))).await;
        }
        let _ = ws_sender.close().await;
    };

    tokio::join!(ws_to_tcp, tcp_to_ws);
}

#[derive(Deserialize)]
struct ProxyQuery {
    url: Option<String>,
}

async fn handle_proxy(
    State(cfg): State<Arc<ServerConfig>>,
    Query(query): Query<ProxyQuery>,
    req: Request<Body>,
) -> impl IntoResponse {
    let secret = cfg.proxy_secret.trim();
    if secret.is_empty() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Server misconfigured: PROXY_SECRET missing").into_response();
    }

    let client_secret = req
        .headers()
        .get("x-proxy-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if client_secret != secret {
        return (StatusCode::FORBIDDEN, "Unauthorized").into_response();
    }

    let target_str = match query.url {
        Some(u) if !u.is_empty() => u,
        _ => return (StatusCode::BAD_REQUEST, "Missing ?url=").into_response(),
    };

    let target_url = match url::Url::parse(&target_str) {
        Ok(u) if u.scheme() == "http" || u.scheme() == "https" => u,
        _ => return (StatusCode::BAD_REQUEST, "Invalid URL").into_response(),
    };

    let method = req.method().clone();
    let mut req_builder = cfg.client.request(method, target_url.clone());

    // 安全审查遵循 P2-02：物理剔除客户端代理凭据，绝对防止向目标站点泄露 Token
    let req_strip: HashSet<&'static str> = [
        "host", "connection", "keep-alive", "transfer-encoding", "upgrade",
        "content-length", "x-proxy-secret", "x-forwarded-for", "x-forwarded-proto",
        "x-forwarded-host", "x-real-ip", "forwarded", "via", "true-client-ip",
        "proxy-authorization", "x-pony-token", "x-pproxy-token",
    ]
    .into_iter()
    .collect();

    for (k, v) in req.headers() {
        let name = k.as_str().to_ascii_lowercase();
        if !req_strip.contains(name.as_str()) {
            req_builder = req_builder.header(k, v);
        }
    }

    let body_bytes = match axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Request body too large").into_response(),
    };

    if !body_bytes.is_empty() {
        req_builder = req_builder.body(body_bytes);
    }

    let upstream_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target = %target_str, error = %e, "Upstream fetch error");
            return (StatusCode::BAD_GATEWAY, format!("Upstream fetch failed: {}", e)).into_response();
        }
    };

    let status = StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let mut resp_headers = HeaderMap::new();

    let resp_strip: HashSet<&'static str> = [
        "transfer-encoding", "connection", "content-length", "content-encoding",
    ]
    .into_iter()
    .collect();

    for (k, v) in upstream_resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        if !resp_strip.contains(name.as_str()) {
            if let Ok(hname) = HeaderName::from_bytes(k.as_ref()) {
                resp_headers.insert(hname, v.clone());
            }
        }
    }
    resp_headers.insert("x-proxy-edge", "vps-rust".parse().unwrap());

    let stream = upstream_resp.bytes_stream();
    let body = Body::from_stream(stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = resp_headers;

    response.into_response()
}

pub async fn run_server(addr: SocketAddr, token_hash: String, proxy_secret: String) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .pool_max_idle_per_host(32)
        .build()?;

    let state = Arc::new(ServerConfig {
        tunnel_token_hash: token_hash,
        proxy_secret,
        client,
        verifier: None,
        user_active_conns: Arc::new(dashmap::DashMap::new()),
        user_used_bytes: Arc::new(dashmap::DashMap::new()),
        revoked_tokens: Arc::new(dashmap::DashSet::new()),
        gate_admin_token: String::new(),
    });

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Native Gate Server listening on {}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests;

