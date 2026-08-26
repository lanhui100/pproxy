//! pproxy-server 入口（T3 §5 装配顺序，硬性约定）：
//! 1. 环境变量覆盖 → 2. 读 config.json → 3. EdgeClient map（先于迁移）→
//! 4. Store::open + migrate_config_if_needed → 5. TokenService/RouteTable/UsageTracker →
//! 6. usage 落库 interval task → 7. 数据面 + 管理面双端口 serve → 8. admin 非回环 warn。

mod api;
mod dsk;
mod gateway;
mod monitor;
mod tunnel;

use std::collections::HashMap;
use std::sync::Arc;

use pproxy_core::{EdgeClient, PoolConfig};
use tokio::net::TcpListener;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 1. 环境变量覆盖（PPROXY_DB 在 core::default_db_path 内解析）
    let config_path = std::env::var("PPROXY_CONFIG").unwrap_or_else(|_| "/home/USER/pproxy/config.json".into());
    let listen_data = std::env::var("PPROXY_LISTEN_DATA").unwrap_or_else(|_| String::new());
    let listen_admin = std::env::var("PPROXY_LISTEN_ADMIN").unwrap_or_else(|_| "127.0.0.1:8900".into());

    // 2. 读完整旧 config.json（容忍迁移后格式——无 routes 键但凭据/上游项保留）
    let config: PoolConfig = if std::path::Path::new(&config_path).exists() {
        serde_json::from_str(&tokio::fs::read_to_string(&config_path).await?)?
    } else {
        PoolConfig::default()
    };

    // 3. 构造 EdgeClient map（先于迁移——迁移重写 config.json 仅删 routes 键，
    //    凭据已在此持有，双保险）
    let mut edges: HashMap<String, EdgeClient> = HashMap::new();
    if let (Some(url), Some(secret)) = (&config.worker_url, &config.worker_secret) {
        edges.insert("worker".into(), EdgeClient::new(url, secret)?);
    }
    for (name, up) in &config.upstreams {
        edges.insert(name.clone(), EdgeClient::new(&up.url, &up.secret)?);
    }
    if edges.is_empty() {
        warn!("no upstreams configured, edge forwarding disabled");
    }
    let edges = Arc::new(edges);

    // 4. Store 打开 + 一次性迁移（routes 键 → routes 表）
    let db_path = pproxy_core::store::default_db_path();
    let (store, created) = pproxy_core::Store::open(&db_path)?;
    info!(db = %db_path.display(), created, "store opened");
    match store.migrate_config_if_needed(std::path::Path::new(&config_path))? {
        pproxy_core::store::MigrationOutcome::Imported(n) => info!("migrated {n} routes from config.json"),
        pproxy_core::store::MigrationOutcome::AlreadyMigrated => {}
        pproxy_core::store::MigrationOutcome::NoLegacyRoutes => {}
    }
    let store = Arc::new(store);

    // 5. 三大服务（TokenService::new 含 admin 引导，ADMIN_TOKEN 经 warn 打印一次）
    let tokens = Arc::new(pproxy_core::TokenService::new(Arc::clone(&store))?);
    let routes = Arc::new(pproxy_core::RouteTable::new(Arc::clone(&store), Arc::clone(&edges))?);
    let usage = Arc::new(pproxy_core::UsageTracker::new(Arc::clone(&store)));

    // 6. usage 落库循环（T5 §6）：首 tick 立即触发，丢弃后再进入循环
    {
        let usage = Arc::clone(&usage);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // 第一次立即触发，跳过
            loop {
                interval.tick().await;
                if let Err(e) = usage.flush().await {
                    warn!(error = %e, "usage flush failed (merged back to live)");
                }
            }
        });
    }

    // 5.5 M3 监控轮询：环境变量配置（config.json 零改动），缺失来源 disabled
    //     仅 info 一行非错误；spawn 后句柄交 AdminState 供 /api/quota 读健康状态。
    //     首 tick 立即采集且不丢弃（启动即采一轮）。
    let monitor_cfg = monitor::MonitorConfig::from_env();
    monitor_cfg.log_disabled();
    let monitor = monitor::spawn_monitor(Arc::clone(&store), monitor_cfg);

    // 7a. 数据面 listener + accept 循环（CONNECT 拦截 + http1::Builder）
    let data_addr = if listen_data.is_empty() {
        format!("{}:{}", config.listen_host, config.listen_port)
    } else {
        listen_data
    };
    let data_listener = TcpListener::bind(&data_addr).await?;
    info!("data plane listening on {data_addr}");

    // 8. 管理面绑定非回环地址时 warn（S-P2-额外 裁决：提示暴露面扩大，不阻止启动）
    let admin_host = listen_admin.rsplit_once(':').map(|(h, _)| h).unwrap_or("").to_string();
    if !is_loopback_host(&admin_host) {
        warn!(addr = %listen_admin, "admin plane bound to non-loopback address; exposure widened");
    }

    // 7b. 管理面 serve（T6 Router）
    let admin_state = api::AdminState {
        tokens: Arc::clone(&tokens),
        routes: Arc::clone(&routes),
        usage: Arc::clone(&usage),
        store: Arc::clone(&store),
        monitor,
        tunnel: tunnel::TunnelProvision::from_env(),
    };
    let admin_router = api::admin_router(admin_state);
    let admin_listener = TcpListener::bind(&listen_admin).await?;
    info!("admin plane listening on {listen_admin}");
    let admin_task = tokio::spawn(async move {
        if let Err(e) = axum::serve(admin_listener, admin_router).await {
            warn!(error = %e, "admin server exited");
        }
    });

    // 数据面在当前线程驱动（主任务）；admin 已移交后台任务
    let gw_state = gateway::GatewayState {
        tokens,
        edges,
        routes,
        usage,
    };
    let router = gateway::data_router(gw_state);
    tokio::select! {
        r = gateway::serve_data_plane(data_listener, router) => r?,
        _ = admin_task => unreachable!("admin task never finishes"),
    }

    #[allow(unreachable_code)]
    Ok(())
}

/// 回环判定（第 8 步）：host 为 127.* / localhost / ::1 / [::1] 视为回环。
fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host == "[::1]" || host == "::1" || host.is_empty() {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}
