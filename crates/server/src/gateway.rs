//! 数据面网关（T3）：axum 化 + 路径 token 鉴权 + 转发。
//!
//! 架构（T3 §2.2 C-P2-14 唯一路径）：
//! accept 循环 → Semaphore 并发上限 → peek 首行分流
//!   ├─ CONNECT → 403 直接写回并关闭（P0-1：无 relay 路径）
//!   └─ 其他 → hyper http1::Builder::serve_connection(数据面 Router)
//!
//! hyper↔axum 桥接：axum 0.7 `Router<()>` 实现的是 tower_service::Service，
//! hyper 1.x 需要自己的 `hyper::service::Service`，经 RouterHyperAdapter
//! 委托桥接（本 crate 无 tower 依赖，poll_ready 恒 Ready 等价转发）。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pproxy_core::{EdgeClient, ForwardRequest, TokenService};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// 全局并发连接上限（T3 §2.3 S-P1-额外）：排队即天然背压，不做主动拒绝。
pub const MAX_CONCURRENT_CONNECTIONS: usize = 256;
/// 请求 body 上限（沿旧实现）：超限 413。
pub const MAX_BODY_SIZE: usize = 32 * 1024 * 1024;

const X_PONY_TOKEN: &str = "x-pony-token";

/// 鉴权通过后经 extensions 传入转发 handler 的上下文（T3 §4.1 第 5 步）。
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: i64,
    pub route: String,
    pub path_query: String,
}

/// 数据面共享状态。
#[derive(Clone)]
pub struct GatewayState {
    pub tokens: Arc<TokenService>,
    pub edges: Arc<HashMap<String, EdgeClient>>,
    pub routes: Arc<pproxy_core::RouteTable>,
    pub usage: Arc<pproxy_core::UsageTracker>,
}

/// 组装数据面 Router（main.rs 与测试共用）。
pub fn data_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(info_endpoint))
        .route(
            "/*rest",
            get(forward_handler)
                .post(forward_handler)
                .put(forward_handler)
                .patch(forward_handler)
                .delete(forward_handler)
                .head(forward_handler)
                .options(forward_handler),
        )
        .layer(from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state)
}

/// 根路径信息端点（T3 §4.3）：无鉴权，仅服务形态，不列路由名。
async fn info_endpoint() -> Response {
    Json(serde_json::json!({
        "service": "pony-proxy",
        "version": "m1",
        "auth": "GET /{token}/{route}/{path} 或 header X-Pony-Token",
        "admin": "http://127.0.0.1:8900/api/*",
    }))
    .into_response()
}

/// 鉴权中间件（T3 §4.1）：路径模式优先，header 模式兜底；失败统一 401。
pub async fn auth_middleware(
    State(state): State<GatewayState>,
    mut req: Request,
    next: Next,
) -> Response {
    // 根路径 info 端点不鉴权（T3 §4.3）
    let path = req.uri().path().to_string();
    if path == "/" {
        return next.run(req).await;
    }

    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let rest = path.strip_prefix('/').unwrap_or(&path).to_string();

    let (token_plaintext, remaining) = split_first_segment(&rest);
    let route_path: String =
        if token_plaintext.starts_with(pproxy_core::token::TOKEN_PREFIX) {
            // 路径模式：首段为 token 段（route 名创建时已禁 pony_ 前缀，T4 §6 无歧义）。
            // 首段以 pony_ 开头但无 header 也按路径模式处理（T3 §4.1 歧义规则）。
            remaining.to_string()
        } else {
            // header 模式：首段即 route，token 取 X-Pony-Token
            match req.headers().get(X_PONY_TOKEN).and_then(|v| v.to_str().ok()) {
                Some(t) if !t.is_empty() => {
                    rest.clone()
                }
                _ => return unauthorized(),
            }
        };

    if route_path.is_empty() {
        return unauthorized();
    }
    let path_query = format!("/{route_path}{query}");

    // verify 三种失败（NotFound/Revoked/Expired）同 401 同体，防枚举（T3 §4.1 第 4 步）
    let tokens = Arc::clone(&state.tokens);
    let plaintext = token_plaintext.to_string();
    let verified = tokio::task::spawn_blocking(move || tokens.verify(&plaintext)).await;
    let row = match verified {
        Ok(Ok(row)) => row,
        Ok(Err(reason)) => {
            // 仅 info 记录原因类别，不打 token 明文（T3 §4.1）
            tracing::info!(reason = %reason, "token verify failed");
            return unauthorized();
        }
        Err(e) => {
            tracing::error!(error = %e, "verify spawn_blocking panic");
            return internal_error();
        }
    };

    // 剥离后的 {route}/{path}?{query} 放入 extensions（T3 §4.1 第 5 步）
    let route_name = route_path.split('/').next().unwrap_or("").to_string();
    if route_name.is_empty() {
        return unauthorized();
    }
    // S-P1-1：进入转发前删除 x-pony-token（路径模式下为 no-op，header 模式下必须删）
    req.headers_mut().remove(X_PONY_TOKEN);
    req.extensions_mut().insert(AuthContext {
        token_id: row.id,
        route: route_name,
        path_query,
    });
    next.run(req).await
}

