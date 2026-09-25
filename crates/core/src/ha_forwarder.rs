//! 本地高可用分发桩 (Local HA Forwarder) — 修复与加固版
//!
//! 核心设计：
//! 1. 独占监听本机对外固定入口（默认 127.0.0.1:8899）；
//! 2. 对入站请求做精准 HTTP/CONNECT 代理语义解析：
//!    - CONNECT host:port -> 向所选上游代理发起 CONNECT，成功后双向透传隧道；
//!    - GET/POST http://... -> 向所选上游代理发送请求，透传响应；
//! 3. 精确切片处理：
//!    - 严禁在原有 \r\n\r\n 基础上插入多余 \r\n\r\n 导致 TLS/HTTP 载荷错位；
//!    - 首包 leftover 数据与握手响应精准拼接；
//!    - CONNECT 非 200 状态码优雅透传退出，防止协程永久挂死；
//! 4. 鉴权安全：
//!    - 本地主引擎（127.0.0.1:18899）回环免认证；
//!    - 远程节点通过 `X-Pony-Cluster-Ticket`（基于 cluster_auth_key 的 HMAC 短效票证）安全认证。

use std::net::SocketAddr;
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
}

impl LocalHaForwarder {
    pub fn new(listen_addr: SocketAddr, local_target: SocketAddr, remotes: Vec<SocketAddr>) -> Self {
        Self {
            listen_addr,
            local_target,
            remote_candidates: Arc::new(RwLock::new(remotes)),
            cluster_auth_key: String::new(),
            node_id: String::new(),
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

    pub async fn start(self: Arc<Self>) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        tracing::info!(
            listen = %self.listen_addr,
            local = %self.local_target,
            ticket_auth = !self.cluster_auth_key.is_empty(),
            "Local HA Forwarder is actively guarding local port"
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
        // 1. 本地主引擎优先（150ms 快速探测）
        if let Ok(Ok(stream)) = tokio::time::timeout(
            Duration::from_millis(150),
            TcpStream::connect(self.local_target),
        ).await {
            let _ = stream.set_nodelay(true);
            return Some((stream, false));
        }

        tracing::warn!(
            local = %self.local_target,
            "Local primary engine unresponsive, activating instant failover to remote cluster peers..."
        );

        // 2. 备灾远程对等节点
        let remotes = {
            let guard = self.remote_candidates.read().await;
            guard.clone()
        };
        for remote in remotes {
            if let Ok(Ok(stream)) = tokio::time::timeout(
                Duration::from_millis(500),
                TcpStream::connect(remote),
            ).await {
                let _ = stream.set_nodelay(true);
                tracing::info!(remote = %remote, "Failover successful: connection dispatched to cluster peer");
                return Some((stream, true));
            }
        }
        None
    }

    async fn handle_conn(&self, mut inbound: TcpStream, _peer_addr: SocketAddr) -> anyhow::Result<()> {
        // 1. 读取首个请求头（截止到 \r\n\r\n）
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

        // header_end_pos 指向首个 \r，完整的 header 块不包含尾部 \r\n\r\n 为 head_buf[..header_end_pos]
        // 随首包到达的所有后续数据（如 TLS ClientHello 或 POST body）从 header_end_pos + 4 开始
        let raw_headers = &head_buf[..header_end_pos];
        let leftover_payload = &head_buf[header_end_pos + 4..];

        let head_str = String::from_utf8_lossy(raw_headers);
        let is_connect = head_str.starts_with("CONNECT ");

        // 2. 选择可用上游
        let (mut outbound, is_remote) = match self.pick_upstream().await {
            Some(pair) => pair,
            None => {
                let _ = inbound.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-type: text/plain\r\n\r\nAll local and remote cluster targets unreachable").await;
                anyhow::bail!("All local and remote HA cluster targets are unreachable");
            }
        };

        // 3. 构造转发头部：
        // 过滤客户端自身伪造的票证头，仅当 failover 至远程节点时追加正规集群票证
        let ticket_hdr_name = crate::cluster_ticket::CLUSTER_TICKET_HEADER;
        let mut clean_headers = Vec::new();
        for line in head_str.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.to_ascii_lowercase().starts_with(ticket_hdr_name) {
                continue;
            }
            clean_headers.push(trimmed);
        }

        let mut out_head_str = clean_headers.join("\r\n");
        if is_remote && !self.cluster_auth_key.is_empty() && !self.node_id.is_empty() {
            if let Ok(ticket) = crate::cluster_ticket::create_ticket(&self.cluster_auth_key, &self.node_id) {
                out_head_str.push_str("\r\n");
                out_head_str.push_str(ticket_hdr_name);
                out_head_str.push_str(": ");
                out_head_str.push_str(&ticket);
            }
        }
        out_head_str.push_str("\r\n\r\n");

        outbound.write_all(out_head_str.as_bytes()).await?;

        // 若首包携带了后续数据（如 POST body），立刻透传
        if !leftover_payload.is_empty() {
            outbound.write_all(leftover_payload).await?;
        }

        // 5. 零拷贝透明双向流透传
        tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
        Ok(())
    }
}

/// 查找 HTTP 头部结束位置 (\r\n\r\n)，返回首个 \r 的索引
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_ha_forwarder_failover_http() {
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

        let mut client = TcpStream::connect(fwd_addr).await.unwrap();
        client.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();

        let mut resp = vec![0u8; 512];
        let n = client.read(&mut resp).await.unwrap();
        let body = String::from_utf8_lossy(&resp[..n]);
        assert!(body.contains("REMOTE_OK"), "failover response mismatch: {body}");
    }
}
