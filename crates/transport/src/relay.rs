//! WS ↔ TCP 双向透传状态机（支持 TCP 半关闭与 Ping/Pong 自动保活）。

use std::io::Result;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{io_err, WsSink, WsStream};
use crate::route::Egress;

/// 传输流量统计接口
pub trait TrafficCounter: Send + Sync {
    fn record_up(&self, egress: Egress, bytes: u64);
    fn record_down(&self, egress: Egress, bytes: u64);
}

/// 默认空统计实现
pub struct NoopTrafficCounter;
impl TrafficCounter for NoopTrafficCounter {
    fn record_up(&self, _egress: Egress, _bytes: u64) {}
    fn record_down(&self, _egress: Egress, _bytes: u64) {}
}

impl TrafficCounter for () {
    fn record_up(&self, _egress: Egress, _bytes: u64) {}
    fn record_down(&self, _egress: Egress, _bytes: u64) {}
}

/// 基础原子计数统计实现
#[derive(Default)]
pub struct AtomicTrafficStats {
    pub cf_up: AtomicU64,
    pub cf_down: AtomicU64,
    pub vercel_up: AtomicU64,
    pub vercel_down: AtomicU64,
}

impl TrafficCounter for AtomicTrafficStats {
    fn record_up(&self, egress: Egress, bytes: u64) {
        match egress {
            Egress::Cf => &self.cf_up,
            Egress::Vercel => &self.vercel_up,
        }
        .fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_down(&self, egress: Egress, bytes: u64) {
        match egress {
            Egress::Cf => &self.cf_down,
            Egress::Vercel => &self.vercel_down,
        }
        .fetch_add(bytes, Ordering::Relaxed);
    }
}

/// 双向透传：客户端读 ↔ WS 读任一事件即处理。
/// 关键修复（SEC-02）：支持 TCP 半关闭（Half-Close）。
/// 客户端发送完请求 Body 并关闭写半区时（cr.read() == 0），仅置位 client_done，
/// 允许下行通道继续将服务端响应完整接收并回写给客户端，绝不直接中断连接。
pub async fn relay_bidir_ws<C: TrafficCounter>(
    client: TcpStream,
    mut ws_tx: WsSink,
    mut ws_rx: WsStream,
    egress: Egress,
    stats: &C,
) -> Result<()> {
    let (mut cr, mut cw) = client.into_split();
    let mut buf = [0u8; 8192];
    let mut client_done = false;

    loop {
        tokio::select! {
            // 客户端 → WS（隧道出口）：客户端单向 EOF 时不再向 WS 发送，但保持下行接收
            n = cr.read(&mut buf), if !client_done => {
                let n = n?;
                if n == 0 {
                    client_done = true;
                } else {
                    stats.record_up(egress, n as u64);
                    ws_tx.send(Message::Binary(buf[..n].to_vec())).await.map_err(io_err)?;
                }
            }
            // WS（隧道入口） → 客户端
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        stats.record_down(egress, b.len() as u64);
                        cw.write_all(&b).await?;
                    }
                    Some(Ok(Message::Ping(p))) => {
                        ws_tx.send(Message::Pong(p)).await.map_err(io_err)?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => return Err(io_err(e)),
                    _ => {}
                }
            }
        }
    }

    let _ = cw.shutdown().await;
    Ok(())
}
