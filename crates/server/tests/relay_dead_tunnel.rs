//! L2-T 功能契约测试（T4a 红相先行，test-agent 冻结，实现者只读不可改）。
//!
//! 契约文件：.dev-team/contracts/2026-10-09-pproxy-accept-loop.md（§3 relay deadline 语义）。
//!
//! 死隧道 relay 有界释放契约：
//!   D1) CONNECT relay 必须有有界生命周期：空闲超时（契约默认 30s）+ 绝对上限
//!       （契约默认 6h），均经 env 注入缩短供测试（契约 §2.3）：
//!       `PPROXY_RELAY_IDLE_TIMEOUT_MS` / `PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS`。
//!   D2) 死隧道（双向静默）必须在注入空闲超时 + margin 内释放许可；当前实现
//!       `relay_bidir_ws`（transport/src/relay.rs L67-110）为无超时 `tokio::select!`，
//!       死隧道永久持 permit → 256 死隧道即饱和事故（Lead 现场）。
//!   D3) relay 关闭（超时动作：双端强制断连）后 permit 释放：后续连接受理（契约 §3
//!       释放 permit 时机 = 取最早者，owned permit 随任务结束 drop）。
//!
//! 数值（契约 §3）：注入 `PPROXY_RELAY_IDLE_TIMEOUT_MS=500` → 释放界 =
//! 500 + margin(1000) = 1500ms（契约冻结，禁止 L2-AT 自选阈值）。超时动作须以
//! tokio::time::timeout 等价原语实现（test-expert 语言等价映射：tokio::time::error::Elapsed）。
//!
//! 红相判据（当前实现）：relay 无超时 → 客户端连接 1500ms 内不被关闭 →
//! D2 以 tokio::time::error::Elapsed 原生失败 → 测试失败（红）。
//!
//! 确定性设计：静默 stub worker 完整复刻 worker.js 协议（Text {"host","port"} →
//! {"ok":true}）后不再收发任何业务数据；客户端 CONNECT 后不发任何字节 —— 双向
//! 静默即"死隧道"的确定构造，无概率成分。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pproxy_server::connect::{TunnelConfig, TunnelPool};
use pproxy_server::gateway::{GatewayState, serve_data_plane};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// 契约 env 注入名（契约 §2.3）：relay 空闲超时（ms，默认 30000）。
const CONTRACT_ENV_RELAY_IDLE_MS: &str = "PPROXY_RELAY_IDLE_TIMEOUT_MS";
/// 契约 env 注入名（契约 §2.3）：relay 绝对上限（ms，默认 21600000；测试不缩短）。
const CONTRACT_ENV_RELAY_ABS_MS: &str = "PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS";
/// 注入的 relay idle 超时（ms）—— 契约 §3 测试接缝（业务默认 30s 无法 CI 实测）。
const RELAY_IDLE_INJECTED_MS: u64 = 500;
/// 契约释放界 ms：注入值 500 + margin 1000 = 1500（契约冻结）。
const RELAY_RELEASE_BOUND_MS: u64 = 1500;

static ENV_LOCK: StdMutex<()> = StdMutex::new(());
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Served(u16, String),
    Closed,
    TimedOut,
}

const GET_ROOT: &[u8] = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

fn gw_state(tag: &str) -> GatewayState {
    let dir = std::env::temp_dir().join(format!(
        "relay-contract-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (store, _) = pproxy_core::Store::open(&dir.join("state.db")).unwrap();
    let store = Arc::new(store);
    GatewayState {
        tokens: Arc::new(pproxy_core::TokenService::new(Arc::clone(&store)).unwrap()),
        edges: Arc::new(std::collections::HashMap::new()),
        routes: Arc::new(
            pproxy_core::RouteTable::new(
                store.clone(),
                Arc::new(std::collections::HashMap::new()),
            )
            .unwrap(),
        ),
        usage: Arc::new(pproxy_core::UsageTracker::new(store)),
        tunnel: None,
    }
}

async fn start_gateway(state: GatewayState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_data_plane(listener, state));
    addr
}

/// 静默 stub worker：接受 WS upgrade → 读取 bind 首帧 → 回 {"ok":true} →
/// 之后不收发任何业务数据（worker 侧静默 = 死隧道）。WS 任务用 sleep 挂住，
/// 不 drop tx/rx（避免主动发 Close 干扰死隧道语义）。
async fn spawn_silent_worker() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let callback = |_req: &tokio_tungstenite::tungstenite::http::Request<()>,
                                resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                    Ok(resp)
                };
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
                    return;
                };
                let (mut tx, mut rx) = ws.split();
                // 首帧：{"host","port"} → 回 {"ok":true}（establish 成功）
                let _ = tokio::time::timeout(Duration::from_secs(5), rx.next()).await;
                let _ = tx.send(Message::Text(r#"{"ok":true}"#.into())).await;
                // 之后完全静默：不向客户端透传任何字节，也不关闭 WS。
                let _ = tokio::time::sleep(Duration::from_secs(60)).await;
            });
        }
    });
    format!("ws://{addr}/ws")
}

