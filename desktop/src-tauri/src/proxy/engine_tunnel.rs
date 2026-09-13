//! WS 隧道客户端（M6 spec §3）：经 wss 连接 gate worker 中继 TLS 字节。
//!
//! 流程：Upgrade（Bearer tunnel_token）→ 首帧 JSON {"host","port"} →
//! {"ok":true} → 回 200/注入首行 → 双向透传。R4：任何失败即报错关闭，
//! 绝不静默回落直连。
//!
//! 底层核心（TunnelPool、Half-Close relay、智能排序与握手协议）已统一下沉至 `pproxy-transport`。

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[allow(unused_imports)]
pub use pproxy_transport::{
    bind_target, classify_egress, is_google_or_ai_host, order_endpoints, probe_gate_rtt,
    probe_via_gate, relay_bidir_ws, try_establish_url, AUTH_401_MARKER, Egress, TunnelPool,
    WsPair,
};

use super::engine::{EngineConfig, EngineStats, Kind, ReqHead};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("tunnel: {e}"))
}

/// 401 自愈冷却：进程内距上次自愈至少间隔此时长（single-flight 负缓存，
/// 防止 token 轮换瞬间 N 个并发 CONNECT 各自同步读 keyring + 重复重试）。
#[cfg(not(test))]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(0);
static LAST_SELF_HEAL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

const RETRY: u32 = 5; // 总尝试次数（首次 + 4 次重试）

