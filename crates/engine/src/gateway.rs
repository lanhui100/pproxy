//! 数据面网关路由与转发处理器。

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pproxy_core::gatekeeper::AuthGatekeeper;
use pproxy_core::token::TOKEN_PREFIX;
use pproxy_core::token::TokenService;
use pproxy_core::usage::UsageTracker;
use pproxy_core::user::UserService;

use crate::auth::{
    extract_credentials, proxy_auth_required, rate_limited_lockout, split_first_segment,
    unauthorized, AuthContext, AuthSubject,
};
use crate::connect::TunnelPool;
use crate::upstream::UpstreamManager;

pub const MAX_BODY_SIZE: usize = 32 * 1024 * 1024;

/// 数据面共享状态。
#[derive(Clone)]
pub struct GatewayState {
    pub tokens: Arc<TokenService>,
    pub users: Option<Arc<UserService>>,
    pub upstream: Arc<UpstreamManager>,
    pub usage: Arc<UsageTracker>,
    pub gatekeeper: Arc<AuthGatekeeper>,
    pub tunnel: Option<Arc<TunnelPool>>,
    pub instance_uuid: String,
}

/// 组装数据面 Router。
pub fn build_data_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(info_endpoint))
        .route("/__pproxy_health", get(health_endpoint))
        .route("/clash", get(clash_profile_handler))
        .route("/clash.yaml", get(clash_profile_handler))
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

/// 根路径信息端点（无鉴权）。
async fn info_endpoint() -> Response {
    Json(serde_json::json!({
        "service": "pony-proxy",
        "version": "0.4.0",
        "auth": "Basic Auth (user:pass) or X-Pony-Token",
    }))
    .into_response()
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
    let (ip, port) = host.rsplit_once(':').unwrap_or((host, "8899"));

    let fallback_yaml = format!(
        r#"# Pony Proxy — Clash Meta 默认配置
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

/// 实例握手探活端点（无鉴权，返回唯一 Instance UUID 防假活）。
async fn health_endpoint(State(state): State<GatewayState>) -> Response {
    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-pproxy-instance-uuid", state.instance_uuid.as_str()),
        ],
        format!(
            r#"{{"status":"ok","instance_uuid":"{}"}}"#,
            state.instance_uuid
        ),
    )
        .into_response()
}

/// 全局认证中间件。
pub async fn auth_middleware(
    State(state): State<GatewayState>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // 1. 公开端点豁免鉴权
    if path == "/" || path == "/__pproxy_health" || path == "/clash" || path == "/clash.yaml" || path.starts_with("/dsk/") {
        return next.run(req).await;
    }

    let client_ip = req
        .extensions()
        .get::<std::net::SocketAddr>()
        .map(|s| s.ip())
        .unwrap_or_else(|| std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));

    // 2. 防爆破门禁检查
    if let Err(_) = state.gatekeeper.check(&client_ip) {
        return rate_limited_lockout();
    }

    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let rest = path.strip_prefix('/').unwrap_or(&path).to_string();
    let (first_seg, remaining) = split_first_segment(&rest);

    let (basic_auth, token_header) = extract_credentials(req.headers());

    // 3. 分流鉴权
    let auth_result = if let Some((username, password)) = basic_auth {
        // Basic Auth 模式
        if let Some(user_service) = state.users.as_ref() {
            if let Some(user) = user_service.verify_user(&username, &password) {
                let (route, path_query) = if first_seg.is_empty() {
                    return unauthorized();
                } else {
                    (first_seg.to_string(), format!("{remaining}{query}"))
                };
                Some(AuthContext {
                    subject: AuthSubject::User {
                        id: user.id,
                        username: user.username,
                    },
                    route,
                    path_query,
                })
            } else {
                None
            }
        } else {
            None
        }
    } else if first_seg.starts_with(TOKEN_PREFIX) {
        // 路径 Token 模式: /{token}/{route}/*
        if let Ok(token_row) = state.tokens.verify(first_seg) {
            let (route_name, sub_path) = split_first_segment(remaining);
            if route_name.is_empty() {
                None
            } else {
                Some(AuthContext {
                    subject: AuthSubject::Token {
                        id: token_row.id,
                        name: token_row.name,
                    },
                    route: route_name.to_string(),
                    path_query: format!("{sub_path}{query}"),
                })
            }
        } else {
            None
        }
    } else if let Some(tok) = token_header {
        // Header Token 模式: X-Pony-Token
        if let Ok(token_row) = state.tokens.verify(&tok) {
            if first_seg.is_empty() {
                None
            } else {
                Some(AuthContext {
                    subject: AuthSubject::Token {
                        id: token_row.id,
                        name: token_row.name,
                    },
                    route: first_seg.to_string(),
                    path_query: format!("{remaining}{query}"),
                })
            }
        } else {
            None
        }
    } else {
        None
    };

    match auth_result {
        Some(ctx) => {
            state.gatekeeper.record_success(&client_ip);
            req.extensions_mut().insert(ctx);
            next.run(req).await
        }
        None => {
            state.gatekeeper.record_failure(client_ip);
            if req.headers().contains_key("proxy-authorization") {
                proxy_auth_required()
            } else {
                unauthorized()
            }
        }
    }
}

/// 统一转发 Handler。
async fn forward_handler(
    State(state): State<GatewayState>,
    method: Method,
    headers: HeaderMap,
    req: Request,
) -> Response {
    // 安全提取 AuthContext，避免未捕获的 panic 500
    let auth = match req.extensions().get::<AuthContext>() {
        Some(ctx) => ctx.clone(),
        None => return unauthorized(),
    };

    let body = req.into_body();
    let body_bytes = match to_bytes(body, MAX_BODY_SIZE).await {
        Ok(b) => b.to_vec(),
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                [("content-type", "application/json")],
                r#"{"error":"payload_too_large"}"#,
            )
                .into_response();
        }
    };

    let bytes_in = body_bytes.len() as u64;

    let resp = state
        .upstream
        .forward(
            &auth.route,
            &auth.path_query,
            method,
            headers,
            body_bytes,
        )
        .await;

    // 记录用量统计
    let token_id = match auth.subject {
        AuthSubject::Token { id, .. } => id,
        AuthSubject::User { id, .. } => id,
    };
    let route = auth.route;
    state.usage.record_request(&route, token_id, bytes_in);

    resp
}
