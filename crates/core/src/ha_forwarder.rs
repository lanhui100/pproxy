//! 本地高可用分发桩 (Local HA Forwarder) — 高性能与自愈优化版
//!
//! 性能关键改进：
//! 1. 消除关键路径上的 150ms 串行硬等：
//!    引入后台异步健康探测器 (start_health_prober)，维护 `local_healthy: Arc<AtomicBool>`；
//!    本地引擎存活时 0 延迟命中本地；熔断下线时 0 延时直切远程备灾节点，不堵塞请求；
//! 2. 精确零拷贝 Header 处理：
//!    直接按切片处理请求头并注入 X-Pony-Cluster-Ticket，消除多次 String 内存分配与拷贝；
//! 3. 双向原始 TCP 流零拷贝中继 (copy_bidirectional)；
//! 4. 远程节点全通过集群机器密钥 (HMAC) 票证认证。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct LocalHaForwarder {
    listen_addr: SocketAddr,
    local_target: SocketAddr,
    remote_candidates: Arc<RwLock<Vec<SocketAddr>>>,
    cluster_auth_key: String,
    node_id: String,
    /// 本地主引擎存活状态（后台探测维护，消除请求关键路径上的 150ms 串行等待）
    local_healthy: Arc<AtomicBool>,
}

