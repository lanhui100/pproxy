//! 管理 API（T6）：:8900 管理面全部端点 + admin Bearer 中间件。
//!
//! 错误契约（C-P1-7）：4xx/5xx 的 error 字段一律为固定文案常量，
//! 禁止 TokenError/RouteError/StoreError 的 Display 透传；
//! 变体 → (状态码, 文案) 映射集中在 `map_token_error` / `map_route_error`。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use pproxy_core::route::Upstream;
use pproxy_core::store::{MarkReadOutcome, RevokeOutcome, RouteRow, TokenRow};
use pproxy_core::token::ADMIN_NAME;
use pproxy_core::{RouteTable, TokenService, UsageTracker};
use serde::Deserialize;

use crate::monitor::MonitorHandle;

/// 管理面共享状态。
#[derive(Clone)]
pub struct AdminState {
    pub tokens: Arc<TokenService>,
    pub routes: Arc<RouteTable>,
    pub usage: Arc<UsageTracker>,
    /// M3：alerts/quota 直查 store（比经 tokens 间接达更高内聚）。
    pub store: Arc<pproxy_core::Store>,
    /// M3：采集来源健康状态（monitor 任务写，此处只读）。
    pub monitor: Arc<MonitorHandle>,
}

// ---- 固定文案常量（C-P1-7：单一出处，handler 禁止各自拼写） ----

const ERR_UNAUTHORIZED: &str = "unauthorized";
const ERR_NOT_FOUND: &str = "not_found";
const ERR_INTERNAL: &str = "internal";
const ERR_INVALID_NAME: &str = "invalid token name";
const ERR_NAME_EXISTS: &str = "name already exists";
const ERR_INVALID_EXPIRY: &str = "invalid expires_days";
const ERR_CANNOT_REVOKE_ADMIN: &str = "cannot revoke admin";
const ERR_INVALID_ROUTE_NAME: &str = "invalid route name";
const ERR_INVALID_HOST: &str = "invalid target host";
const ERR_INVALID_UPSTREAM: &str = "invalid upstream";
const ERR_ROUTE_EXISTS: &str = "route name already exists";
const ERR_BAD_REQUEST: &str = "bad_request";

fn err_body(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

// ---- TokenError / RouteError → 固定文案映射（集中点，C-P1-7） ----

fn map_token_error(e: &pproxy_core::token::TokenError) -> Response {
    use pproxy_core::token::TokenError as TE;
    match e {
        TE::NotFound | TE::Revoked | TE::Expired => {
            err_body(StatusCode::UNAUTHORIZED, ERR_UNAUTHORIZED)
        }
        TE::Duplicate => err_body(StatusCode::BAD_REQUEST, ERR_NAME_EXISTS),
        TE::InvalidName => err_body(StatusCode::BAD_REQUEST, ERR_INVALID_NAME),
        TE::InvalidExpiry => err_body(StatusCode::BAD_REQUEST, ERR_INVALID_EXPIRY),
        TE::Store(_) => err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL),
    }
}

fn map_route_error(e: &pproxy_core::route::RouteError) -> Response {
    use pproxy_core::route::RouteError as RE;
    match e {
        RE::UnknownRoute => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        RE::Disabled => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        RE::InvalidName => err_body(StatusCode::BAD_REQUEST, ERR_INVALID_ROUTE_NAME),
        RE::InvalidHost => err_body(StatusCode::BAD_REQUEST, ERR_INVALID_HOST),
        RE::InvalidUpstream => err_body(StatusCode::BAD_REQUEST, ERR_INVALID_UPSTREAM),
        RE::Duplicate => err_body(StatusCode::BAD_REQUEST, ERR_ROUTE_EXISTS),
        RE::Store(_) => err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL),
    }
}

// ---- admin Bearer 中间件（T6 §3.1） ----

