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
    bind_target, connect_ws, io_err, try_establish_url, AUTH_401_MARKER, DIAL_TIMEOUT,
    FIRST_FRAME_TIMEOUT, WsPair, WsSink, WsStream,
};
pub use relay::{relay_bidir_ws, AtomicTrafficStats, NoopTrafficCounter, TrafficCounter};
pub use route::{
    classify_egress, is_google_host, is_google_or_ai_host, is_strict_ai_host, order_endpoints,
    order_endpoints_with, ordered_gate_urls, requires_compliant_egress, Egress,
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
}
