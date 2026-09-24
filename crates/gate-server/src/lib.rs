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
    routing::{any, get},
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
        .route("/proxy", any(handle_proxy))
        .route("/api/proxy", any(handle_proxy))
        .route("/", get(handle_root))
        .with_state(cfg)
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

    let expected_hash = cfg.tunnel_token_hash.trim().to_ascii_lowercase();
    if expected_hash.is_empty() || presented.is_empty() || sha256_hex(presented) != expected_hash {
        tracing::warn!("Unauthorized WS upgrade attempt");
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized").into_response();
    }

    ws.on_upgrade(move |socket| handle_ws_socket(socket))
}

async fn handle_ws_socket(socket: WebSocket) {
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

    // 4. Bi-directional relay between WS binary frames and raw TCP stream
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    let mut ws_to_tcp = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(bin)) => {
                    if tcp_write.write_all(&bin).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Ping(_)) => {
                    // Handled automatically by axum ws, but counts as activity
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        let _ = tcp_write.shutdown().await;
    });

    let mut tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 16384];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if ws_sender.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = ws_sender.close().await;
    });

    // Wait until either direction finishes
    tokio::select! {
        _ = &mut ws_to_tcp => {},
        _ = &mut tcp_to_ws => {},
    }
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

    let req_strip: HashSet<&'static str> = [
        "host", "connection", "keep-alive", "transfer-encoding", "upgrade",
        "content-length", "x-proxy-secret", "x-forwarded-for", "x-forwarded-proto",
        "x-forwarded-host", "x-real-ip", "forwarded", "via", "true-client-ip",
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
    });

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Native Gate Server listening on {}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(Into::into)
}