async fn admin_auth_middleware(
    State(state): State<AdminState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        return err_body(StatusCode::UNAUTHORIZED, ERR_UNAUTHORIZED);
    };
    // verify_admin 失败与缺失同 401 同体，不区分原因（防探测）
    let tokens = Arc::clone(&state.tokens);
    let plaintext = token.to_string();
    match tokio::task::spawn_blocking(move || tokens.verify_admin(&plaintext)).await {
        Ok(Ok(_)) => next.run(req).await,
        Ok(Err(e)) => {
            tracing::info!(reason = %e, "admin auth failed");
            err_body(StatusCode::UNAUTHORIZED, ERR_UNAUTHORIZED)
        }
        Err(e) => {
            tracing::error!(error = %e, "verify_admin spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

// ---- Router 组装（T6 §5：全部端点无例外鉴权，health 也要） ----

pub fn admin_router(state: AdminState) -> Router {
    Router::new()
        .route("/api/tokens", post(create_token_handler).get(list_tokens_handler))
        .route("/api/tokens/:id", delete(revoke_token_handler))
        .route(
            "/api/routes",
            get(list_routes_handler).post(create_route_handler),
        )
        .route(
            "/api/routes/:name",
            patch(update_route_handler).delete(delete_route_handler),
        )
        .route("/api/routes/:name/test", post(test_route_handler))
        .route("/api/usage", get(usage_handler))
        .route("/api/health", get(health_handler))
        .route("/api/alerts", get(alerts_handler))
        .route("/api/alerts/:id/read", post(mark_alert_read_handler))
        .route("/api/quota", get(quota_handler))
        .layer(from_fn_with_state(state.clone(), admin_auth_middleware))
        .with_state(state)
}

// ---- DTO ----

#[derive(Deserialize)]
struct CreateTokenReq {
    name: String,
    #[serde(default)]
    expires_days: Option<u64>,
}

#[derive(Deserialize)]
struct CreateRouteReq {
    name: String,
    target_host: String,
    #[serde(default)]
    override_upstream: Option<String>,
}

/// PATCH 三态 DTO（C-P0-2 double_option）：None=不改；Some(None)=清除；Some(Some(v))=设置。
#[derive(Deserialize)]
struct UpdateRouteReq {
    #[serde(default, deserialize_with = "deserialize_double_option")]
    override_upstream: Option<Option<String>>,
    #[serde(default)]
    enabled: Option<bool>,
    /// C-P2-9：upstream 列是创建时快照不可 PATCH；出现即 400
    #[serde(default)]
    upstream: Option<serde_json::Value>,
}

/// serde double_option：字段缺席 → None；显式 null → Some(None)；有值 → Some(Some(v))。
fn deserialize_double_option<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_with_double_option(de)
}

// 手写 double_option（workspace 无 serde_with，禁改 Cargo.toml）
fn serde_with_double_option<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Option<Option<String>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("string or null or absent")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Some(None))
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Some(None))
        }
        fn visit_some<D2: serde::Deserializer<'de>>(
            self,
            d: D2,
        ) -> Result<Self::Value, D2::Error> {
            Ok(Some(Some(String::deserialize(d)?)))
        }
    }
    de.deserialize_option(V)
}

#[derive(Deserialize)]
struct UsageQuery {
    hours: Option<u64>,
    route: Option<String>,
    token_id: Option<i64>,
}

/// 用量响应行 DTO（C-P2-10）：不含 ts_hour（聚合哨兵值禁止透出）。
#[derive(serde::Serialize)]
struct UsageRowDto {
    route: String,
    token_id: i64,
    requests: u64,
    bytes_in: u64,
    bytes_out: u64,
}

// ---- handler：tokens ----

