#[cfg(test)]
mod tests {
    use crate::{build_router, ServerConfig};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use pproxy_core::{TokenSigner, UserTokenClaims};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_gate_user_token_auth_flow() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = Arc::new(pproxy_core::TokenVerifier::new(vk));
        let active_conns = Arc::new(dashmap::DashMap::new());
        let used_bytes = Arc::new(dashmap::DashMap::new());

        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: "".into(),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: Some(verifier),
            user_active_conns: active_conns.clone(),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
        });

        let app = build_router(cfg.clone());

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = UserTokenClaims {
            jti: "jti-1".into(),
            sub: "usr_alice".into(),
            name: "Alice".into(),
            quota_bytes: 100 * 1024 * 1024,
            lease_bytes: 50 * 1024 * 1024,
            exp: now + 3600,
            iat: now,
            max_conns: 3,
        };
        let token = signer.sign_token(&claims).unwrap();

        // 1. 模拟请求 profile 接口
        let req = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["user"]["name"], "Alice");
        assert_eq!(json["user"]["quota_bytes"], 100 * 1024 * 1024);
        assert_eq!(json["user"]["status"], "active");

        // 2. 模拟超额情况
        used_bytes.insert("usr_alice".into(), std::sync::atomic::AtomicU64::new(100 * 1024 * 1024));
        let req_over = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        let resp_over = app.clone().oneshot(req_over).await.unwrap();
        let body_over = axum::body::to_bytes(resp_over.into_body(), 1024 * 1024).await.unwrap();
        let json_over: serde_json::Value = serde_json::from_slice(&body_over).unwrap();
        assert_eq!(json_over["user"]["status"], "quota_exceeded");
    }
}
