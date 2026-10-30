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
        assert!(resp_clash.headers().contains_key("subscription-userinfo"));
        assert_eq!(
            resp_clash.headers().get("profile-update-interval").and_then(|h| h.to_str().ok()),
            Some("24")
        );
        let clash_bytes = axum::body::to_bytes(resp_clash.into_body(), 1024 * 1024).await.unwrap();
        let clash_yaml = String::from_utf8(clash_bytes.to_vec()).unwrap();
        assert!(clash_yaml.contains("name: \"Pony-Tunnel\""));
        assert!(clash_yaml.contains("type: http"));
        assert!(!clash_yaml.contains("type: ws"), "Clash Meta 严禁非法 type: ws 出站代理");
        assert!(clash_yaml.contains("tls: true"));
        assert!(clash_yaml.contains("server: rn.ponygo.fun"));
        // C3：出站节点走公网 8443 标准 CONNECT + HTTP Basic；严禁 Bearer（CONNECT 不接受）
        assert!(clash_yaml.contains("port: 8443"), "C3：公网 CONNECT 数据面在 8443");
        assert!(!clash_yaml.contains("port: 443"), "C3：443 后面没有 CONNECT 能力");
        assert!(
            clash_yaml.contains(&format!("username: \"{}\"", token)),
            "C3：凭据走 HTTP Basic username"
        );
        assert!(
            clash_yaml.contains(&format!("password: \"{}\"", token)),
            "C3：凭据走 HTTP Basic password"
        );
        assert!(
            !clash_yaml.contains("Authorization: \"Bearer"),
            "C3：严禁输出 Bearer 死配置"
        );
        assert!(clash_yaml.contains("DOMAIN-SUFFIX,openai.com,PROXY"));
        // C6：兜底默认走代理；探活域与内网 CIDR 必须排在第一条 PROXY 规则之前
        assert!(
            clash_yaml.trim_end().ends_with("- MATCH,PROXY"),
            "C6：兜底规则必须为 MATCH,PROXY"
        );
        assert!(!clash_yaml.contains("MATCH,DIRECT"), "C6：严禁回落 MATCH,DIRECT");
        let first_proxy_idx = clash_yaml.find(",PROXY").expect("应有 PROXY 规则");
        for direct in [
            "- DOMAIN,connectivitycheck.gstatic.com,DIRECT",
            "- DOMAIN,connectivitycheck.android.com,DIRECT",
            "- DOMAIN,clients3.google.com,DIRECT",
            "- DOMAIN,msftconnecttest.com,DIRECT",
            "- DOMAIN,captive.apple.com,DIRECT",
            "- DOMAIN,cp.cloudflare.com,DIRECT",
            "- IP-CIDR,127.0.0.0/8,DIRECT,no-resolve",
            "- IP-CIDR,172.16.0.0/12,DIRECT,no-resolve",
            "- IP-CIDR,192.168.0.0/16,DIRECT,no-resolve",
            "- IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
        ] {
            let idx = clash_yaml.find(direct).unwrap_or_else(|| panic!("缺少直连规则 {direct}"));
            assert!(idx < first_proxy_idx, "C6：{direct} 必须排在第一条 PROXY 规则之前");
        }

        // C5：订阅端点鉴权——缺失 / 无效令牌必须 401，不再是无鉴权的假成功信号
        let req_no_token = Request::builder()
            .uri("/clash")
            .body(Body::empty())
            .unwrap();
        let resp_no_token = app.clone().oneshot(req_no_token).await.unwrap();
        assert_eq!(resp_no_token.status(), StatusCode::UNAUTHORIZED, "C5：缺失令牌必须 401");
        let body_no_token = axum::body::to_bytes(resp_no_token.into_body(), 1024 * 1024).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body_no_token).contains("missing token"),
            "C5：缺失令牌 body 需明示 missing token"
        );

        let req_bad_token = Request::builder()
            .uri("/api/clash?token=probe")
            .body(Body::empty())
            .unwrap();
        let resp_bad_token = app.clone().oneshot(req_bad_token).await.unwrap();
        assert_eq!(resp_bad_token.status(), StatusCode::UNAUTHORIZED, "C5：无效令牌必须 401");
        let body_bad_token = axum::body::to_bytes(resp_bad_token.into_body(), 1024 * 1024).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body_bad_token).contains("invalid token"),
            "C5：无效令牌 body 需明示 invalid token"
        );

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
