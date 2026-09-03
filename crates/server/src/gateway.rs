//! 数据面网关（T3）：axum 化 + 路径 token 鉴权 + 转发。
//!
//! 架构（T3 §2.2 C-P2-14 唯一路径 + pproxy-connect-tunnel 扩展）：
//! accept 循环 → Semaphore 并发上限 → hyper http1::Builder::serve_connection
//!   ├─ CONNECT → RouterHyperAdapter 拦截 → connect::handle_connect
//!   │            （allowlist 命中 → WS 隧道透传；否则 403/502 + x-pproxy-reason）
//!   └─ 其他 → 数据面 Router（路径 token 鉴权 + 转发）
//!
//! hyper↔axum 桥接：axum 0.7 `Router<()>` 实现的是 tower_service::Service，
//! hyper 1.x 需要自己的 `hyper::service::Service`，经 RouterHyperAdapter
//! 委托桥接（本 crate 无 tower 依赖，poll_ready 恒 Ready 等价转发）。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
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
    /// CONNECT 隧道（pproxy-connect-tunnel spec §3.4 + 待命池）：None 时 CONNECT 全 403。
    pub tunnel: Option<Arc<crate::connect::TunnelPool>>,
}

/// 组装数据面 Router（main.rs 与测试共用）。
pub fn data_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(info_endpoint))
        .route("/clash", get(clash_profile_handler))
        .route("/clash.yaml", get(clash_profile_handler))
        .route("/dsk/:filename", get(crate::dsk::dsk_file_public))
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

