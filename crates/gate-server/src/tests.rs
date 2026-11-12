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
            legacy_verifiers: Vec::new(),
            user_active_conns: active_conns.clone(),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: None,
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
        // 订阅头必须是真实口径：total/expire 来自令牌 claims，不再是硬编码 100GB/0
        let userinfo = resp_clash
            .headers()
            .get("subscription-userinfo")
            .and_then(|h| h.to_str().ok())
            .expect("必须返回 subscription-userinfo 头");
        assert_eq!(
            userinfo,
            format!(
                "upload=0; download=0; total={}; expire={}",
                claims.quota_bytes, claims.exp
            ),
            "订阅头 total/expire 必须与令牌 claims 一致"
        );
        assert!(
            !userinfo.contains("107374182400"),
            "严禁硬编码 100GB 配额：{userinfo}"
        );
        assert_eq!(
            resp_clash.headers().get("profile-update-interval").and_then(|h| h.to_str().ok()),
            Some("24")
        );
        // 响应体含长期令牌 → 必须禁止缓存
        assert_eq!(
            resp_clash.headers().get("cache-control").and_then(|h| h.to_str().ok()),
            Some("no-store"),
            "订阅响应含长期令牌，必须 cache-control: no-store"
        );
        let clash_bytes = axum::body::to_bytes(resp_clash.into_body(), 1024 * 1024).await.unwrap();
        let clash_yaml = String::from_utf8(clash_bytes.to_vec()).unwrap();
        assert!(clash_yaml.contains("name: \"Pony-Tunnel\""));
        assert!(clash_yaml.contains("type: http"));
        assert!(!clash_yaml.contains("type: ws"), "Clash Meta 严禁非法 type: ws 出站代理");
        assert!(clash_yaml.contains("tls: true"));
        assert!(clash_yaml.contains("server: rn.ponygo.fun"));
        // B3：组自检必须走 443 探活（CONNECT 数据面只放行 443），明文 80 探活必然全超时
        assert!(
            clash_yaml.contains("url: \"https://www.gstatic.com/generate_204\""),
            "B3：健康检查 URL 必须走 https/443"
        );
        assert!(
            !clash_yaml.contains("http://cp.cloudflare.com"),
            "B3：严禁明文 80 端口健康检查（CONNECT ACL 仅放行 443）"
        );
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
            legacy_verifiers: Vec::new(),
            user_active_conns: active_conns.clone(),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: None,
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
            legacy_verifiers: Vec::new(),
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: None,
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
        assert_eq!(json["user"]["unlimited"], false);
    }

    /// ADR 2026-11-05：Admin 账户 profile 映射 RN VPS 整机 500GB 配额与账单周期重置点，
    /// `used_bytes` 为当前账单周期内经持久化账本聚合的全体租户水位（服务端为唯一真相源）。
    #[tokio::test]
    async fn test_admin_profile_maps_vps_quota_and_billing_cycle() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = Arc::new(pproxy_core::TokenVerifier::new(vk));
        let used_bytes = Arc::new(dashmap::DashMap::new());

        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::usage::UsageManager::new(
            dir.path().join("gate_usage.json"),
            1,
            used_bytes.clone(),
        ));
        manager.load_or_init();
        used_bytes.insert("usr_a".into(), std::sync::atomic::AtomicU64::new(1_000));
        used_bytes.insert("usr_b".into(), std::sync::atomic::AtomicU64::new(2_000));
        manager.persist();

        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: "".into(),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: Some(verifier),
            legacy_verifiers: Vec::new(),
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: Some(manager),
        });

        let app = build_router(cfg.clone());
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = UserTokenClaims {
            jti: "jti-admin-vps".into(),
            sub: "usr_admin".into(),
            name: "admin".into(),
            quota_bytes: 1,
            lease_bytes: 1,
            exp: now + 3600,
            iat: now,
            max_conns: 8,
            role: "admin".into(),
        };
        let token = signer.sign_token(&claims).unwrap();

        let req = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["user"]["status"], "active", "管理员豁免配额熔断");
        assert_eq!(json["user"]["unlimited"], true);
        assert_eq!(json["user"]["role"], "admin");
        assert_eq!(json["user"]["quota_bytes"], crate::usage::VPS_TOTAL_QUOTA_BYTES);
        assert_eq!(json["user"]["used_bytes"], 3_000, "管理员看到的是整机聚合水位而非自身令牌额度");

        let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let (_, reset_ts, days_left) = crate::usage::compute_billing_cycle(after, 1);
        assert_eq!(json["user"]["expire_at"], reset_ts, "管理员到期时间语义为账单周期重置点");
        let got = json["user"]["days_left"].as_u64().unwrap();
        assert!(got == days_left || got + 1 == days_left, "days_left 应等于距账单重置的天数，实际 {got} vs {days_left}");
    }

    /// 订阅头必须反映服务端账本里该租户的真实已用量：移动端据此提示额度耗尽，
    /// 而不是永远显示“0 已用”。`/clash` 与 `/api/clash` 共用 handler，两条端点都要一致。
    #[tokio::test]
    async fn test_clash_subscription_userinfo_reflects_real_usage() {
        let (signer, vk) = TokenSigner::generate();
        let verifier = Arc::new(pproxy_core::TokenVerifier::new(vk));
        let used_bytes = Arc::new(dashmap::DashMap::new());

        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: "".into(),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: Some(verifier),
            legacy_verifiers: Vec::new(),
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: None,
        });

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let quota = 50 * 1024 * 1024 * 1024u64; // 50G 月度额度
        let claims = UserTokenClaims {
            jti: "jti-usage".into(),
            sub: "usr_bob".into(),
            name: "Bob".into(),
            quota_bytes: quota,
            lease_bytes: 50 * 1024 * 1024,
            exp: now + 30 * 86400,
            iat: now,
            max_conns: 3,
            role: "user".into(),
        };
        let token = signer.sign_token(&claims).unwrap();
        used_bytes.insert("usr_bob".into(), std::sync::atomic::AtomicU64::new(42));

        let expected = format!("upload=0; download=42; total={quota}; expire={}", claims.exp);
        for path in ["/clash", "/api/clash"] {
            let req = Request::builder()
                .uri(format!("{path}?token={token}"))
                .header("Host", "rn.ponygo.fun")
                .body(Body::empty())
                .unwrap();
            let resp = build_router(cfg.clone()).oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path} 应鉴权通过");
            assert_eq!(
                resp.headers().get("subscription-userinfo").and_then(|h| h.to_str().ok()),
                Some(expected.as_str()),
                "{path} 的 subscription-userinfo 必须反映真实用量与令牌额度/到期"
            );
        }
    }

    /// 单口令兼容模式没有租户语义：订阅头回落 VPS 整机基线与当前周期账本聚合用量。
    #[tokio::test]
    async fn test_clash_subscription_single_password_reports_vps_quota() {
        let used_bytes = Arc::new(dashmap::DashMap::new());
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::usage::UsageManager::new(
            dir.path().join("gate_usage.json"),
            1,
            used_bytes.clone(),
        ));
        used_bytes.insert("usr_a".into(), std::sync::atomic::AtomicU64::new(1_000));
        used_bytes.insert("usr_b".into(), std::sync::atomic::AtomicU64::new(234));

        let password = "single-pass";
        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: crate::sha256_hex(password),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: None,
            legacy_verifiers: Vec::new(),
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: used_bytes.clone(),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: Some(manager),
        });

        let req = Request::builder()
            .uri(format!("/clash?token={password}"))
            .header("Host", "rn.ponygo.fun")
            .body(Body::empty())
            .unwrap();
        let resp = build_router(cfg).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("subscription-userinfo").and_then(|h| h.to_str().ok()),
            Some(
                format!(
                    "upload=0; download=1234; total={}; expire=0",
                    crate::usage::VPS_TOTAL_QUOTA_BYTES
                )
                .as_str()
            ),
            "单口令模式应展示 VPS 整机基线与账本聚合用量"
        );
    }

    /// 密钥轮换双信任：旧私钥签发的令牌在 `legacy_verifiers` 窗口内仍可通过，
    /// 同时新私钥签发的令牌走主公钥 —— 两者并存，避免轮换时一次性全量失效（B021）。
    #[tokio::test]
    async fn test_legacy_verifying_key_still_trusted() {
        let (legacy_signer, legacy_vk) = TokenSigner::generate();
        let (_current_signer, current_vk) = TokenSigner::generate();

        let cfg = Arc::new(ServerConfig {
            tunnel_token_hash: "".into(),
            proxy_secret: "sec".into(),
            client: reqwest::Client::new(),
            verifier: Some(Arc::new(pproxy_core::TokenVerifier::new(current_vk))),
            legacy_verifiers: vec![Arc::new(pproxy_core::TokenVerifier::new(legacy_vk))],
            user_active_conns: Arc::new(dashmap::DashMap::new()),
            user_used_bytes: Arc::new(dashmap::DashMap::new()),
            revoked_tokens: Arc::new(dashmap::DashSet::new()),
            gate_admin_token: "test-admin".into(),
            usage_manager: None,
        });

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = UserTokenClaims {
            jti: "jti-legacy".into(),
            sub: "usr_legacy".into(),
            name: "Legacy".into(),
            quota_bytes: 1024 * 1024 * 1024,
            lease_bytes: 10 * 1024 * 1024,
            exp: now + 3600,
            iat: now,
            max_conns: 1,
            role: "user".into(),
        };
        let token = legacy_signer.sign_token(&claims).unwrap();

        let app = build_router(cfg);
        let req = Request::builder()
            .uri("/api/user/profile")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "历史公钥签发的令牌应仍在轮换窗口内通过"
        );
    }

    /// `USER_VERIFYING_KEY` 支持逗号分隔多公钥：首项为主、其余为历史；非法项跳过不致命。
    #[test]
    fn test_parse_verifying_keys_multi_and_invalid() {
        let (a_signer, a_vk) = TokenSigner::generate();
        let (_b_signer, b_vk) = TokenSigner::generate();
        let a_hex = hex::encode(a_vk.to_bytes());
        let b_hex = hex::encode(b_vk.to_bytes());
        let raw = format!("  {b_hex} , not-a-key , {a_hex} ");

        let (primary, legacy) = crate::parse_verifying_keys(&raw);
        let primary = primary.expect("首项应作为主公钥");
        assert_eq!(legacy.len(), 1, "非法项应被跳过，仅剩 1 个历史公钥");

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = UserTokenClaims {
            jti: "jti-parse".into(),
            sub: "usr_parse".into(),
            name: "Parse".into(),
            quota_bytes: 1024,
            lease_bytes: 512,
            exp: now + 60,
            iat: now,
            max_conns: 1,
            role: "user".into(),
        };
        let a_token = a_signer.sign_token(&claims).unwrap();
        assert!(
            primary.verify_token(&a_token).is_err(),
            "主公钥不应验通历史密钥签发的令牌"
        );
        assert!(
            legacy[0].verify_token(&a_token).is_ok(),
            "历史公钥应验通旧私钥签发的令牌"
        );
    }
}