impl LocalHaForwarder {
    pub fn new(listen_addr: SocketAddr, local_target: SocketAddr, remotes: Vec<SocketAddr>) -> Self {
        Self {
            listen_addr,
            local_target,
            remote_candidates: Arc::new(RwLock::new(remotes)),
            cluster_auth_key: String::new(),
            node_id: String::new(),
            local_healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn with_cluster_identity(mut self, cluster_auth_key: impl Into<String>, node_id: impl Into<String>) -> Self {
        self.cluster_auth_key = cluster_auth_key.into();
        self.node_id = node_id.into();
        self
    }

    pub async fn update_remotes(&self, remotes: Vec<SocketAddr>) {
        let mut guard = self.remote_candidates.write().await;
        *guard = remotes;
    }

    /// 后台主动健康探活循环：300ms 探测一次本地主引擎，实现零毫秒关键路径故障切换
    fn start_health_prober(self: &Arc<Self>) {
        let local_target = self.local_target;
        let healthy_flag = Arc::clone(&self.local_healthy);

        tokio::spawn(async move {
            let mut failed_count = 0;
            loop {
                let probe_ok = match tokio::time::timeout(
                    Duration::from_millis(80),
                    TcpStream::connect(local_target),
                ).await {
                    Ok(Ok(stream)) => {
                        let _ = stream.set_nodelay(true);
                        drop(stream);
                        true
                    }
                    _ => false,
                };

                if probe_ok {
                    failed_count = 0;
                    if !healthy_flag.load(Ordering::Relaxed) {
                        healthy_flag.store(true, Ordering::Release);
                        tracing::info!(local = %local_target, "Local primary engine restored to healthy state (0ms routing)");
                    }
                } else {
                    failed_count += 1;
                    // 连续 2 次探测失败即认定下线，开启熔断，请求直接跳过本地走远程备灾
                    if failed_count >= 2 && healthy_flag.load(Ordering::Relaxed) {
                        healthy_flag.store(false, Ordering::Release);
                        tracing::warn!(local = %local_target, "Local primary engine marked UNHEALTHY, activating zero-wait failover");
                    }
                }

                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        });
    }

    pub async fn start(self: Arc<Self>) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        self.start_health_prober();

        tracing::info!(
            listen = %self.listen_addr,
            local = %self.local_target,
            ticket_auth = !self.cluster_auth_key.is_empty(),
            "Local HA Forwarder is actively guarding local port (Zero-Wait Prober active)"
        );

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((inbound, peer_addr)) => {
                        let forwarder = Arc::clone(&self);
                        tokio::spawn(async move {
                            let _ = inbound.set_nodelay(true);
                            if let Err(e) = forwarder.handle_conn(inbound, peer_addr).await {
                                tracing::debug!(peer = %peer_addr, error = %e, "forwarder session closed");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "forwarder accept error");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn pick_upstream(&self) -> Option<(TcpStream, bool)> {
        // 1. 若本地健康标记为 true，才尝试建连本地；若已熔断标记为 false，0 耗时直接跳过！
        if self.local_healthy.load(Ordering::Acquire) {
            if let Ok(Ok(stream)) = tokio::time::timeout(
                Duration::from_millis(80),
                TcpStream::connect(self.local_target),
            ).await {
                let _ = stream.set_nodelay(true);
                return Some((stream, false));
            }
            // 本次即时建连失败，临时熔断置为 false
            self.local_healthy.store(false, Ordering::Release);
        }

        // 2. 备灾远程对等节点快速并发或轮询 (收紧单次握手至 300ms)
        let remotes = {
            let guard = self.remote_candidates.read().await;
            guard.clone()
        };

        for remote in remotes {
            if let Ok(Ok(stream)) = tokio::time::timeout(
                Duration::from_millis(300),
                TcpStream::connect(remote),
            ).await {
                let _ = stream.set_nodelay(true);
                return Some((stream, true));
            }
        }
        None
    }

    async fn handle_conn(&self, mut inbound: TcpStream, _peer_addr: SocketAddr) -> anyhow::Result<()> {
        // 1. 读请求头（首个 \r\n\r\n 边界）
        let mut head_buf = Vec::with_capacity(4096);
        let mut tmp = [0u8; 4096];
        let header_end_pos = loop {
            let n = tokio::time::timeout(Duration::from_secs(10), inbound.read(&mut tmp)).await??;
            if n == 0 {
                return Ok(());
            }
            head_buf.extend_from_slice(&tmp[..n]);
            if head_buf.len() > 64 * 1024 {
                anyhow::bail!("request head too large");
            }
            if let Some(pos) = find_header_end(&head_buf) {
                break pos;
            }
        };

        let raw_headers = &head_buf[..header_end_pos];
        let leftover_payload = &head_buf[header_end_pos + 4..];

        // 2. 选上游（若本地挂掉，经过后台探活，此处为 0 延时切远程！）
        let (mut outbound, is_remote) = match self.pick_upstream().await {
            Some(pair) => pair,
            None => {
                let _ = inbound.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-type: text/plain\r\n\r\nAll local and remote cluster targets unreachable").await;
                anyhow::bail!("All local and remote HA cluster targets are unreachable");
            }
        };

        // 3. 构造转发头部：高效按切片组装，消除全量 String 堆分配
        if is_remote && !self.cluster_auth_key.is_empty() && !self.node_id.is_empty() {
            if let Ok(ticket) = crate::cluster_ticket::create_ticket(&self.cluster_auth_key, &self.node_id) {
                outbound.write_all(raw_headers).await?;
                outbound.write_all(b"\r\n").await?;
                outbound.write_all(crate::cluster_ticket::CLUSTER_TICKET_HEADER.as_bytes()).await?;
                outbound.write_all(b": ").await?;
                outbound.write_all(ticket.as_bytes()).await?;
                outbound.write_all(b"\r\n\r\n").await?;
            } else {
                outbound.write_all(&head_buf[..header_end_pos + 4]).await?;
            }
        } else {
            // 本地直连原样发出去，零分配零拷贝
            outbound.write_all(&head_buf[..header_end_pos + 4]).await?;
        }

        // 4. 首包剩余载荷（如 TLS ClientHello）快速透传
        if !leftover_payload.is_empty() {
            outbound.write_all(leftover_payload).await?;
        }

        // 5. 双向流零拷贝中继
        tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
        Ok(())
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_ha_forwarder_zero_wait_prober() {
        let remote_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote_listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = remote_listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nREMOTE_OK").await;
            }
        });

        let bind = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fwd_addr = bind.local_addr().unwrap();
        drop(bind);

        let dead: SocketAddr = "127.0.0.1:65432".parse().unwrap();
        let forwarder = Arc::new(LocalHaForwarder::new(fwd_addr, dead, vec![remote_addr]));
        forwarder.start().await.unwrap();

        // 快速请求，确认能正常路由到远程
        let mut client = TcpStream::connect(fwd_addr).await.unwrap();
        client.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();

        let mut resp = vec![0u8; 512];
        let n = client.read(&mut resp).await.unwrap();
        let body = String::from_utf8_lossy(&resp[..n]);
        assert!(body.contains("REMOTE_OK"), "failover response mismatch: {body}");
    }
}
