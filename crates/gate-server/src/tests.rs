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
            role: "user".into(),
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

        // 3. 验证 /clash 订阅端点对齐公网隧道与 token
        let req_clash = Request::builder()
            .uri(format!("/clash?token={}", token))
            .header("Host", "rn.ponygo.fun")
            .body(Body::empty())
            .unwrap();

        let resp_clash = app.clone().oneshot(req_clash).await.unwrap();
        assert_eq!(resp_clash.status(), StatusCode::OK);
        let clash_bytes = axum::body::to_bytes(resp_clash.into_body(), 1024 * 1024).await.unwrap();
        let clash_yaml = String::from_utf8(clash_bytes.to_vec()).unwrap();
        assert!(clash_yaml.contains("name: \"Pony-Tunnel\""));
        assert!(clash_yaml.contains("server: rn.ponygo.fun"));
        assert!(clash_yaml.contains(&format!("Authorization: \"Bearer {}\"", token)));
        assert!(clash_yaml.contains("DOMAIN-SUFFIX,openai.com,PROXY"));

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

    #[tokio::test]
    async fn test_admin_token_ignores_quota_limit() {
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
            jti: "jti-admin".into(),
            sub: "usr_admin".into(),
            name: "admin".into(),
            quota_bytes: 1,
            lease_bytes: 1,
            exp: now + 3600,
            iat: now,
            max_conns: 3,
            role: "admin".into(),
        };
        let token = signer.sign_token(&claims).unwrap();
        used_bytes.insert("usr_admin".into(), std::sync::atomic::AtomicU64::new(u64::MAX));

        let req = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["user"]["status"], "active");
    }

    /// ADR 2026-10-03：name 前缀启发式已废除——普通租户即使 name 为
    /// `admin_xxx`，role=user 时仍受配额熔断（防伪造 name 绕过豁免）。
    #[tokio::test]
    async fn test_name_admin_prefix_does_not_evade_quota() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = Arc::new(pproxy_core::TokenVerifier::new(vk));
        let used_bytes = Arc::new(dashmap::DashMap::new());

        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: "".into(),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: Some(verifier),
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
        });

        let app = build_router(cfg.clone());
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = UserTokenClaims {
            jti: "jti-admin-prefix".into(),
            sub: "usr_admin_bob".into(),
            name: "admin_bob".into(), // 恶意/误导性 name，但 role 是普通用户
            quota_bytes: 100,
            lease_bytes: 50,
            exp: now + 3600,
            iat: now,
            max_conns: 3,
            role: "user".into(),
        };
        let token = signer.sign_token(&claims).unwrap();
        used_bytes.insert("usr_admin_bob".into(), std::sync::atomic::AtomicU64::new(200)); // 已超额

        let req = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["user"]["status"], "quota_exceeded", "role=user 不得因 name 前缀豁免");
    }
}
