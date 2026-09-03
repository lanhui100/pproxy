//! `pproxy serve` — 单机独立前台起服与守护进程（嵌入式网关运行时）。
//!
//! 具备端口级跨进程排他文件锁、端口自检、优雅退出、双端口（数据面+管理面）合一与局域网网络提示。

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use fs2::FileExt;
use pproxy_core::gatekeeper::AuthGatekeeper;
use pproxy_core::route::RouteTable;
use pproxy_core::store::default_db_path;
use pproxy_core::token::TokenService;
use pproxy_core::usage::UsageTracker;
use pproxy_core::user::UserService;
use pproxy_core::{EdgeClient, PoolConfig, Store};
use pproxy_engine::connect::{TunnelConfig, TunnelPool};
use pproxy_engine::{
    generate_instance_uuid, run_engine, EngineConfig, GatewayState, UpstreamManager,
};
use pproxy_server::api::{admin_router, AdminState};
use pproxy_server::monitor::{spawn_monitor, MonitorConfig};
use pproxy_server::tunnel::TunnelProvision;
use rand::RngCore as _;

use crate::config;
use crate::{EXIT_FAILURE, EXIT_OK};

fn get_lock_path_for_port(port: u16) -> PathBuf {
    let db_path = default_db_path();
    let parent = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    parent.join(format!("pproxy_{port}.lock"))
}

/// 自动探测本机在局域网中的真实内网 IP 地址（例如 192.168.x.x）
pub fn get_local_lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").or_else(|_| socket.connect("1.1.1.1:80")).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// 解析最终数据面监听地址（优先级：显式 --listen > --lan/--port 组合 > 默认 127.0.0.1:8899）
pub fn resolve_listen_addr(listen_addr: Option<&str>, lan: bool, port: Option<u16>) -> String {
    if let Some(addr) = listen_addr {
        return addr.to_string();
    }
    let port = port.unwrap_or(8899);
    if lan {
        format!("0.0.0.0:{port}")
    } else {
        format!("127.0.0.1:{port}")
    }
}