/// 读完整响应头，返回 (status, head)。
async fn read_response(c: &mut TcpStream) -> (u16, String) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = c.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed before response");
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head)
}

/// 读到 EOF（对端关闭）。Ok = 已关闭；Err = 上界内未关闭。
async fn read_until_eof(c: &mut TcpStream) -> Result<(), ()> {
    let mut tmp = [0u8; 4096];
    loop {
        match c.read(&mut tmp).await {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(_) => return Ok(()), // RST 等视为关闭
        }
    }
}

async fn exchange(addr: SocketAddr, req: &[u8], bound_ms: u64) -> Outcome {
    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let read_res = tokio::time::timeout(Duration::from_millis(bound_ms), async {
        loop {
            let n = c.read(&mut tmp).await.unwrap();
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf).into_owned();
                let status: u16 = head.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
                return Some((status, head));
            }
        }
    })
    .await;
    match read_res {
        Ok(Some((s, h))) => Outcome::Served(s, h),
        Ok(None) => Outcome::Closed,
        Err(_) => Outcome::TimedOut,
    }
}

/// D1+D2+D3：死隧道 relay 必须在有界时间内关闭连接并释放 permit（契约 §3）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_tunnel_relay_closes_within_bound_and_releases_permit() {
    let _guard = env_lock();
    let prev_idle = std::env::var_os(CONTRACT_ENV_RELAY_IDLE_MS);
    let prev_abs = std::env::var_os(CONTRACT_ENV_RELAY_ABS_MS);
    std::env::set_var(CONTRACT_ENV_RELAY_IDLE_MS, RELAY_IDLE_INJECTED_MS.to_string());
    std::env::remove_var(CONTRACT_ENV_RELAY_ABS_MS); // 绝对上限保持契约默认 6h
    struct EnvRestore(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var(CONTRACT_ENV_RELAY_IDLE_MS, v),
                None => std::env::remove_var(CONTRACT_ENV_RELAY_IDLE_MS),
            }
            match &self.1 {
                Some(v) => std::env::set_var(CONTRACT_ENV_RELAY_ABS_MS, v),
                None => std::env::remove_var(CONTRACT_ENV_RELAY_ABS_MS),
            }
        }
    }
    let _restore = EnvRestore(prev_idle, prev_abs);

    let stub_url = spawn_silent_worker().await;
    let cfg = TunnelConfig {
        gate_url: stub_url,
        token: "tok-dead-tunnel".into(),
        allowlist: vec!["dead.example.com".into()],
    };
    let mut state = gw_state("dead");
    // size=0：不预建待命会话，CONNECT 走冷建连（确定性单次 establish）。
    state.tunnel = Some(TunnelPool::with_size(cfg, 0));
    let addr = start_gateway(state).await;

    // D1 前置：establish 成功（200）才进入 relay 阶段。
    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(b"CONNECT dead.example.com:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let (status, head) = read_response(&mut c).await;
    assert_eq!(status, 200, "tunnel establish 必须先成功（200），head: {head}");

    // D2 断言：客户端侧双向静默（死隧道），服务端必须在有界时间内强制断连。
    // 当前实现：relay_bidir_ws 无超时 → 1500ms 内无 EOF → Err(Elapsed) → 红。
    let closed = tokio::time::timeout(
        Duration::from_millis(RELAY_RELEASE_BOUND_MS),
        read_until_eof(&mut c),
    )
    .await;
    assert!(
        closed.is_ok(),
        "契约 D2 违约: 死隧道 relay 未在 {RELAY_RELEASE_BOUND_MS}ms 内关闭客户端连接 \
         （env {CONTRACT_ENV_RELAY_IDLE_MS}={RELAY_IDLE_INJECTED_MS} 注入未生效 / relay 无有界 \
         生命周期）。红相判据：transport relay_bidir_ws 为无超时 select，死隧道永久持 permit"
    );
    println!(
        "PASS: dead tunnel relay closed within {RELAY_RELEASE_BOUND_MS}ms (idle={RELAY_IDLE_INJECTED_MS}ms + margin 1000ms)"
    );

    // D3 断言：relay 关闭后 permit 释放（契约 §3 释放时机）→ 新连接立即被正常受理 (200)。
    let o = exchange(addr, GET_ROOT, RELAY_RELEASE_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(200, _)),
        "契约 D3 违约: relay 关闭后 permit 未释放 —— 新连接未在 {RELAY_RELEASE_BOUND_MS}ms 内\
         得到 200 服务, {o:?}"
    );
    println!("PASS: permit released after relay close (next connection served 200)");
}
