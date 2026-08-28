//! WS 隧道客户端（M6 spec §3）：经 wss 连接 gate worker 中继 TLS 字节。
//!
//! 流程：Upgrade（Bearer tunnel_token）→ 首帧 JSON {"host","port"} →
//! {"ok":true} → 回 200/注入首行 → 双向透传。R4：任何失败即报错关闭，
//! 绝不静默回落直连。
//!
//! 2026-08 加固：
//! - 写入 200 之前的失败（WS 建连/首帧超时/gate denied）自动重试一次，
//!   缓解 edge PoP 抖动（YouTube 等偶发 timeout）；
//! - 透传期应答 WS Ping/Pong，避免 CF 侧长时间空闲判定断连。

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use super::engine::{EngineConfig, Kind, ReqHead};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("tunnel: {e}"))
}

const RETRY: u32 = 2; // 总尝试次数（首次 + 1 次重试）
const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 建连阶段（写 200 之前）：WS Upgrade + 首帧 + 等 {"ok":true}。
/// 返回分裂后的 sink/stream，供 relay 使用。
async fn establish(
    cfg: &EngineConfig,
    parsed: &ReqHead,
) -> Result<
    (
        futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, Message>,
        futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>,
    ),
    std::io::Error,
> {
    let url = cfg.tunnel_url.as_deref().ok_or_else(|| io("tunnel_url not configured"))?;
    let token = cfg.tunnel_token.as_deref().ok_or_else(|| io("tunnel token missing"))?;

    let mut req = url
        .into_client_request()
        .map_err(|e| io(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(io)?,
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(req).await.map_err(io)?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    let first = serde_json::json!({ "host": parsed.host, "port": parsed.port }).to_string();
    ws_tx.send(Message::Text(first)).await.map_err(io)?;

    let deadline = tokio::time::Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws_rx.next())
            .await
            .map_err(|_| io("first-frame timeout"))?
            .ok_or_else(|| io("closed before ok"))?
            .map_err(io)?;
        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).map_err(io)?;
                if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                    return Ok((ws_tx, ws_rx));
                }
                return Err(io(format!(
                    "denied: {}",
                    v.get("reason").and_then(|r| r.as_str()).unwrap_or("?")
                )));
            }
            Message::Ping(p) => ws_tx.send(Message::Pong(p)).await.map_err(io)?,
            Message::Close(c) => return Err(io(format!("closed: {c:?}"))),
            _ => {}
        }
    }
}

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    cfg: &EngineConfig,
) -> std::io::Result<()> {
    // 建连阶段可重试（尚未向客户端写 200，浏览器感知不到）；重试前给
    // gate/edge 一点恢复时间。relay 一旦开始（已写 200）不再重试。
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RETRY {
        match establish(cfg, &parsed).await {
            Ok((mut ws_tx, ws_rx)) => {
                // 4) CONNECT → 客户端回 200；absolute-form → 注入重写首行
                if parsed.kind == Kind::Connect {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;
                } else {
                    let first_line = rewrite_first_line(head);
                    ws_tx
                        .send(Message::Binary(first_line.into_bytes()))
                        .await
                        .map_err(io)?;
                }
                return relay(client, ws_tx, ws_rx).await;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < RETRY {
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io("tunnel establish failed")))
}

/// 双向透传（单任务 select 驱动）：客户端读 ↔ WS 读任一事件即处理，
/// 顺带应答 WS Ping/Pong（CF 侧空闲判定不误杀长连接）。
async fn relay(
    client: TcpStream,
    mut ws_tx: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, Message>,
    mut ws_rx: futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>,
) -> std::io::Result<()> {
    let (mut cr, mut cw) = client.into_split();
    let mut buf = [0u8; 8192];
    loop {
        tokio::select! {
            // 客户端 → WS（隧道出口）
            n = cr.read(&mut buf) => {
                let n = n?;
                if n == 0 { break; }
                ws_tx.send(Message::Binary(buf[..n].to_vec())).await.map_err(io)?;
            }
            // WS（隧道入口） → 客户端
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => { cw.write_all(&b).await?; }
                    Some(Ok(Message::Ping(p))) => { ws_tx.send(Message::Pong(p)).await.map_err(io)?; }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(e)) => return Err(io(e)),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn rewrite_first_line(head: &str) -> String {
    let parts: Vec<&str> = head.lines().next().unwrap_or("").splitn(3, ' ').collect();
    if parts.len() != 3 {
        return head.lines().next().unwrap_or("").to_string();
    }
    let path = parts[1]
        .strip_prefix("http://")
        .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or("/");
    format!("{} {} {}", parts[0], path, parts[2])
}