/// Clash Meta 客户端配置订阅端点（供手机直接扫码或通过 URL 订阅导入）。
async fn clash_profile_handler(headers: HeaderMap) -> Response {
    let db_path = pproxy_core::store::default_db_path();
    let parent = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let clash_file = parent.join("clash.yaml");

    if let Ok(yaml_bytes) = tokio::fs::read(&clash_file).await {
        return (
            StatusCode::OK,
            [
                ("content-type", "application/yaml; charset=utf-8"),
                (
                    "content-disposition",
                    "inline; filename=\"clash.yaml\"",
                ),
            ],
            yaml_bytes,
        )
            .into_response();
    }

    // 若本地 clash.yaml 不存在，根据请求 host 动态合成默认配置
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1:8899");
    let (ip, port) = host.split_once(':').unwrap_or((host, "8899"));
    let fallback_yaml = format!(
        r#"# ================================================================
#  Pony Proxy — Clash Meta 默认代理配置 (动态合成)
# ================================================================
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
ipv6: false

proxies:
  - name: "Pony-Proxy"
    type: http
    server: {ip}
    port: {port}

proxy-groups:
  - name: "PROXY"
    type: select
    proxies:
      - "Pony-Proxy"
      - DIRECT

rules:
  - DOMAIN-SUFFIX,openai.com,PROXY
  - DOMAIN-SUFFIX,chatgpt.com,PROXY
  - DOMAIN-SUFFIX,oaistatic.com,PROXY
  - DOMAIN-SUFFIX,oaiusercontent.com,PROXY
  - DOMAIN-SUFFIX,anthropic.com,PROXY
  - DOMAIN-SUFFIX,claude.ai,PROXY
  - DOMAIN-SUFFIX,google.com,PROXY
  - DOMAIN-SUFFIX,googleapis.com,PROXY
  - DOMAIN-SUFFIX,github.com,PROXY
  - GEOIP,CN,DIRECT
  - MATCH,PROXY
"#
    );

    (
        StatusCode::OK,
        [
            ("content-type", "application/yaml; charset=utf-8"),
            (
                "content-disposition",
                "inline; filename=\"clash.yaml\"",
            ),
        ],
        fallback_yaml,
    )
        .into_response()
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
    // 根路径 info 端点与 clash 配置端点不鉴权
    let path = req.uri().path().to_string();
    if path == "/" || path == "/clash" || path == "/clash.yaml" || path.starts_with("/dsk/") {
        return next.run(req).await;
    }

    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let rest = path.strip_prefix('/').unwrap_or(&path).to_string();

    let (first_seg, remaining) = split_first_segment(&rest);
    // F-fix(m4)：token 与 route_path 必须同源绑定——header 模式此前只消费了
    // 匹配结果却未把 header 值赋给 token，导致 verify 的是路径首段、header
    // 模式恒 401（公网入口联调发现，路径模式不受影响）。
    let (token_plaintext, route_path): (String, String) =
        if first_seg.starts_with(pproxy_core::token::TOKEN_PREFIX) {
            // 路径模式：首段为 token 段（route 名创建时已禁 pony_ 前缀，T4 §6 无歧义）。
            // 首段以 pony_ 开头但无 header 也按路径模式处理（T3 §4.1 歧义规则）。
            (first_seg.to_string(), remaining.to_string())
        } else {
            // header 模式：首段即 route，token 取 X-Pony-Token
            match req.headers().get(X_PONY_TOKEN).and_then(|v| v.to_str().ok()) {
                Some(t) if !t.is_empty() => (t.to_string(), rest.clone()),
                _ => return unauthorized(),
            }
        };

    if route_path.is_empty() {
        return unauthorized();
    }
    // F-2026-08-27：route 名必须从业务路径剥离——T3 §4.1 语义为
    // /{token}/{route}/{path} → https://{target_host}/{path}。
    // 此前 path_query 携带完整剩余路径（含 route 名），resolve 拼出的上游
    // URL 恒为 https://{host}/{route}/{path}，实测 opencode.ai/zen 与
    // api.anthropic.com 均 404（2026-08-27 zen 回归定位）。
    let (route_name, rest_path) = split_first_segment(&route_path);
    if route_name.is_empty() {
        return unauthorized();
    }
    let path_query = format!("/{rest_path}{query}");

    // verify 三种失败（NotFound/Revoked/Expired）同 401 同体，防枚举（T3 §4.1 第 4 步）
    // 纯内存无阻塞鉴权：消除 spawn_blocking 线程池调度开销
    let row = match state.tokens.verify(&token_plaintext) {
        Ok(row) => row,
        Err(reason) => {
            // 仅 info 记录原因类别，不打 token 明文（T3 §4.1）
            tracing::info!(reason = %reason, "token verify failed");
            return unauthorized();
        }
    };

    // 剥离后的 {route}/{path}?{query} 放入 extensions（T3 §4.1 第 5 步）。
    // route_name/path_query 已在上方派生（F-2026-08-27：route 名不入 path_query）
    let route_name = route_name.to_string();
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
    // 纯内存无阻塞查询：消除 spawn_blocking 线程池调度开销
    let (target_url, upstream) = match state.routes.resolve(&ctx.route, &ctx.path_query) {
        Ok(v) => v,
        Err(_) => {
            // UnknownRoute / Disabled 均映射 404 {"error":"unknown_route"}（T3 §4.2 第 2 步）
            return not_found();
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

/// accept 循环（T3 §2.2/§2.3 + CONNECT 隧道扩展）：Semaphore 并发上限 + http1 驱动。
/// peek 首行分流：CONNECT 不经过 hyper（hyper 不支持裸 CONNECT 隧道），
/// 直接由 connect::handle_connect 走 WS 隧道；其余由 hyper http1 驱动（axum 路由）。
/// Semaphore 说明：permit 在 `hyper::serve_connection` 或 tunnel 连接结束时释放。
/// 隧道连接持 permit 至 relay 结束（长连接场景下实际上限由 OS TCP 连接数构成第二道槛）。
pub async fn serve_data_plane(listener: TcpListener, state: GatewayState) -> std::io::Result<()> {
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let router = data_router(state.clone());
    loop {
        let (stream, _) = listener.accept().await?;
        // 入站 socket 禁用 Nagle：小包响应（headers/首 chunk）不再等 40ms+
        // delayed-ACK 凑包，hyper 手动 serve_connection 不会代为设置。
        let _ = stream.set_nodelay(true);
        let permit = Arc::clone(&sem).acquire_owned().await;
        let router = router.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handle_conn(stream, router, state).await;
        });
    }
}

/// 单连接处理：peek 首行分流 CONNECT 与普通 HTTP。
/// F3：header_read_timeout 由 hyper 覆盖（非 CONNECT 路径）；CONNECT 路径读头后
/// 需在 30s 内完成首行接收（防慢速连接占满 Semaphore 配额）。
async fn handle_conn(
    mut stream: tokio::net::TcpStream,
    router: Router,
    state: GatewayState,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut peek_buf = [0u8; 16];
    let n = match stream.peek(&mut peek_buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let first = &peek_buf[..n];
    if first.len() >= 7 && first[..7].eq_ignore_ascii_case(b"CONNECT") {
        // CONNECT 隧道：不经过 hyper，直接裸 TCP 处理
        // 读全请求头至 \r\n\r\n（上限 16KB），超时 30s
        let mut buf = Vec::with_capacity(1024);
        let mut tmp = [0u8; 2048];
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // 超时，关闭连接
                let _ = stream.shutdown().await;
                return;
            }
            match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
                Ok(Ok(0)) => return,
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                        break;
                    }
                }
                _ => return,
            }
        }
        let split_pos = buf.windows(4).position(|w| w == b"\r\n\r\n");
        let (head_bytes, leftover) = match split_pos {
            Some(pos) => (&buf[..pos + 4], buf[pos + 4..].to_vec()),
            None => (&buf[..], Vec::new()),
        };
        let head = String::from_utf8_lossy(head_bytes);
        let _ = crate::connect::handle_connect_raw(state, &head, leftover, stream).await;
        return;
    }
    // 普通 HTTP：axum Router 经适配器作为 hyper Service 驱动（TokioIo 桥接 tokio stream）。
    // F3：header_read_timeout 限首部读取窗口，防慢速连接占满 Semaphore 配额。
    let adapter = RouterHyperAdapter { router };
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Some(std::time::Duration::from_secs(30)))
        .keep_alive(true)
        .serve_connection(hyper_util::rt::TokioIo::new(stream), adapter)
        .await;
}