/// 按首段切分：返回 (首段, 其余部分)。
fn split_first_segment(s: &str) -> (&str, &str) {
    match s.split_once('/') {
        Some((a, b)) => (a, b),
        None => (s, ""),
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"error":"unauthorized"}"#,
    )
        .into_response()
}

fn internal_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, r#"{"error":"internal"}"#).into_response()
}

/// 转发 handler（T3 §4.2）：resolve → 读 body → 计数 → 转发 → 流式透传。
async fn forward_handler(State(state): State<GatewayState>, req: Request) -> Response {
    let ctx = match req.extensions().get::<AuthContext>() {
        Some(c) => c.clone(),
        None => return unauthorized(),
    };

    // 第 1 步：剥离 x-pony-token（S-P1-1：header 模式下防明文透传上游）
    let mut headers = req.headers().clone();
    headers.remove(X_PONY_TOKEN);
    let method = req.method().clone();
    let body = req.into_body();

    // 第 2 步：resolve（UnknownRoute/Disabled 同 404 同体，防路由枚举）
    let routes = Arc::clone(&state.routes);
    let route_name = ctx.route.clone();
    let path_query = ctx.path_query.clone();
    let resolve = tokio::task::spawn_blocking(move || routes.resolve(&route_name, &path_query))
        .await;
    let (target_url, upstream) = match resolve {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => {
            // UnknownRoute / Disabled 均映射 404 {"error":"unknown_route"}（T3 §4.2 第 2 步）
            return not_found();
        }
        Err(e) => {
            tracing::error!(error = %e, "resolve spawn_blocking panic");
            return internal_error();
        }
    };

    // 第 3 步：读 body（32MB 上限，超限 413；失败路径不计 requests，C-P2-5）
    let body_bytes = match to_bytes(body, MAX_BODY_SIZE).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "body read failed or too large");
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                r#"{"error":"body_too_large"}"#,
            )
                .into_response();
        }
    };
    let bytes_in = body_bytes.len() as u64;

    // 第 4 步：用量计数（body 读取完成后，C-P2-5）
    let usage = Arc::clone(&state.usage);
    usage.record_request(&ctx.route, ctx.token_id, bytes_in);

    // 第 5 步：构造 ForwardRequest 转发
    let edge = match state.edges.get(upstream.as_str()) {
        Some(e) => e.clone(),
        None => {
            tracing::error!(upstream = %upstream.as_str(), "upstream not configured");
            return (
                StatusCode::BAD_GATEWAY,
                r#"{"error":"upstream_error"}"#,
            )
                .into_response();
        }
    };
    let fwd = ForwardRequest {
        method: reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::GET),
        target_url,
        headers: pproxy_core::EdgeClient::sanitize_headers(&headers),
        body: if body_bytes.is_empty() {
            None
        } else {
            Some(body_bytes.to_vec())
        },
    };
    let resp = match edge.execute(fwd).await {
        Ok(r) => r,
        Err(e) => {
            // 错误日志禁记完整 URL（S-P2-10）：仅 route/host 类别
            tracing::warn!(route = %ctx.route, error_kind = "edge_execute_failed", detail = %e, "upstream error");
            return (
                StatusCode::BAD_GATEWAY,
                r#"{"error":"upstream_error"}"#,
            )
                .into_response();
        }
    };

    // 第 6 步：响应透传（状态码 + 过滤头 + 流式 body + bytes_out 计数）
    let status = resp.status();
    let mut out_headers = axum::http::HeaderMap::new();
    for (k, v) in resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        // 响应头过滤（T3 §4.2 第 6 步）：去 hop-by-hop 与 content-length
        if matches!(
            name.as_str(),
            "transfer-encoding" | "connection" | "content-length"
        ) {
            continue;
        }
        out_headers.insert(k.clone(), v.clone());
    }
    out_headers.insert(
        axum::http::header::CONNECTION,
        HeaderValue::from_static("close"),
    );

    let route_name = ctx.route.clone();
    let token_id = ctx.token_id;
    let usage_for_stream = usage;
    let stream = resp.bytes_stream();
    let counted = futures::stream::unfold(
        (stream, route_name, token_id, usage_for_stream),
        |(mut stream, route, token_id, usage)| async move {
            match futures::StreamExt::next(&mut stream).await {
                Some(Ok(chunk)) => {
                    usage.record_bytes_out(&route, token_id, chunk.len() as u64);
                    Some((Ok::<_, std::io::Error>(chunk), (stream, route, token_id, usage)))
                }
                Some(Err(e)) => Some((Err(std::io::Error::other(e.to_string())), (stream, route, token_id, usage))),
                None => None,
            }
        },
    );

    let mut builder = Response::builder().status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY));
    if let Some(hm) = builder.headers_mut() {
        *hm = out_headers;
    }
    builder
        .body(Body::from_stream(counted))
        .unwrap_or_else(|_| internal_error())
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, r#"{"error":"unknown_route"}"#).into_response()
}