async fn create_token_handler(State(st): State<AdminState>, body: Option<Json<CreateTokenReq>>) -> Response {
    let Some(Json(req)) = body else {
        return err_body(StatusCode::BAD_REQUEST, ERR_BAD_REQUEST);
    };
    // __admin__ 保留名前置拒绝（T6 §4；TokenService 对 InvalidName 同样拒绝，双保险）
    if req.name == ADMIN_NAME {
        return err_body(StatusCode::BAD_REQUEST, ERR_INVALID_NAME);
    }
    let tokens = Arc::clone(&st.tokens);
    let name = req.name.clone();
    let created =
        tokio::task::spawn_blocking(move || tokens.create_token(&name, req.expires_days)).await;
    match created {
        Ok(Ok((id, plaintext))) => {
            // expires_at 回读列表中该行（create 后必然存在）；直接按规则重算展示值
            // （避免二次查库：expires_days→expires_at 与 T2 实现同式）
            let expires_at = req.expires_days.map(|d| {
                pproxy_core::store::now_unix() + d * 86_400
            });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": id,
                    "name": req.name,
                    "token": plaintext, // 明文仅此一次（T6 §4）
                    "expires_at": expires_at,
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => map_token_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "create_token spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

/// status 派生（T6 §4 GET /api/tokens）：revoked > expired > active。
fn token_status(row: &TokenRow) -> &'static str {
    if row.revoked_at.is_some() {
        "revoked"
    } else if let Some(exp) = row.expires_at {
        if exp < pproxy_core::store::now_unix() {
            return "expired";
        }
        "active"
    } else {
        "active"
    }
}

async fn list_tokens_handler(State(st): State<AdminState>) -> Response {
    let tokens = Arc::clone(&st.tokens);
    match tokio::task::spawn_blocking(move || tokens.list_tokens()).await {
        Ok(Ok(rows)) => {
            // 脱敏（C-P1-8）：不返回 token_hash、不返回前 8 位——仅元数据
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "created_at": r.created_at,
                        "expires_at": r.expires_at,
                        "revoked_at": r.revoked_at,
                        "last_used_at": r.last_used_at,
                        "status": token_status(r),
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "tokens": items }))).into_response()
        }
        Ok(Err(e)) => map_token_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "list_tokens spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn revoke_token_handler(State(st): State<AdminState>, Path(id): Path<u64>) -> Response {
    let id = id as i64;
    // 防自锁：admin 行禁止撤销（T6 §4 DELETE /api/tokens/{id}）
    let tokens = Arc::clone(&st.tokens);
    let is_admin = match tokio::task::spawn_blocking(move || {
        tokens.list_tokens().ok().and_then(|rows| {
            rows.iter().find(|r| r.id == id).map(|r| r.name == ADMIN_NAME)
        })
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list_tokens spawn_blocking panic");
            return err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL);
        }
    };
    match is_admin {
        None => return err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        Some(true) => return err_body(StatusCode::BAD_REQUEST, ERR_CANNOT_REVOKE_ADMIN),
        Some(false) => {}
    }

    let tokens = Arc::clone(&st.tokens);
    match tokio::task::spawn_blocking(move || tokens.revoke_token(id)).await {
        Ok(Ok(outcome)) => match outcome {
            RevokeOutcome::Revoked | RevokeOutcome::AlreadyRevoked => {
                // 幂等（C-P1-4）：已撤销同样 200 {"revoked":true}
                (StatusCode::OK, Json(serde_json::json!({ "revoked": true }))).into_response()
            }
            RevokeOutcome::NotFound => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        },
        Ok(Err(e)) => map_token_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "revoke_token spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

// ---- handler：routes ----

fn route_row_json(r: &RouteRow, effective: Upstream) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "target_host": r.target_host,
        "upstream": r.upstream,
        "override_upstream": r.override_upstream,
        "enabled": r.enabled,
        "created_at": r.created_at,
        "effective_upstream": effective.as_str(),
    })
}

