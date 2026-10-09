//! WS ↔ TCP 双向透传状态机（支持 TCP 半关闭与 Ping/Pong 自动保活）。

use std::io::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
    pub rn_up: AtomicU64,
    pub rn_down: AtomicU64,
}

impl TrafficCounter for AtomicTrafficStats {
    fn record_up(&self, egress: Egress, bytes: u64) {
        match egress {
            Egress::Cf => &self.cf_up,
            Egress::Vercel => &self.vercel_up,
            Egress::NativeVps => &self.rn_up,
        }
        .fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_down(&self, egress: Egress, bytes: u64) {
        match egress {
            Egress::Cf => &self.cf_down,
            Egress::Vercel => &self.vercel_down,
            Egress::NativeVps => &self.rn_down,
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
    ws_tx: WsSink,
    ws_rx: WsStream,
    egress: Egress,
    stats: &C,
) -> Result<()> {
    relay_bidir_ws_bounded(client, ws_tx, ws_rx, egress, stats, None, None)
        .await
        .map(|_| ())
}

/// relay 有界生命周期命中类别（数据面日志 reason 用，契约 wave-pproxy-accept-loop §3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayDeadlineHit {
    /// 双向（client↔ws）连续无字节窗口达到空闲超时。
    Idle,
    /// relay 从启动起的总时长达到绝对上限。
    Absolute,
}

/// 带可选有界生命周期的双向透传（契约 §3 语义的运输实现）：
/// - `idle`：双向连续无字节窗口（空闲判定，活动即重置）；命中 → 强制断连返回 `Idle`。
/// - `absolute`：自启动起的总时长硬界（不随活动重置）；命中 → 强制断连返回 `Absolute`。
/// - 均为 `None` 时行为与 [`relay_bidir_ws`] 完全一致（既有调用方零改动）。
/// 命中/完成/错误均双端关闭（cw shutdown + ws sink drop），复用既有关闭语义，
/// 不引入额外常驻句柄/线程（timer 随 select 分支创建与丢弃）。
pub async fn relay_bidir_ws_bounded<C: TrafficCounter>(
    client: TcpStream,
    mut ws_tx: WsSink,
    mut ws_rx: WsStream,
    egress: Egress,
    stats: &C,
    idle: Option<Duration>,
    absolute: Option<Duration>,
) -> Result<Option<RelayDeadlineHit>> {
    let (mut cr, mut cw) = client.into_split();
    let mut buf = [0u8; 8192];
    let mut client_done = false;
    let start = tokio::time::Instant::now();
    // 空闲 deadline 随每次双向活动重置；无 idle 时给一个远端值（分支被守卫禁用）。
    let idle_window = idle.unwrap_or(Duration::from_secs(86400 * 365));
    let mut idle_deadline = start + idle_window;
    let absolute_deadline = start + absolute.unwrap_or(Duration::from_secs(86400 * 365));

    loop {
        tokio::select! {
            // 空闲 deadline：双向连续无字节窗口到点 → 强制断连（死隧道释放 permit）
            _ = tokio::time::sleep_until(idle_deadline), if idle.is_some() => {
                let _ = cw.shutdown().await;
                return Ok(Some(RelayDeadlineHit::Idle));
            }
            // 绝对 deadline：单次 relay 总时长硬界（不随活动重置）
            _ = tokio::time::sleep_until(absolute_deadline), if absolute.is_some() => {
                let _ = cw.shutdown().await;
                return Ok(Some(RelayDeadlineHit::Absolute));
            }
            // 客户端 → WS（隧道出口）：客户端单向 EOF 时不再向 WS 发送，但保持下行接收
            n = cr.read(&mut buf), if !client_done => {
                let n = n?;
                if n == 0 {
                    client_done = true;
                } else {
                    stats.record_up(egress, n as u64);
                    idle_deadline = tokio::time::Instant::now() + idle_window;
                    ws_tx.send(Message::Binary(buf[..n].to_vec())).await.map_err(io_err)?;
                }
            }
            // WS（隧道入口） → 客户端
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        stats.record_down(egress, b.len() as u64);
                        idle_deadline = tokio::time::Instant::now() + idle_window;
                        cw.write_all(&b).await?;
                    }
                    Some(Ok(Message::Ping(p))) => {
                        idle_deadline = tokio::time::Instant::now() + idle_window;
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
    Ok(None)
}