/// accept 循环（T3 §2.2/§2.3）：CONNECT 拦截 + Semaphore 并发上限 + http1 驱动。
pub async fn serve_data_plane(listener: TcpListener, router: Router) -> std::io::Result<()> {
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    loop {
        let (stream, _) = listener.accept().await?;
        let permit = Arc::clone(&sem).acquire_owned().await;
        let router = router.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handle_conn(stream, router).await;
        });
    }
}

/// 单连接处理：peek 首行分流 CONNECT 与普通 HTTP。
async fn handle_conn(
    mut stream: tokio::net::TcpStream,
    router: Router,
) {
    
    let mut peek_buf = [0u8; 16];
    let n = match stream.peek(&mut peek_buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let first = &peek_buf[..n];
    if first.len() >= 7 && first[..7].eq_ignore_ascii_case(b"CONNECT") {
        // P0-1：CONNECT 直接禁用，写 403 后关闭
        let body = br#"{"error":"connect_forbidden"}"#;
        let resp = format!(
            "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        use tokio::io::AsyncWriteExt;
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.write_all(body).await;
        let _ = stream.flush().await;
        return;
    }
    // 普通 HTTP：axum Router 经适配器作为 hyper Service 驱动（TokioIo 桥接 tokio stream）。
    // F3：header_read_timeout 限首部读取窗口，防慢速连接占满 Semaphore 配额。
    use hyper_util::rt::{TokioTimer, tokio::TokioExecutor};
    let adapter = RouterHyperAdapter { router };
    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
        .http1()
        .header_read_timeout(Some(std::time::Duration::from_secs(30)))
        .timer(TokioTimer::new())
        .serve_connection(hyper_util::rt::TokioIo::new(stream), adapter)
        .await;
}

/// hyper Service 适配器：axum `Router<()>` 实现的是 tower_service::Service
/// （&mut self），hyper 1.x 的 Service 是 &self——每连接 clone Router 后在
/// clone 上调用（Router::clone 廉价：内部 Arc）。Request<Incoming> →
/// Request<Body> 转换在此完成。
struct RouterHyperAdapter {
    router: Router,
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for RouterHyperAdapter {
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        Box::pin(tower::Service::call(&mut self.router.clone(), req.map(Body::new)))
    }
}
