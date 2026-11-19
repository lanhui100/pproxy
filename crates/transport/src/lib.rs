//! Pony Proxy 核心纯净网络传输层 (pproxy-transport)。
//!
//! 提供跨端（Desktop / CLI / Server）共用的底层网络机制：
//! - WebSocket 隧道连接与首帧目标声明协议 (`proto`)；
//! - 跨洲待命连接池与动态配置保活 (`pool`)；
//! - 智能目标域名感知与多端点合规优先级排序 (`route`)；
//! - 支持 TCP 半关闭（Half-Close）的双向数据透传状态机 (`relay`)；
//! - 热态物理 RTT 延迟探测 (`probe`)。

pub mod pool;
pub mod probe;
pub mod proto;
pub mod relay;
pub mod route;

pub use pool::{IdleSession, PoolStats, TunnelPool, token_fp8_of};
pub use probe::{probe_gate_rtt, probe_via_gate};
pub use proto::{
    bind_target, connect_ws, connect_ws_with_timeout, io_err, try_establish_url, AUTH_401_MARKER,
    DIAL_TIMEOUT, FIRST_FRAME_TIMEOUT, PREWARM_DIAL_TIMEOUT, WsPair, WsSink, WsStream,
};
pub use relay::{
    relay_bidir_ws, relay_bidir_ws_bounded, AtomicTrafficStats, NoopTrafficCounter,
    RelayDeadlineHit, TrafficCounter,
};
pub use route::{
    classify_egress, compliant_egress_endpoints, is_google_host, is_google_or_ai_host,
    is_strict_ai_host, is_vercel_endpoint, is_youtube_host, order_endpoints, order_endpoints_with,
    ordered_gate_urls, requires_compliant_egress, Egress,
};

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    /// 模拟 gate 服务端
    async fn spawn_mock_gate(first_ok: bool, conns: Arc<AtomicUsize>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                conns.fetch_add(1, Ordering::SeqCst);
                let conns_inner = Arc::clone(&conns);
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
                                Message::Ping(p) => {
                                    let _ = ws.send(Message::Pong(p)).await;
                                }
                                Message::Binary(b) => {
                                    // 回显
                                    let _ = ws.send(Message::Binary(b)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                    conns_inner.fetch_sub(0, Ordering::SeqCst);
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn test_pool_preconnect_and_checkout() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 2);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池预建超时");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(conns.load(Ordering::SeqCst), 2);

        // checkout（同指纹命中）
        let item = pool.checkout(&[&url], Some(&token_fp8_of("token")));
        assert!(item.is_some());
        let (tx, rx, used_url) = item.unwrap();
        assert_eq!(used_url, url);

        // bind
        let bound = bind_target(tx, rx, "google.com", 443).await;
        assert!(bound.is_ok());
    }

    #[tokio::test]
    async fn test_pool_size_zero_disables_preconnect() {
        // PPROXY_TUNNEL_POOL=0 的语义：不预建任何待命会话（此前被 size.max(1) 吞掉）。
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 0);
        pool.start_maintain();

        tokio::time::sleep(Duration::from_millis(400)).await;

        assert_eq!(pool.idle_total(), 0, "size=0 不得预建待命会话");
        assert_eq!(conns.load(Ordering::SeqCst), 0, "size=0 不得发起任何 WS 连接");
        assert!(pool.checkout(&[&url], None).is_none(), "size=0 时 checkout 必须为空");
    }

    #[tokio::test]
    async fn test_pool_refills_expired_automatically() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        let pool = TunnelPool::with_timing(
            rx,
            1,
            Duration::from_millis(300),
            Duration::from_millis(80),
            Duration::from_millis(150),
        );
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(conns.load(Ordering::SeqCst), 1);

        // 等待连接过期 (测试环境 IDLE_TTL = 300ms)
        tokio::time::sleep(Duration::from_millis(400)).await;

        // maintain 自动剔除并补建
        let deadline2 = tokio::time::Instant::now() + Duration::from_secs(5);
        while conns.load(Ordering::SeqCst) < 2 {
            assert!(tokio::time::Instant::now() < deadline2);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn test_pool_exits_when_owner_dropped() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 1);
        let handle = pool.start_maintain().unwrap();

        // 等待首次建连
        tokio::time::sleep(Duration::from_millis(100)).await;

        // drop pool
        drop(pool);

        // handle 必须在超时内完成退出
        let res = tokio::time::timeout(Duration::from_secs(3), handle).await;
        assert!(res.is_ok(), "maintain 任务未在 pool drop 后及时退出");
    }

    #[tokio::test]
    async fn test_pool_skips_vercel_endpoints() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        // 端点 URL 中包含 /api/ws 或 vercel，代表 Vercel 出口
        let vercel_url = format!("ws://{addr}/api/ws");
        let (_tx, rx) = watch::channel((Some(vercel_url.clone()), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 2);
        pool.start_maintain();

        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(pool.idle_total(), 0, "Vercel 端点不得预建待命连接");
        assert_eq!(conns.load(Ordering::SeqCst), 0, "Vercel 端点不得发起预连接");
        assert!(pool.checkout(&[&vercel_url], None).is_none());
    }

    /// 握手被延迟的 mock gate：验证补给预算能覆盖超出交互预算的长尾握手。
    async fn spawn_slow_mock_gate(
        delay: Duration,
        conns: Arc<AtomicUsize>,
    ) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                conns.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Ok(mut ws) = accept_async(stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            match msg {
                                Message::Text(_) => {
                                    let resp =
                                        serde_json::json!({ "ok": true }).to_string();
                                    let _ = ws.send(Message::Text(resp)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    /// 门禁：补给预算必须宽于交互预算，否则长尾拨号又会被砍掉。
    #[test]
    fn test_prewarm_budget_wider_than_interactive() {
        assert!(
            PREWARM_DIAL_TIMEOUT > DIAL_TIMEOUT,
            "补给拨号预算 {PREWARM_DIAL_TIMEOUT:?} 必须宽于交互预算 {DIAL_TIMEOUT:?}"
        );
    }

    /// 回归：握手耗时介于交互预算与补给预算之间时，补给仍须成功。
    /// 旧实现复用 `DIAL_TIMEOUT`（生产 8s），实测跨境 p99 长尾被稳定切掉 → 池见底 → 冷建连穿透。
    #[tokio::test]
    async fn test_pool_refill_tolerates_slow_handshake() {
        // 超出交互预算、仍在补给预算内
        let delay = DIAL_TIMEOUT + Duration::from_millis(200);
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_slow_mock_gate(delay, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "慢握手（{delay:?}）应被补给预算容忍，实际池仍为空"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 回归：补给轮内某端点失败不得阻断其它端点补池。
    /// 旧实现失败即 `break` 退出端点循环，造成队头阻塞——排在前面的坏端点会让后面的健康端点永远补不上。
    #[tokio::test]
    async fn test_pool_refill_isolates_failing_endpoint() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;

        // 取一个必然拒绝连接的端口：绑定后立即释放
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);

        let dead_url = format!("ws://{dead_addr}");
        let good_url = format!("ws://{addr}");
        let urls = format!("{dead_url},{good_url}");
        let (_tx, rx) = watch::channel((Some(urls), Some("token".into())));

        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if pool
                .checkout(&[&good_url], Some(&token_fp8_of("token")))
                .is_some()
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "失败端点不得阻断健康端点的补给"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 回归：补给必须在会话过期**之前**触发（提前量 `2 × refill_interval`）。
    /// 旧口径按“未过期”计数 → 缺口在过期前一刻恒为 0 → 池在每轮过期瞬间见底，
    /// 直到下一轮 refill 才恢复，窗口内到达的请求 100% 冷建连。
    #[tokio::test]
    async fn test_pool_refills_before_expiry() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        // ttl 600ms / refill 50ms → 提前量 100ms；size=1 让“池是否见底”无歧义
        let pool = TunnelPool::with_timing(
            rx,
            1,
            Duration::from_millis(600),
            Duration::from_millis(50),
            Duration::from_millis(50),
        );
        pool.start_maintain();

        // 等首条就位（首轮预建完成前的空池不算稳态）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.fresh_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "首轮预建超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // 覆盖 3 个 TTL；5ms 采样 ≪ 100ms 提前量，过期空洞必被发现
        let end = tokio::time::Instant::now() + Duration::from_millis(1800);
        let mut samples = 0usize;
        while tokio::time::Instant::now() < end {
            let n = pool.fresh_total();
            samples += 1;
            assert!(
                n >= 1,
                "稳态下池不得见底，第 {samples} 次采样可用会话 = {n}"
            );
            assert!(n <= 2, "补建过量：单端点 size=1 时不应有 {n} 条未过期会话");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(samples > 200, "采样数过少（{samples}），判据失效");
        assert!(
            conns.load(Ordering::SeqCst) >= 3,
            "应跨越 ≥3 个 TTL 周期，实际建连 {}",
            conns.load(Ordering::SeqCst)
        );
    }

    /// 回归：同一批预建的会话必须错开出生时间。
    /// 旧实现整批 `born = now()` → 25s 后集体过期 → 池成段清空（生产 pooled=false 的直接来源之一）。
    #[tokio::test]
    async fn test_pool_staggers_same_batch_birth() {
        let conns = Arc::new(AtomicUsize::new(0));
        let addr = spawn_mock_gate(true, Arc::clone(&conns)).await;
        let url = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(url.clone()), Some("token".into())));

        // ttl 600ms / refill 50ms → 提前量 100ms、可用寿命 500ms；size 3 → 步长 ≈166ms
        let pool = TunnelPool::with_timing(
            rx,
            3,
            Duration::from_millis(600),
            Duration::from_millis(50),
            Duration::from_millis(50),
        );
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.idle_total() < 3 {
            assert!(tokio::time::Instant::now() < deadline, "首轮预建超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let ages = pool.ages();
        let spread = ages[ages.len() - 1] - ages[0];
        assert!(
            spread >= Duration::from_millis(120),
            "同批 3 条会话的出生时间未错峰（spread={spread:?}，期望 ≥120ms）: {ages:?}"
        );
    }
}
