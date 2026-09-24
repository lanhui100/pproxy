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

    tracing::info!("Starting native Rust Gate Server on {}", addr);
    pproxy_gate_server::run_server(addr, token_hash, proxy_secret).await
}