/// 解析最终管理面监听地址（优先级：显式 --admin-listen > PPROXY_LISTEN_ADMIN 环境变量 > --lan 模式 0.0.0.0:8900 > 默认 127.0.0.1:8900）
pub fn resolve_admin_listen_addr(admin_listen: Option<&str>, lan: bool) -> String {
    if let Some(addr) = admin_listen {
        return addr.to_string();
    }
    if let Ok(env_addr) = std::env::var("PPROXY_LISTEN_ADMIN") {
        let trimmed = env_addr.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if lan {
        "0.0.0.0:8900".to_string()
    } else {
        "127.0.0.1:8900".to_string()
    }
}

pub fn run(
    listen_addr: Option<&str>,
    admin_listen: Option<&str>,
    lan: bool,
    port: Option<u16>,
) -> Result<i32, String> {
    let addr = resolve_listen_addr(listen_addr, lan, port);
    let admin_addr = resolve_admin_listen_addr(admin_listen, lan);

    let port: u16 = addr
        .split(':')
        .next_back()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8899);

    let admin_port: u16 = admin_addr
        .split(':')
        .next_back()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8900);

    // 1. 端口级跨进程排他文件锁（支持多端口多实例，同时同一端口互斥）
    let lock_path = get_lock_path_for_port(port);
    if let Some(dir) = lock_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let open_res = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&lock_path);

    let lock_file = match open_res {
        Ok(f) => f,
        Err(e) => {
            // Windows 下若已有进程以独占模式打开，open 可能直接返回 Sharing Violation
            eprintln!("\n┌─ [ERROR] 端口 {port} 服务启动冲突 ──────────────────────────");
            eprintln!("│ 无法访问实例锁文件 ({e})：已有 pproxy 实例正在监听端口 {port}。");
            eprintln!("│ ");
            eprintln!("│ 👉 若需停止后台进程，请在原终端按 Ctrl+C，或运行: pproxy stop");
            eprintln!("│ 👉 若需启动另一前台实例，请指定新端口: pproxy serve --listen 127.0.0.1:{}", port + 1);
            eprintln!("└─────────────────────────────────────────────────────────────\n");
            return Ok(EXIT_FAILURE);
        }
    };

    if lock_file.try_lock_exclusive().is_err() {
        eprintln!("\n┌─ [ERROR] 端口 {port} 服务启动冲突 ──────────────────────────");
        eprintln!("│ 无法获取实例锁：已有 pproxy 实例正在监听端口 {port}。");
        eprintln!("│ ");
        eprintln!("│ 👉 若需停止后台进程，请在原终端按 Ctrl+C，或运行: pproxy stop");
        eprintln!("│ 👉 若需启动另一前台实例，请指定新端口: pproxy serve --listen 127.0.0.1:{}", port + 1);
        eprintln!("└─────────────────────────────────────────────────────────────\n");
        return Ok(EXIT_FAILURE);
    }

    // 成功获取锁后，安全截断并记录当前 PID 与监听地址
    let mut file = lock_file;
    let _ = file.set_len(0);
    let _ = writeln!(file, "pid={}\naddr={addr}\nadmin_addr={admin_addr}", std::process::id());
    let _ = file.flush();

    // 2. 异步运行时启动
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Tokio runtime 初始化失败: {e}"))?;

    rt.block_on(async move {
        let db_path = default_db_path();
        let (store, is_fresh) = Store::open(&db_path).map_err(|e| e.to_string())?;
        let store = Arc::new(store);

        let mut cfg = config::load().unwrap_or_default();
        let mut config_modified = false;

        if cfg.server.is_empty() {
            cfg.server = format!("http://127.0.0.1:{admin_port}");
            config_modified = true;
        }

        // 若本地未配置 admin token 且环境变量无注入，则检查或预先生成一个并持久化，使 CLI 命令无缝免配置直连
        if cfg.admin_token.is_empty() && std::env::var("PPROXY_ADMIN_TOKEN").is_err() {
            let has_admin = store
                .list_tokens()
                .map(|rows| rows.iter().any(|r| r.name == pproxy_core::token::ADMIN_NAME && r.revoked_at.is_none()))
                .unwrap_or(false);
            if !has_admin {
                let mut bytes = [0u8; 24];
                rand::thread_rng().fill_bytes(&mut bytes);
                let new_token = format!("pony_admin_{}", hex::encode(bytes));
                std::env::set_var("PPROXY_ADMIN_TOKEN", &new_token);
                cfg.admin_token = new_token;
                config_modified = true;
            }
        }
        if config_modified {
            let _ = config::save(&cfg);
        }

        let tokens = Arc::new(TokenService::new(store.clone()).map_err(|e| e.to_string())?);
        let users = Arc::new(UserService::new(store.clone()).map_err(|e| e.to_string())?);
        let usage = Arc::new(UsageTracker::new(store.clone()));
        let gatekeeper = Arc::new(AuthGatekeeper::default());

        // 定时用量落库后台任务（每小时持久化）
        {
            let usage = Arc::clone(&usage);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                interval.tick().await; // 第一次立即触发，跳过
                loop {
                    interval.tick().await;
                    if let Err(e) = usage.flush().await {
                        tracing::warn!(error = %e, "usage flush failed");
                    }
                }
            });
        }

        let mut edges = HashMap::new();
        if let Some(secret) = &cfg.proxy_secret {
            if let Ok(cf_edge) = EdgeClient::new("https://edge.ponyjob.top", secret) {
                edges.insert("worker".to_string(), cf_edge);
            }
            if let Ok(vercel_edge) = EdgeClient::new("https://vedge.ponyjob.top/api/proxy", secret) {
                edges.insert("vercel".to_string(), vercel_edge);
            }
        }

        let edges_arc = Arc::new(edges.clone());
        let routes = Arc::new(RouteTable::new(store.clone(), edges_arc).map_err(|e| e.to_string())?);
        let upstream = Arc::new(UpstreamManager::new_direct(edges, routes.clone()));

        let (tunnel_gate_url, tunnel_token) = config::get_tunnel_config(&cfg);
        let tunnel_allowlist = std::env::var("PPROXY_TUNNEL_ALLOWLIST").ok();

        let pool_config = PoolConfig {
            worker_url: Some("https://edge.ponyjob.top".to_string()),
            worker_secret: cfg.proxy_secret.clone(),
            ..Default::default()
        };

        let tunnel_cfg = match (tunnel_gate_url.as_deref(), tunnel_token.as_deref()) {
            (Some(u), Some(t)) => {
                let custom = tunnel_allowlist.as_deref().map(|s| {
                    s.split(',').map(str::trim).collect::<Vec<_>>()
                });
                Some(TunnelConfig::build(u, t, custom.as_deref()))
            }
            _ => TunnelConfig::from_pool_config_and_env(&pool_config),
        };
        let tunnel_pool = tunnel_cfg.map(TunnelPool::new);

        let instance_uuid = generate_instance_uuid();

        // 组装管理面 (Admin API)
        let monitor_cfg = MonitorConfig::from_env();
        let monitor = spawn_monitor(Arc::clone(&store), monitor_cfg);
        let tunnel_provision = TunnelProvision::from_pool_config_and_env(&pool_config);

        let admin_state = AdminState {
            tokens: Arc::clone(&tokens),
            routes: Arc::clone(&routes),
            usage: Arc::clone(&usage),
            store: Arc::clone(&store),
            monitor,
            tunnel: tunnel_provision,
        };
        let admin_router = admin_router(admin_state);

        let admin_listener = match tokio::net::TcpListener::bind(&admin_addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("\n┌─ [ERROR] 管理面端口绑定失败 ({admin_addr}) ─────────────────");
                eprintln!("│ 无法监听管理面地址 ({e})。");
                eprintln!("│ 👉 若需指定其他管理面端口，请添加: --admin-listen 127.0.0.1:{}", admin_port + 1);
                eprintln!("└─────────────────────────────────────────────────────────────\n");
                return Ok(EXIT_FAILURE);
            }
        };

        // 注册退出信号
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut admin_rx = shutdown_rx.clone();

        tokio::spawn(async move {
            let _ = axum::serve(admin_listener, admin_router)
                .with_graceful_shutdown(async move {
                    let _ = admin_rx.changed().await;
                })
                .await;
        });

        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            println!("\n接收到终止信号，正在优雅关闭网关服务...");
            let _ = shutdown_tx.send(true);
        });

        let state = GatewayState {
            tokens,
            users: Some(users),
            upstream,
            usage,
            gatekeeper,
            tunnel: tunnel_pool.clone(),
            instance_uuid: instance_uuid.clone(),
        };

        let engine_config = EngineConfig {
            listen_addr: addr.clone(),
            instance_uuid: instance_uuid.clone(),
            max_connections: 512,
        };

        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║            Pony Proxy 嵌入式独立网关 (双面合一 v0.4.0)          ║");
        println!("╚════════════════════════════════════════════════════════════════╝");
        println!("  数据面 (HTTP/HTTPS 代理):  http://{addr}");
        println!("  管理面 (Admin REST API):   http://{admin_addr}");
        println!("  实例签名: {instance_uuid}");
        if let Some(pool) = &tunnel_pool {
            println!("  出海隧道: \x1b[1;32m已就绪\x1b[0m (Gate: {})", pool.config().gate_url);
        } else {
            println!("  出海隧道: \x1b[1;33m未配置\x1b[0m (可通过 pproxy config set-tunnel 配置 Gate 端点)");
        }
        if is_fresh {
            println!("  数据库:   全新创建 ({})", db_path.display());
        }

        if addr.starts_with("0.0.0.0") {
            let lan_ip = get_local_lan_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "本机局域网IP".to_string());
            println!("\n\x1b[1;33m⚠ 局域网共享模式已开启：\x1b[0m");
            println!("  数据面 (局域网设备代理):   \x1b[1;32mhttp://{lan_ip}:{port}\x1b[0m");
            println!("  管理面 (远程/局域网管理):   \x1b[1;36mhttp://{lan_ip}:{admin_port}\x1b[0m");
            println!("  手机/平板连接同一 WiFi 后，代理主机填入 {lan_ip}，端口填入 {port} 即可。");
            println!("  提示: 若手机无法连接，请放行防火墙端口 (如: netsh advfirewall / sudo ufw allow {port}/tcp)。");
        } else {
            println!("\n  访问模式: 本机独享 (127.0.0.1)");
            println!("  提示: 如需供同局域网手机/设备使用，请使用快捷选项: \x1b[1;36mpproxy serve --lan\x1b[0m");
        }

        println!("\n✓ 代理与管理服务已全功能就绪！按 Ctrl+C 退出服务。\n");

        if let Err(e) = run_engine(engine_config, state, Some(shutdown_rx)).await {
            let err_str = e.to_string();
            eprintln!("服务运行异常退出: {err_str}");
            if err_str.contains("Address already in use")
                || err_str.contains("Address in use")
                || err_str.contains("os error 98")
                || err_str.contains("os error 10048")
            {
                eprintln!("\n💡 提示: 端口 {port} 已被占用！");
                eprintln!("   1. 若您此前在后台启动了守护服务，请先运行: \x1b[1;36mpproxy stop\x1b[0m 停止后台服务。");
                eprintln!("   2. 或指定其他空闲端口启动: \x1b[1;36mpproxy serve -g -p {}\x1b[0m\n", port + 1);
            }
            return Ok(EXIT_FAILURE);
        }

        Ok(EXIT_OK)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_path_format() {
        let path = get_lock_path_for_port(9999);
        assert!(path.to_string_lossy().ends_with("pproxy_9999.lock"));
    }

    #[test]
    fn test_file_lock_mutual_exclusion() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("test_serve.lock");

        let file1 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
            .unwrap();

        assert!(file1.try_lock_exclusive().is_ok());

        let file2 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
            .unwrap();

        // 另一个句柄尝试获取排他锁应该失败
        assert!(file2.try_lock_exclusive().is_err());

        // 释放锁 1 后，锁 2 可以获取
        drop(file1);
        assert!(file2.try_lock_exclusive().is_ok());
    }

    #[test]
    fn test_resolve_listen_addr() {
        // 默认 127.0.0.1:8899
        assert_eq!(resolve_listen_addr(None, false, None), "127.0.0.1:8899");
        // 自定义端口
        assert_eq!(resolve_listen_addr(None, false, Some(9000)), "127.0.0.1:9000");
        // 开启局域网共享 --lan
        assert_eq!(resolve_listen_addr(None, true, None), "0.0.0.0:8899");
        assert_eq!(resolve_listen_addr(None, true, Some(9000)), "0.0.0.0:9000");
        // 显式 --listen 优先级最高
        assert_eq!(
            resolve_listen_addr(Some("192.168.1.5:8080"), true, Some(9000)),
            "192.168.1.5:8080"
        );
    }

    #[test]
    fn test_resolve_admin_listen_addr() {
        // 默认 127.0.0.1:8900
        assert_eq!(resolve_admin_listen_addr(None, false), "127.0.0.1:8900");
        // 开启局域网共享 --lan
        assert_eq!(resolve_admin_listen_addr(None, true), "0.0.0.0:8900");
        // 显式 --admin-listen 优先级最高
        assert_eq!(
            resolve_admin_listen_addr(Some("192.168.1.100:9900"), true),
            "192.168.1.100:9900"
        );
    }

    #[tokio::test]
    async fn test_serve_admin_router_integration() {
        use tower::ServiceExt;

        let test_admin_token = "pony_admin_integration_test_secret_123";
        std::env::set_var("PPROXY_ADMIN_TOKEN", test_admin_token);

        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_serve.db");
        let (store, _) = Store::open(&db_path).unwrap();
        let store = Arc::new(store);
        let tokens = Arc::new(TokenService::new(store.clone()).unwrap());
        let routes = Arc::new(RouteTable::new(store.clone(), Arc::new(HashMap::new())).unwrap());
        let usage = Arc::new(UsageTracker::new(store.clone()));
        let monitor = spawn_monitor(store.clone(), MonitorConfig::from_env());

        let admin_state = AdminState {
            tokens: tokens.clone(),
            routes,
            usage,
            store,
            monitor,
            tunnel: None,
        };
        let app = admin_router(admin_state);

        // 携带 Admin Token 访问 /api/health 端点
        let req = axum::http::Request::builder()
            .uri("/api/health")
            .header("Authorization", format!("Bearer {test_admin_token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}