async fn list_routes_handler(State(st): State<AdminState>) -> Response {
    let routes = Arc::clone(&st.routes);
    let listed = tokio::task::spawn_blocking(move || routes.list_routes()).await;
    match listed {
        Ok(Ok(rows)) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    let eff = st.routes.effective_upstream(&r.name);
                    route_row_json(r, eff.unwrap_or(Upstream::Worker))
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "routes": items }))).into_response()
        }
        Ok(Err(e)) => map_route_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "list_routes spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn create_route_handler(State(st): State<AdminState>, body: Option<Json<CreateRouteReq>>) -> Response {
    let Some(Json(req)) = body else {
        return err_body(StatusCode::BAD_REQUEST, ERR_BAD_REQUEST);
    };
    let new_route = pproxy_core::store::NewRoute {
        name: req.name.clone(),
        target_host: req.target_host.clone(),
        override_upstream: req.override_upstream.clone(),
    };
    let routes = Arc::clone(&st.routes);
    let created = tokio::task::spawn_blocking(move || routes.create_route(&new_route)).await;
    match created {
        Ok(Ok(())) => {
            // 创建时的自动选择结果（T6 §4 POST /api/routes 响应）
            let upstream = st
                .routes
                .effective_upstream(&req.name)
                .unwrap_or(Upstream::Worker);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "name": req.name,
                    "upstream": upstream.as_str(),
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => map_route_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "create_route spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn update_route_handler(
    State(st): State<AdminState>,
    Path(name): Path<String>,
    body: Option<Json<UpdateRouteReq>>,
) -> Response {
    let Some(Json(req)) = body else {
        return err_body(StatusCode::BAD_REQUEST, ERR_BAD_REQUEST);
    };
    // C-P2-9：upstream 字段不可修改
    if req.upstream.is_some() {
        return err_body(StatusCode::BAD_REQUEST, ERR_INVALID_UPSTREAM);
    }
    let name_for_lookup = name.clone();
    let routes = Arc::clone(&st.routes);
    let updated = tokio::task::spawn_blocking(move || {
        routes.update_route(&name, req.override_upstream, req.enabled)
    })
    .await;
    match updated {
        Ok(Ok(true)) => {
            let routes_now = Arc::clone(&st.routes);
            let row = tokio::task::spawn_blocking(move || routes_now.list_routes())
                .await
                .ok()
                .and_then(|r| r.ok())
                .and_then(|rows| rows.into_iter().find(|r| r.name == name_for_lookup));
            let enabled = row.map(|r| r.enabled).unwrap_or(false);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "name": name_for_lookup, "enabled": enabled })),
            )
                .into_response()
        }
        Ok(Ok(false)) => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        Ok(Err(e)) => map_route_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "update_route spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn delete_route_handler(State(st): State<AdminState>, Path(name): Path<String>) -> Response {
    let routes = Arc::clone(&st.routes);
    match tokio::task::spawn_blocking(move || routes.delete_route(&name)).await {
        Ok(Ok(true)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": true })),
        )
            .into_response(),
        Ok(Ok(false)) => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        Ok(Err(e)) => map_route_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "delete_route spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn test_route_handler(State(st): State<AdminState>, Path(name): Path<String>) -> Response {
    // 前置存在性判断（404 先于网络调用）
    let name_for_exists = name.clone();
    let routes = Arc::clone(&st.routes);
    let exists =
        tokio::task::spawn_blocking(move || routes.effective_upstream(&name_for_exists).is_some())
            .await;
    match exists {
        Ok(true) => {}
        Ok(false) => return err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        Err(e) => {
            tracing::error!(error = %e, "exists check spawn_blocking panic");
            return err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL);
        }
    }
    let routes = Arc::clone(&st.routes);
    let tested = tokio::task::spawn_blocking(move || routes.test_route(&name)).await;
    match tested {
        Ok(Ok(result)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": result.ok,
                "status": result.status,
                "latency_ms": result.latency_ms,
                "error": result.error,
            })),
        )
            .into_response(),
        Ok(Err(e)) => map_route_error(&e),
        Err(e) => {
            tracing::error!(error = %e, "test_route spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

// ---- handler：usage / health / alerts ----

async fn usage_handler(State(st): State<AdminState>, Query(q): Query<UsageQuery>) -> Response {
    let hours = q.hours.unwrap_or(24);
    if hours == 0 || hours > 720 {
        return err_body(StatusCode::BAD_REQUEST, ERR_BAD_REQUEST);
    }
    let since_hour = pproxy_core::store::hour_floor(pproxy_core::store::now_unix())
        - hours * 3600;
    match st.usage.query(since_hour, q.route.as_deref(), q.token_id).await {
        Ok(rows) => {
            // C-P2-10：独立响应 DTO，不含 ts_hour
            let dtos: Vec<UsageRowDto> = rows
                .iter()
                .map(|r| UsageRowDto {
                    route: r.route.clone(),
                    token_id: r.token_id,
                    requests: r.requests,
                    bytes_in: r.bytes_in,
                    bytes_out: r.bytes_out,
                })
                .collect();
            let total_req: u64 = dtos.iter().map(|d| d.requests).sum();
            let total_in: u64 = dtos.iter().map(|d| d.bytes_in).sum();
            let total_out: u64 = dtos.iter().map(|d| d.bytes_out).sum();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "hours": hours,
                    "since_hour": since_hour,
                    "rows": dtos,
                    "total": {
                        "requests": total_req,
                        "bytes_in": total_in,
                        "bytes_out": total_out,
                    },
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "usage query failed");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

async fn health_handler(State(st): State<AdminState>) -> Response {
    // db 探测（C-P1-11）：失败 500
    let store_alive = {
        // Store 经 TokenService 间接持有；ping 走独立轻路径：
        // 直接用 usage flush 不合适，这里经 list_tokens 触达 DB 即可等价探测
        let tokens = Arc::clone(&st.tokens);
        tokio::task::spawn_blocking(move || tokens.list_tokens()).await
    };
    let db_ok = matches!(&store_alive, Ok(Ok(_)));
    if !db_ok {
        return err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL);
    }
    let rows = store_alive.ok().and_then(|r| r.ok()).unwrap_or_default();
    let tokens_active = rows
        .iter()
        .filter(|r| {
            r.revoked_at.is_none()
                && r.expires_at.map(|e| e >= pproxy_core::store::now_unix()).unwrap_or(true)
        })
        .count() as u64;

    let routes = Arc::clone(&st.routes);
    let route_rows = tokio::task::spawn_blocking(move || routes.list_routes())
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let mut routes_json = serde_json::Map::new();
    for r in &route_rows {
        let eff = st.routes.effective_upstream(&r.name).unwrap_or(Upstream::Worker);
        routes_json.insert(
            r.name.clone(),
            serde_json::json!({ "enabled": r.enabled, "upstream": eff.as_str() }),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "routes": routes_json,
            "tokens_active": tokens_active,
            "db": "ok",
        })),
    )
        .into_response()
}

// ---- handler：alerts / quota（M3） ----

#[derive(Deserialize)]
struct AlertsQuery {
    /// ?unread=1 仅未读
    #[serde(default)]
    unread: Option<u8>,
    /// 默认 50，钳制 ≤500（spec §8）
    #[serde(default)]
    limit: Option<u32>,
}

async fn alerts_handler(State(st): State<AdminState>, Query(q): Query<AlertsQuery>) -> Response {
    let unread_only = q.unread.map(|v| v != 0).unwrap_or(false);
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let store = Arc::clone(&st.store);
    match tokio::task::spawn_blocking(move || store.list_alerts(unread_only, limit)).await {
        Ok(Ok(rows)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "alerts": rows })), // AlertRow 已 Serialize，倒序
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "alerts query failed");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
        Err(e) => {
            tracing::error!(error = %e, "list_alerts spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

/// POST /api/alerts/{id}/read：R4 三态幂等——Marked/AlreadyRead 均 200，
/// NotFound → 404 not_found。
async fn mark_alert_read_handler(State(st): State<AdminState>, Path(id): Path<i64>) -> Response {
    let store = Arc::clone(&st.store);
    match tokio::task::spawn_blocking(move || store.mark_alert_read(id)).await {
        Ok(Ok(MarkReadOutcome::Marked | MarkReadOutcome::AlreadyRead)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "read": true })),
        )
            .into_response(),
        Ok(Ok(MarkReadOutcome::NotFound)) => err_body(StatusCode::NOT_FOUND, ERR_NOT_FOUND),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "mark alert read failed");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
        Err(e) => {
            tracing::error!(error = %e, "mark_alert_read spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}

/// GET /api/quota：snapshots 为每 (upstream,metric) 最新值；
/// sources 反映采集器健康（ok/disabled/error/unsupported_plan），spec §8。
async fn quota_handler(State(st): State<AdminState>) -> Response {
    let store = Arc::clone(&st.store);
    match tokio::task::spawn_blocking(move || store.latest_quota_snapshots()).await {
        Ok(Ok(snapshots)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "snapshots": snapshots, // QuotaSnapshotRow 已 Serialize
                "sources": st.monitor.statuses(),
            })),
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "quota snapshots query failed");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
        Err(e) => {
            tracing::error!(error = %e, "latest_quota_snapshots spawn_blocking panic");
            err_body(StatusCode::INTERNAL_SERVER_ERROR, ERR_INTERNAL)
        }
    }
}
