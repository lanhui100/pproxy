//! `pproxy serve` — 单机独立前台起服与守护进程（嵌入式网关运行时）。
//!
//! 具备端口级跨进程排他文件锁、端口自检、优雅退出与局域网网络提示。

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
use pproxy_engine::connect::TunnelConfig;
use pproxy_engine::{
    generate_instance_uuid, run_engine, EngineConfig, GatewayState, UpstreamManager,
};

use crate::config::load;
use crate::{EXIT_FAILURE, EXIT_OK};

fn get_lock_path_for_port(port: u16) -> PathBuf {
    let db_path = default_db_path();
    let parent = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    parent.join(format!("pproxy_{port}.lock"))
}

pub fn run(listen_addr: Option<&str>) -> Result<i32, String> {
    let addr = listen_addr.unwrap_or("127.0.0.1:8899").to_string();

    let port: u16 = addr
        .split(':')
        .last()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8899);

    // 1. 端口级跨进程排他文件锁（支持多端口多实例，同时同一端口互斥）
    let lock_path = get_lock_path_for_port(port);
    if let Some(dir) = lock_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let open_res = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path);

    let lock_file = match open_res {
        Ok(f) => f,
        Err(e) => {
            // Windows 下若已有进程以独占模式打开，open 可能直接返回 Sharing Violation
            eprintln!("\n┌─ [ERROR] 端口 {port} 服务启动冲突 ──────────────────────────");
            eprintln!("│ 无法访问实例锁文件 ({e})：已有 pproxy 实例正在监听端口 {port}。");
            eprintln!("│ ");
            eprintln!("│ 👉 若需停止后台守护进程，请运行: pproxy stop");
            eprintln!("│ 👉 若需启动另一前台实例，请指定新端口: pproxy serve --listen 127.0.0.1:{}", port + 1);
            eprintln!("└─────────────────────────────────────────────────────────────\n");
            return Ok(EXIT_FAILURE);
        }
    };

    if lock_file.try_lock_exclusive().is_err() {
        eprintln!("\n┌─ [ERROR] 端口 {port} 服务启动冲突 ──────────────────────────");
        eprintln!("│ 无法获取实例锁：已有 pproxy 实例正在监听端口 {port}。");
        eprintln!("│ ");
        eprintln!("│ 👉 若需停止后台守护进程，请运行: pproxy stop");
        eprintln!("│ 👉 若需启动另一前台实例，请指定新端口: pproxy serve --listen 127.0.0.1:{}", port + 1);
        eprintln!("└─────────────────────────────────────────────────────────────\n");
        return Ok(EXIT_FAILURE);
    }

    // 成功获取锁后，安全截断并记录当前 PID 与监听地址
    let mut file = lock_file;
    let _ = file.set_len(0);
    let _ = writeln!(file, "pid={}\naddr={addr}", std::process::id());
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

        let tokens = Arc::new(TokenService::new(store.clone()).map_err(|e| e.to_string())?);
        let users = Arc::new(UserService::new(store.clone()).map_err(|e| e.to_string())?);
        let usage = Arc::new(UsageTracker::new(store.clone()));
        let gatekeeper = Arc::new(AuthGatekeeper::default());

        let cfg = load().unwrap_or_default();

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

        let pool_config = PoolConfig {
            worker_url: Some("https://edge.ponyjob.top".to_string()),
            worker_secret: cfg.proxy_secret.clone(),
            ..Default::default()
        };
        let tunnel_cfg = TunnelConfig::from_pool_config_and_env(&pool_config);

        let instance_uuid = generate_instance_uuid();

        let state = GatewayState {
            tokens,
            users: Some(users),
            upstream,
            usage,
            gatekeeper,
            tunnel: tunnel_cfg.map(Arc::new),
            instance_uuid: instance_uuid.clone(),
        };

        let engine_config = EngineConfig {
            listen_addr: addr.clone(),
            instance_uuid: instance_uuid.clone(),
            max_connections: 512,
        };

        // 注册退出信号
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            println!("\n接收到终止信号，正在优雅关闭网关服务...");
            let _ = shutdown_tx.send(true);
        });

        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║            Pony Proxy 嵌入式独立网关 (v0.4.0)                  ║");
        println!("╚════════════════════════════════════════════════════════════════╝");
        println!("  监听地址: http://{addr}");
        println!("  实例签名: {instance_uuid}");
        if is_fresh {
            println!("  数据库: 全新创建 ({})", db_path.display());
        }

        if addr.starts_with("0.0.0.0") {
            println!("\n⚠ 局域网共享模式已开启：");
            println!("  同局域网设备可通过 本机IP:{port} 连接代理（如 http://192.168.x.x:{port}）。");
            println!("  提示: 若局域网无法连接，请放行防火墙 (如: sudo ufw allow {port}/tcp)。");
        }

        println!("\n✓ 代理服务已就绪！按 Ctrl+C 退出服务。\n");

        if let Err(e) = run_engine(engine_config, state, Some(shutdown_rx)).await {
            eprintln!("服务运行异常退出: {e}");
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
            .open(&lock_path)
            .unwrap();

        assert!(file1.try_lock_exclusive().is_ok());

        let file2 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .unwrap();

        // 另一个句柄尝试获取排他锁应该失败
        assert!(file2.try_lock_exclusive().is_err());

        // 释放锁 1 后，锁 2 可以获取
        drop(file1);
        assert!(file2.try_lock_exclusive().is_ok());
    }
}