/// hyper Service 适配器：axum `Router<()>` 实现的是 tower_service::Service
/// （&mut self），hyper 1.x 的 Service 是 &self——每连接 clone Router 与 state 后在
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

#[cfg(test)]
mod tests {
    //! 鉴权中间件回归（M4 公网入口联调发现 header 模式恒 401 的缺陷后补齐）：
    //! 路径/双模式的 token 绑定正确性——此前自动化只覆盖路径模式成功路径。
    use super::*;
    use axum::http::{HeaderValue, Request};
    use pproxy_core::{Store, UsageTracker};
    use tower::ServiceExt;

    /// 标准库临时目录建库（不引 tempfile 依赖，Cargo.toml 冻结）。
    fn temp_store(tag: &str) -> Arc<pproxy_core::Store> {
        let dir = std::env::temp_dir().join(format!("m4-gwtest-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (store, _) = Store::open(&dir.join("state.db")).unwrap();
        Arc::new(store)
    }

    /// WHY 返回 TokenService 本体：缓存即全量权威快照（token.rs §5，cache miss
    /// 不回库），token 必须经 router 持有的同一实例创建才能被 verify 命中。
    fn build_router(tag: &str) -> (Router, Arc<TokenService>) {
        let store = temp_store(tag);
        let tokens = Arc::new(TokenService::new(Arc::clone(&store)).unwrap());
        let routes = Arc::new(pproxy_core::RouteTable::new(
            Arc::clone(&store),
            Arc::new(HashMap::new()),
        )
        .unwrap());
let router = data_router(GatewayState {
	            tokens: Arc::clone(&tokens),
	            edges: Arc::new(HashMap::new()),
	            routes,
	            usage: Arc::new(UsageTracker::new(store)),
	            tunnel: None,
	        });
        (router, tokens)
    }

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// 局部命名 req_get：避免与模块级 axum::routing::get 歧义。
    fn req_get(uri: &str, token_header: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Some(t) = token_header {
            b = b.header(X_PONY_TOKEN, t);
        }
        b.body(Body::empty()).unwrap()
    }

    // ---- header 模式：有效 token 必须穿透鉴权层抵达路由解析（404 unknown_route）----

    #[tokio::test]
    async fn header_mode_valid_token_reaches_route_layer() {
        let (router, tokens) = build_router("hdr-ok");
        let (_, plaintext) = tokens.create_token("hdr-test", None).unwrap();
        let resp = router.oneshot(req_get("/nosuchroute-x/v1/x", Some(&plaintext))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "header 模式有效 token 应穿过鉴权层");
        assert!(body_string(resp).await.contains("unknown_route"));
    }

    // ---- 路径模式：同判据（防修复回归路径模式）----

    #[tokio::test]
    async fn path_mode_valid_token_unknown_route_404() {
        let (router, tokens) = build_router("path-ok");
        let (_, plaintext) = tokens.create_token("path-test", None).unwrap();
        let resp = router.oneshot(req_get(&format!("/{plaintext}/nosuchroute-x/v1/x"), None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(body_string(resp).await.contains("unknown_route"));
    }

    // ---- 无效/缺失 token：401 同体（两种模式一致）----

    #[tokio::test]
    async fn invalid_and_missing_token_unauthorized_both_modes() {
        let (router, _tokens) = build_router("neg");
        // header 模式：无效值
        let r1 = router.clone().oneshot(req_get("/openai/models", Some("pony_ffffffff"))).await.unwrap();
        assert_eq!(r1.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_string(r1).await, r#"{"error":"unauthorized"}"#);
        // header 模式：缺失头
        let r2 = router.clone().oneshot(req_get("/openai/models", None)).await.unwrap();
        assert_eq!(r2.status(), StatusCode::UNAUTHORIZED);
        // 路径模式：无效 token
        let r3 = router.oneshot(req_get("/pony_00000000000000000000000000000000/openai/models", None)).await.unwrap();
        assert_eq!(r3.status(), StatusCode::UNAUTHORIZED);
    }

    // ---- 根路径 info 端点无鉴权（T3 §4.3 既有行为守卫）----

    #[tokio::test]
    async fn root_info_endpoint_no_auth() {
        let (router, _tokens) = build_router("root");
        let resp = router.oneshot(req_get("/", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("service"));
    }

    // ---- 长连接支持：响应头不含强制 Connection: close ----

    #[tokio::test]
    async fn response_does_not_force_connection_close() {
        let (router, _tokens) = build_router("keepalive");
        let resp = router.oneshot(req_get("/", None)).await.unwrap();
        assert_ne!(
            resp.headers().get(axum::http::header::CONNECTION),
            Some(&HeaderValue::from_static("close")),
            "网关响应不得强制插入 Connection: close"
        );
    }

    // ---- 回归（F-2026-08-27）：上游 target URL 必须剥离 route 名 ----
    // 缺陷现场：auth_middleware 把 route 名混入 path_query，resolve 拼出的
    // 上游 URL 恒为 https://{host}/{route}/{path}——opencode.ai/zen 与
    // api.anthropic.com 均以 404 应答。既有单测只覆盖 resolve（入参本就是
    // 干净 path_query），未覆盖网关侧的派生，此处经本地 stub「上游边缘」
    // 端到端断言 EdgeClient 实际收到的 target。

    fn pct_decode(s: &str) -> String {
        fn hex_val(b: u8) -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            }
        }
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(hi * 16 + lo);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// 本地 stub「上游边缘」：模拟 vedge/api/proxy——解析 ?url= 并把解码后的
    /// target 回传为响应体（线程返回捕获值供断言）。
    fn spawn_url_echo_stub() -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_len = 0usize;
            loop {
                let mut h = String::new();
                let n = reader.read_line(&mut h).unwrap();
                if n == 0 || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            // 读尽 body，避免 reqwest 写侧中断（内容不使用）
            let mut sink = vec![0u8; content_len];
            let _ = reader.read_exact(&mut sink);
            let target = request_line
                .split_whitespace()
                .nth(1)
                .and_then(|p| p.split_once("url="))
                .map(|(_, raw)| pct_decode(raw))
                .unwrap_or_default();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                target.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.write_all(target.as_bytes()).unwrap();
            target
        });
        (format!("http://{addr}/api/proxy"), handle)
    }

    #[tokio::test]
    async fn forwarded_url_strips_route_name() {
        let (stub_url, stub_handle) = spawn_url_echo_stub();
        let store = temp_store("urlstrip");
        let tokens = Arc::new(TokenService::new(Arc::clone(&store)).unwrap());
        let mut edges = HashMap::new();
        edges.insert(
            "vercel".to_string(),
            EdgeClient::new(&stub_url, "stub-secret").unwrap(),
        );
        let routes = Arc::new(
            pproxy_core::RouteTable::new(Arc::clone(&store), Arc::new(HashMap::new())).unwrap(),
        );
        // opencode 属 VERCEL_HOSTS 主机规则，无 override 也解析到 vercel 上游
        routes
            .create_route(&pproxy_core::store::NewRoute {
                name: "opencode".into(),
                target_host: "opencode.ai".into(),
                override_upstream: None,
            })
            .unwrap();
        let router = data_router(GatewayState {
            tokens: Arc::clone(&tokens),
            edges: Arc::new(edges),
            routes,
            usage: Arc::new(UsageTracker::new(store)),
            tunnel: None,
        });
        let (_, plaintext) = tokens.create_token("urlstrip-tok", None).unwrap();

        // 路径模式 POST：/{token}/opencode/zen/go/v1/responses
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{plaintext}/opencode/zen/go/v1/responses"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer sk-test")
                    .body(Body::from(r#"{"model":"muse-spark-1.2-contributor"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let want = "https://opencode.ai/zen/go/v1/responses";
        assert_eq!(body_string(resp).await, want, "stub 回显 target");
        assert_eq!(stub_handle.join().unwrap(), want, "上游实际收到的 target");
    }
}
