use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,pproxy_gate_server=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3101);

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;

    let token_hash = std::env::var("TUNNEL_TOKEN_HASH").unwrap_or_default();
    let proxy_secret = std::env::var("PROXY_SECRET").unwrap_or_default();
    let gate_admin_token = std::env::var("GATE_ADMIN_TOKEN").unwrap_or_default();
    let verifying_key_hex = std::env::var("USER_VERIFYING_KEY").ok();

    let verifier = if let Some(hex_key) = verifying_key_hex {
        match pproxy_core::TokenVerifier::from_hex(&hex_key) {
            Ok(v) => {
                tracing::info!("User Token Verifier enabled with public key: {}", hex_key);
                Some(std::sync::Arc::new(v))
            }
            Err(e) => {
                tracing::error!("Failed to parse USER_VERIFYING_KEY: {}", e);
                None
            }
        }
    } else {
        None
    };

    tracing::info!("Starting native Rust Gate Server on {}", addr);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .pool_max_idle_per_host(32)
        .build()?;

    let revoked_tokens = std::sync::Arc::new(dashmap::DashSet::new());
    pproxy_gate_server::load_revoked_tokens(&revoked_tokens);

    let state = std::sync::Arc::new(pproxy_gate_server::ServerConfig {
        tunnel_token_hash: token_hash,
        proxy_secret,
        client,
        verifier,
        user_active_conns: std::sync::Arc::new(dashmap::DashMap::new()),
        user_used_bytes: std::sync::Arc::new(dashmap::DashMap::new()),
        revoked_tokens,
        gate_admin_token,
    });

    let app = pproxy_gate_server::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(Into::into)
}
