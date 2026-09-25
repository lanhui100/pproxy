//! Pony Proxy 嵌入式核心引擎 (pproxy-engine)。
//!
//! 提供支持 Basic Auth、Token Auth、防爆破限流、CONNECT 强制鉴权与双模上游的本地网关。

pub mod auth;
pub mod connect;
pub mod gateway;
pub mod server;
pub mod upstream;

pub use auth::{AuthContext, AuthSubject};
pub use connect::{TunnelConfig, TunnelPool};
pub use gateway::{build_data_router, GatewayState};
pub use server::{generate_instance_uuid, run_engine, EngineConfig};
pub use upstream::{RemoteProxyConfig, UpstreamManager, UpstreamMode};

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use pproxy_core::gatekeeper::AuthGatekeeper;
    use pproxy_core::route::RouteTable;
    use pproxy_core::store::Store;
    use pproxy_core::token::TokenService;
    use pproxy_core::usage::UsageTracker;
    use pproxy_core::user::UserService;
    use tower::ServiceExt;

    use super::*;

    fn setup_test_state() -> (GatewayState, Arc<Store>, Arc<UserService>, Arc<TokenService>) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.db");
        let (store, _) = Store::open(&db_path).unwrap();
        let store = Arc::new(store);

        let tokens = Arc::new(TokenService::new(store.clone()).unwrap());
        let users = Arc::new(UserService::new(store.clone()).unwrap());
        let edges = Arc::new(HashMap::new());
        let routes = Arc::new(RouteTable::new(store.clone(), edges).unwrap());
        let usage = Arc::new(UsageTracker::new(store.clone()));
        let gatekeeper = Arc::new(AuthGatekeeper::default());
        let upstream = Arc::new(UpstreamManager::new_direct(HashMap::new(), routes.clone()));
        let instance_uuid = generate_instance_uuid();

        let state = GatewayState {
            tokens: tokens.clone(),
            users: Some(users.clone()),
            upstream,
            usage,
            gatekeeper,
            tunnel: None,
            instance_uuid,
            cluster_auth_key: None,
        };

        (state, store, users, tokens)
    }

    #[tokio::test]
    async fn health_endpoint_returns_instance_uuid() {
        let (state, _, _, _) = setup_test_state();
        let expected_uuid = state.instance_uuid.clone();
        let router = build_data_router(state);

        let req = Request::builder()
            .uri("/__pproxy_health")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-pproxy-instance-uuid").unwrap(),
            expected_uuid.as_str()
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains(&expected_uuid));
    }

    #[tokio::test]
    async fn unauthorized_request_returns_401_or_407() {
        let (state, _, _, _) = setup_test_state();
        let router = build_data_router(state);

        // 注入非回环客户端地址（10.0.0.99），确保不命中"回环免认证"豁免路径
        let client_addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 99)), 12345);

        // 1. 无凭据请求普通路径 -> 401
        let req = Request::builder()
            .uri("/openai/v1/chat/completions")
            .body(Body::empty())
            .unwrap();
        let mut req = req;
        req.extensions_mut().insert(client_addr);
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2. 携带错误 Proxy-Authorization -> 407
        let req_proxy = Request::builder()
            .uri("/openai/v1/chat/completions")
            .header("proxy-authorization", "Basic aW52YWxpZDp3cm9uZw==")
            .body(Body::empty())
            .unwrap();
        let mut req_proxy = req_proxy;
        req_proxy.extensions_mut().insert(client_addr);
        let resp_proxy = router.oneshot(req_proxy).await.unwrap();
        assert_eq!(resp_proxy.status(), StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }

    #[tokio::test]
    async fn basic_auth_user_reaches_route_layer() {
        let (state, _, users, _) = setup_test_state();
        // 创建测试用户
        users
            .create_user("testuser", "CorrectPassword123", None)
            .unwrap();

        let router = build_data_router(state);

        // 使用 base64(testuser:CorrectPassword123) = dGVzdHVzZXI6Q29ycmVjdFBhc3N3b3JkMTIz
        let req = Request::builder()
            .uri("/openai/v1/models")
            .header(
                "proxy-authorization",
                "Basic dGVzdHVzZXI6Q29ycmVjdFBhc3N3b3JkMTIz",
            )
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        // 鉴权通过后到达路由层，因为未配置 openai 路由，返回 404 (route_not_found)，而非 401/407
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn gatekeeper_locks_out_after_repeated_failures() {
        let (state, _, _, _) = setup_test_state();
        let client_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 99));

        // 模拟连续 5 次错误密码
        for _ in 0..5 {
            state.gatekeeper.record_failure(client_ip);
        }

        // 第 6 次请求应该被拦截为 429
        let router = build_data_router(state);
        let mut req = Request::builder()
            .uri("/openai/v1/models")
            .header("proxy-authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(std::net::SocketAddr::new(client_ip, 12345));

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
