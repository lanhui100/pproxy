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
use pproxy_engine::connect::{TunnelConfig, TunnelPool};
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

/// 自动探测本机在局域网中的真实内网 IP 地址（例如 192.168.x.x）
pub fn get_local_lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").or_else(|_| socket.connect("1.1.1.1:80")).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// 解析最终监听地址（优先级：显式 --listen > --lan/--port 组合 > 默认 127.0.0.1:8899）
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

pub fn run(listen_addr: Option<&str>, lan: bool, port: Option<u16>) -> Result<i32, String> {
    let addr = resolve_listen_addr(listen_addr, lan, port);

    let port: u16 = addr
        .split(':')
        .next_back()
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
        let tunnel_pool = tunnel_cfg.map(TunnelPool::new);

        let instance_uuid = generate_instance_uuid();

        let state = GatewayState {
            tokens,
            users: Some(users),
            upstream,
            usage,
            gatekeeper,
            tunnel: tunnel_pool,
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
            let lan_ip = get_local_lan_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "本机局域网IP".to_string());
            println!("\n\x1b[1;33m⚠ 局域网共享模式已开启 (0.0.0.0:{port})：\x1b[0m");
            println!("  本机访问地址:     http://127.0.0.1:{port}");
            println!("  局域网设备请连接: \x1b[1;32mhttp://{lan_ip}:{port}\x1b[0m");
            println!("  手机/平板连接同一 WiFi 后，代理主机填入 {lan_ip}，端口填入 {port} 即可。");
            println!("  提示: 若手机无法连接，请放行防火墙端口 (如: netsh advfirewall / sudo ufw allow {port}/tcp)。");
        } else {
            println!("\n  访问模式: 本机独享 (127.0.0.1:{port})");
            println!("  提示: 如需供同局域网手机/设备使用，请使用快捷选项: \x1b[1;36mpproxy serve --lan\x1b[0m");
        }

        println!("\n✓ 代理服务已就绪！按 Ctrl+C 退出服务。\n");

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
}