/// 建连阶段：支持多中继端点自动切换（CF 节点不可达/被拒时无缝回退备用 Vercel/Node 节点）。
/// 返回成功使用的端点 URL，供流量统计按出口（CF/Vercel）归账。
///
/// 优先从待命池 checkout（已预建 WS Upgrade，热态），命中则只做首帧声明（1 RTT）；
/// 池 miss / 池会话死亡（bind 失败）回落到冷建连全流程。
async fn establish(
    cfg: &EngineConfig,
    parsed: &ReqHead,
) -> Result<(WsPair, String), std::io::Error> {
    let (url_raw, token) = {
        let (u, t) = cfg.tunnel.borrow().clone();
        (
            u.ok_or_else(|| io("tunnel_url not configured"))?,
            t.ok_or_else(|| io("tunnel token missing"))?,
        )
    };

    let urls: Vec<&str> = url_raw
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    // 按目标 host 重排端点优先级（Google/AI 目标 → Vercel 优先；其他 → CF 优先）
    let urls = order_endpoints(urls, &parsed.host);

    // 方案 A：先取池会话，bind 热态首帧
    if let Some((tx, rx, used_url)) = cfg.pool.checkout(&urls) {
        match bind_target(tx, rx, &parsed.host, parsed.port).await {
            Ok(pair) => {
                log::debug!("tunnel established from pool via {used_url} for {}", parsed.host);
                return Ok((pair, used_url));
            }
            Err(e) => {
                log::warn!("pooled session bind failed for {} on {used_url}: {e}", parsed.host);
            }
        }
    }

    let mut last_err = None;
    let mut saw_401 = false;
    for u in &urls {
        match try_establish_url(u, &token, &parsed.host, parsed.port).await {
            Ok(pair) => return Ok((pair, (*u).to_string())),
            Err(e) => {
                if e.to_string().contains(AUTH_401_MARKER) { saw_401 = true; }
                log::warn!("tunnel establish on {u} for {} failed: {e}", parsed.host);
                last_err = Some(e);
            }
        }
    }

    // 401 自愈：全部端点鉴权失败时，凭据可能已被外部更新（轮换）——重读凭据并刷新重试。
    // 对抗加固：仅在成功读取到新 token 并广播 watch 后才置位冷却时间，防止旧 token 阻断自愈。
    if saw_401 {
        let should_check = {
            let g = LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner());
            match *g {
                Some(t) => t.elapsed() >= SELF_HEAL_COOLDOWN,
                None => true,
            }
        };
        let (fresh_url, fresh_token) = if should_check {
            tokio::task::spawn_blocking(crate::tunnel_config_load).await.unwrap_or((None, None))
        } else {
            (None, None)
        };
        if let (Some(u), Some(t)) = (fresh_url, fresh_token) {
            let current = cfg.tunnel.borrow().clone();
            // 严防倒灌：仅当重读出来的 token 与当前内存 token 不同、且确有值时才轮换；
            // 且必须确保 fresh_token 经过基本有效性检验（不能是空串）。
            if Some(t.clone()) != current.1 && t != token && !t.trim().is_empty() {
                log::info!("tunnel token rotated on disk, refreshing watch and retrying once");
                *LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
                let _ = crate::ensure_tunnel_watch().send((Some(u.clone()), Some(t.clone())));
                let urls2: Vec<&str> = u.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                let urls2 = order_endpoints(urls2, &parsed.host);
                for u2 in &urls2 {
                    match try_establish_url(u2, &t, &parsed.host, parsed.port).await {
                        Ok(pair) => return Ok((pair, (*u2).to_string())),
                        Err(e) => {
                            log::warn!("tunnel re-establish on {u2} for {} failed: {e}", parsed.host);
                            last_err = Some(e);
                        }
                    }
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| io("no valid tunnel urls configured")))
}

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    cfg: &EngineConfig,
    stats: &EngineStats,
) -> std::io::Result<()> {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RETRY {
        match establish(cfg, &parsed).await {
            Ok(((mut ws_tx, ws_rx), used_url)) => {
                let egress = classify_egress(&used_url);
                match egress {
                    Egress::Cf => &stats.cf_reqs,
                    Egress::Vercel => &stats.vercel_reqs,
                }
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if parsed.kind == Kind::Connect {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;
                } else {
                    let full_req = rebuild_request(head);
                    let bytes = full_req.into_bytes();
                    match egress {
                        Egress::Cf => &stats.cf_up,
                        Egress::Vercel => &stats.vercel_up,
                    }
                    .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    use futures_util::SinkExt as _;
                    ws_tx
                        .send(tokio_tungstenite::tungstenite::Message::Binary(bytes))
                        .await
                        .map_err(io)?;
                }
                return relay_bidir_ws(client, ws_tx, ws_rx, egress, stats).await;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < RETRY {
                    let backoff = std::time::Duration::from_millis(
                        50 * (1 << attempt.min(6)) + (rand::random::<u64>() % 50),
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    let err = last_err.unwrap_or_else(|| io("tunnel establish failed"));
    let msg = format!("502 Bad Gateway: tunnel failed for {}: {}", parsed.host, err);
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        msg.len(),
        msg
    );
    let _ = client.write_all(resp.as_bytes()).await;
    Err(err)
}

/// 重写明文请求：首行 absolute-form → origin-form，其余头原样保留（含结尾空行与 body）。
fn rebuild_request(head: &str) -> String {
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, "\r\n\r\n"));
    format!("{}\r\n{}", rewrite_first_line(first), rest)
}

fn rewrite_first_line(first: &str) -> String {
    let parts: Vec<&str> = first.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return first.to_string();
    }
    let after_scheme = parts[1].strip_prefix("http://").unwrap_or(parts[1]);
    let path_start = after_scheme.find(['/', '?']);
    let path = match path_start {
        Some(idx) => &after_scheme[idx..],
        None => "/",
    };
    format!("{} {} {}", parts[0], path, parts[2])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    async fn spawn_fake_gate(ok: bool) -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut ws) = accept_async(stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            match msg {
                                Message::Text(t) => {
                                    let _v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                                    let resp = if ok {
                                        serde_json::json!({ "ok": true }).to_string()
                                    } else {
                                        serde_json::json!({ "ok": false, "reason": "mock_denied" }).to_string()
                                    };
                                    let _ = ws.send(Message::Text(resp)).await;
                                }
                                Message::Binary(b) => {
                                    let _ = ws.send(Message::Binary(b)).await;
                                }
                                Message::Ping(p) => {
                                    let _ = ws.send(Message::Pong(p)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        Ok((addr, handle))
    }

    fn cfg_with_endpoints(urls: String) -> EngineConfig {
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(crate::proxy::pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        let (_utx, urx) = watch::channel(None);
        let (_ptx, prx) = watch::channel((None, None));
        EngineConfig {
            listen_addr: "127.0.0.1:0".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
            pool: TunnelPool::with_size(prx, 0),
        }
    }

    fn cfg_with_pool(urls: String, pool: Arc<TunnelPool>) -> EngineConfig {
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(crate::proxy::pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        let (_utx, urx) = watch::channel(None);
        EngineConfig {
            listen_addr: "127.0.0.1:0".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
            pool,
        }
    }

    #[tokio::test]
    async fn establish_falls_over_to_second_endpoint_on_denied() {
        let (a_addr, a_handle) = spawn_fake_gate(false).await.unwrap();
        let (b_addr, b_handle) = spawn_fake_gate(true).await.unwrap();
        let cfg = cfg_with_endpoints(format!("ws://{a_addr},ws://{b_addr}"));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };

        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "首端点 denied 后应落到次端点: {result:?}");

        let ((mut ws_tx, mut ws_rx), _used) = result.unwrap();
        ws_tx.send(Message::Binary(b"PING".to_vec())).await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(3), ws_rx.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply.into_data(), b"PING");

        a_handle.abort();
        b_handle.abort();
    }

    #[tokio::test]
    async fn establish_falls_over_on_silent_endpoint() {
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        let (b_addr, b_handle) = spawn_fake_gate(true).await.unwrap();
        let cfg = cfg_with_endpoints(format!("ws://{dead_addr},ws://{b_addr}"));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };

        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "静默端点后应落到次端点: {result:?}");
        b_handle.abort();
    }

    #[test]
    fn rebuild_request_keeps_single_blank_line_and_body() {
        assert_eq!(
            rebuild_request("POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"),
            "POST /y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
    }

    #[test]
    fn rebuild_request_without_body_ends_with_blank_line() {
        assert_eq!(
            rebuild_request("GET http://x/y HTTP/1.1\r\nHost: x\r\n\r\n"),
            "GET /y HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }

    #[test]
    fn google_host_detection_matches_gate_policy() {
        for h in [
            "google.com",
            "www.google.com",
            "google.com.hk",
            "www.google.co.jp",
            "accounts.google.com",
            "generativelanguage.googleapis.com",
            "oauth2.googleapis.com",
            "www.gstatic.com",
            "deepmind.google",
            "antigravity.google",
            "api.antigravity.google",
            "labs.google",
            "g.co",
            "openai.com",
            "api.openai.com",
            "chatgpt.com",
            "claude.ai",
            "anthropic.com",
        ] {
            assert!(is_google_or_ai_host(h), "应识别为 Google/AI 系: {h}");
        }
        for h in ["github.com", "youtube.com", "notgoogleapis.com", "googleapis.com.evil.cn", "baidu.com"] {
            assert!(!is_google_or_ai_host(h), "不应识别为 Google/AI 系: {h}");
        }
    }

    #[test]
    fn endpoint_order_prefers_vercel_for_google_and_ai() {
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];
        let ordered = order_endpoints(urls.clone(), "oauth2.googleapis.com");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "Google 应 Vercel 优先");
        assert_eq!(ordered[1], "wss://gate.example.com/ws");

        let ordered = order_endpoints(urls.clone(), "google.com.hk");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "Google 国别域应 Vercel 优先");

        let ordered = order_endpoints(urls.clone(), "antigravity.google");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "agy 域名应 Vercel 优先");

        let ordered = order_endpoints(urls.clone(), "api.openai.com");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "OpenAI 应 Vercel 优先");
        assert_eq!(ordered[1], "wss://gate.example.com/ws");

        let ordered = order_endpoints(urls.clone(), "github.com");
        assert_eq!(ordered[0], "wss://gate.example.com/ws", "常规非 Google/AI 应 CF 优先");
        assert_eq!(ordered[1], "wss://vgate.example.com/api/ws");
    }

    #[test]
    fn endpoint_order_stable_for_custom_and_unknown() {
        let urls = vec![
            "wss://custom-a.example/ws",
            "wss://custom-b.example/ws",
        ];
        let ordered = order_endpoints(urls, "random.host.io");
        assert_eq!(ordered[0], "wss://custom-a.example/ws");
        assert_eq!(ordered[1], "wss://custom-b.example/ws");
    }

    async fn spawn_counting_gate(
        first_ok: bool,
        conns: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::io::Result<std::net::SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                conns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    if let Ok(mut ws) = accept_async(stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            match msg {
                                Message::Text(_) => {
                                    let resp = if first_ok {
                                        serde_json::json!({ "ok": true }).to_string()
                                    } else {
                                        serde_json::json!({ "ok": false, "reason": "mock_denied" }).to_string()
                                    };
                                    let _ = ws.send(Message::Text(resp)).await;
                                }
                                Message::Binary(b) => {
                                    let _ = ws.send(Message::Binary(b)).await;
                                }
                                Message::Ping(p) => {
                                    let _ = ws.send(Message::Pong(p)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        Ok(addr)
    }

    fn test_pool(rx: watch::Receiver<(Option<String>, Option<String>)>, size: usize) -> Arc<TunnelPool> {
        TunnelPool::with_timing(
            rx,
            size,
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(80),
            std::time::Duration::from_millis(150),
        )
    }

    #[tokio::test]
    async fn tunnel_pool_preconnects_without_traffic() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未在 5s 内预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tunnel_pool_checkout_follows_endpoint_order() {
        let a_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let b_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let a_url = format!("ws://{a_addr}/api/ws");
        let b_url = format!("ws://{b_addr}/ws");
        let urls = format!("{a_url},{b_url}");
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let ordered_b = order_endpoints(vec![&a_url, &b_url], "github.com");
        let item = pool.checkout(&ordered_b);
        assert!(item.is_some());
        assert_eq!(item.unwrap().2, b_url);
    }

    #[tokio::test]
    async fn establish_reuses_pooled_hot_session() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未在 5s 内预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        let cfg = cfg_with_pool(urls, Arc::clone(&pool));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.google.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "establish 应命中池并成功: {result:?}");
        assert!(conns.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn tunnel_pool_clears_and_rebuilds_on_watch_change() {
        let conns_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr_a = spawn_counting_gate(true, Arc::clone(&conns_a)).await.unwrap();
        let (tx, rx) = watch::channel((Some(format!("ws://{addr_a}")), Some("mock-token-a".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let conns_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr_b = spawn_counting_gate(true, Arc::clone(&conns_b)).await.unwrap();
        tx.send((Some(format!("ws://{addr_b}")), Some("mock-token-b".into()))).unwrap();

        let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while conns_b.load(std::sync::atomic::Ordering::SeqCst) < 1 {
            assert!(tokio::time::Instant::now() < deadline2, "watch 变化后未向新端点建连");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn establish_falls_back_when_pooled_session_bind_fails() {
        let a_addr = spawn_counting_gate(false, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let b_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let urls = format!("ws://{a_addr},ws://{b_addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let cfg = cfg_with_pool(urls, pool);
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "池会话 bind 失败应回落并命中次端点: {result:?}");
    }

    #[tokio::test]
    async fn tunnel_pool_refills_expired_sessions_automatically() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while conns.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            assert!(tokio::time::Instant::now() < deadline2);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn tunnel_pool_maintain_exits_when_owner_dropped() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        let handle = pool.start_maintain().unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(pool);

        let res = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
        assert!(res.is_ok(), "maintain 任务未在 pool drop 后及时退出");
    }
}
