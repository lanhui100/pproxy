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
    // 支持逗号分隔的多公钥：首项主验签，其余为轮换窗口内的历史公钥（B021）
    let (verifier, legacy_verifiers) = pproxy_gate_server::verifiers_from_env();

    tracing::info!("Starting native Rust Gate Server on {}", addr);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .pool_max_idle_per_host(32)
        .build()?;

    let revoked_tokens = std::sync::Arc::new(dashmap::DashSet::new());
    pproxy_gate_server::load_revoked_tokens(&revoked_tokens);

    let user_used_bytes = std::sync::Arc::new(dashmap::DashMap::new());
    // 用量账本：启动恢复 + 每 10s 落盘并检查账单周期滚动（ADR 2026-11-05）
    let usage_manager = std::sync::Arc::new(pproxy_gate_server::usage::UsageManager::new(
        pproxy_gate_server::usage::default_usage_path(),
        pproxy_gate_server::usage::get_billing_day(),
        user_used_bytes.clone(),
    ));
    usage_manager.load_or_init();
    usage_manager.clone().spawn_periodic_sync();

    let state = std::sync::Arc::new(pproxy_gate_server::ServerConfig {
        tunnel_token_hash: token_hash,
        proxy_secret,
        client,
        verifier,
        legacy_verifiers,
        user_active_conns: std::sync::Arc::new(dashmap::DashMap::new()),
        user_used_bytes,
        revoked_tokens,
        gate_admin_token,
        usage_manager: Some(usage_manager.clone()),
        short_links: std::sync::Arc::new(pproxy_gate_server::shortlink::ShortLinkStore::open(
            pproxy_gate_server::shortlink::ShortLinkStore::default_path(),
        )),
    });

    let app = pproxy_gate_server::build_router(state.clone());

    // 契约 C1：标准 HTTP CONNECT 正向代理裸 TCP 监听（env GATE_CONNECT_LISTEN，未设置则不启动）。
    // 模块已移入 lib（`pproxy_gate_server::connect_proxy`），与 CLI 的 run_server 路径同源。
    pproxy_gate_server::connect_proxy::spawn_from_env(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(pproxy_gate_server::usage::wait_for_exit_signal())
    .await?;
    // 退出前强制落盘：否则最后 ≤10s 的计量会随进程消失（ADR 2026-11-05）
    usage_manager.flush_now();
    tracing::info!("Gate Server stopped, usage ledger flushed");
    Ok(())
}
