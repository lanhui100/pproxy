//! pproxy-server 入口（T3 §5 装配顺序，硬性约定）：
//! 1. 环境变量覆盖 → 2. 读 config.json → 3. EdgeClient map（先于迁移）→
//! 4. Store::open + migrate_config_if_needed → 5. TokenService/RouteTable/UsageTracker →
//! 6. usage 落库 interval task → 7. 数据面 + 管理面双端口 serve → 8. admin 非回环 warn。

use pproxy_server::{api, connect, gateway, is_loopback_host, monitor, should_keepalive_edge, tunnel};

use std::collections::HashMap;
use std::sync::Arc;

use pproxy_core::{EdgeClient, PoolConfig};
use tokio::net::TcpListener;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化 rustls CryptoProvider（tokio-tungstenite 0.24 + rustls 0.23 需要）
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("rustls CryptoProvider install failed");
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 1. 环境变量覆盖（PPROXY_DB 在 core::default_db_path 内解析）
    let config_path = std::env::var("PPROXY_CONFIG").unwrap_or_else(|_| "/etc/pproxy/config.json".into());
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

    // 5.5 CONNECT 隧道配置（pproxy-connect-tunnel spec §3.4）：PoolConfig 智能推导 + env 覆盖，fail-closed
    // 性能专项：包装为待命隧道池（预建 WS 会话，establish 从 ~5 RTT 压到 1 RTT）。
    // 运维回滚开关：PPROXY_TUNNEL_POOL=0 禁池化（退回每 CONNECT 冷建连的旧行为）。
    let tunnel_cfg = connect::TunnelConfig::from_pool_config_and_env(&config).map(|c| {
        if std::env::var("PPROXY_TUNNEL_POOL").ok().as_deref() == Some("0") {
            connect::TunnelPool::with_size(c, 0)
        } else {
            connect::TunnelPool::new(c)
        }
    });
    if let Some(pool) = &tunnel_cfg {
        let allowlist = pool.config().allowlist.join(", ");
        info!(gate_url = %pool.config().gate_url, allowlist = %allowlist, "CONNECT tunnel enabled via gate worker");
    }

    // 8. 管理面绑定非回环地址时 warn（S-P2-额外 裁决：提示暴露面扩大，不阻止启动）
    let admin_host = listen_admin.rsplit_once(':').map(|(h, _)| h).unwrap_or("").to_string();
    if !is_loopback_host(&admin_host) {
        warn!(addr = %listen_admin, "admin plane bound to non-loopback address; exposure widened");
    }
    // S-P2-数据面（pproxy-connect-tunnel 裁决）：数据面非回环绑定且隧道已配置 → warn
    let data_host = data_addr.rsplit_once(':').map(|(h, _)| h).unwrap_or("").to_string();
    if tunnel_cfg.is_some() && !is_loopback_host(&data_host) {
        // 数据面绑定非回环时 CONNECT 隧道即为无鉴权出口（spec §3.5 S-P2-数据面），
        // 仅 warn 不阻止——与 admin 面 S-P2-额外 一致。
        warn!(
            addr = %data_addr,
            "data plane bound to non-loopback address with CONNECT tunnel enabled; \
             tunnel hosts are reachable without authentication (S-P2-数据面)"
        );
    }

    // 7b. 管理面 serve（T6 Router）
    let admin_state = api::AdminState {
        tokens: Arc::clone(&tokens),
        routes: Arc::clone(&routes),
        usage: Arc::clone(&usage),
        store: Arc::clone(&store),
        monitor,
        tunnel: tunnel::TunnelProvision::from_pool_config_and_env(&config),
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
    // 边缘连接池保活（性能专项，env 开关 PPROXY_EDGE_KEEPALIVE=1）：
    // 间隔 45s < reqwest pool_idle_timeout 90s，保持到 edge 的 TLS 连接常驻，
    // 消除冷握手（跨洲 ~2-3 RTT）。默认关闭：仅对 CF Worker 保活，Vercel 上游
    // 每次 ping 计一次函数调用（约 1920 次/日）已自动排除（should_keepalive_edge）。
    if std::env::var("PPROXY_EDGE_KEEPALIVE").ok().as_deref() == Some("1") {
        let edges = Arc::clone(&edges);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(45)).await;
                for (name, edge) in edges.iter() {
                    if !should_keepalive_edge(name) {
                        continue;
                    }
                    if let Err(e) = edge.keepalive_ping().await {
                        tracing::debug!(upstream = %name, error = %e, "edge keepalive ping failed");
                    }
                }
            }
        });
        info!("edge keepalive enabled (PPROXY_EDGE_KEEPALIVE=1, interval 45s; Vercel 上游已安全排除以避免额度超限)");
    }

    let gw_state = gateway::GatewayState {
        tokens,
        edges,
        routes,
        usage,
        tunnel: tunnel_cfg,
    };
    tokio::select! {
        r = gateway::serve_data_plane(data_listener, gw_state) => r?,
        _ = admin_task => unreachable!("admin task never finishes"),
    }

    #[allow(unreachable_code)]
    Ok(())
}
