//! 延迟探测工具：剥离冷建连开销，真实反映热态 1 RTT 物理延迟。

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{DIAL_TIMEOUT, FIRST_FRAME_TIMEOUT};

/// 接口拨测（仪表盘「接口状态」）：经指定 gate 端点建立 WS 连接后，
/// 在热态 WS 管道上发送 Ping 测试到边缘节点的物理往返延迟（1 RTT）。
pub async fn probe_gate_rtt(url: &str, token: &str) -> Result<u64, String> {
    let mut req = url
        .into_client_request()
        .map_err(|e| format!("bad tunnel url: {e}"))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );

    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let (ws, _resp) = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| format!("dial timeout after {DIAL_TIMEOUT:?}"))?
        .map_err(|e| e.to_string())?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    let ping_start = tokio::time::Instant::now();
    if ws_tx.send(Message::Ping(vec![1, 2, 3, 4])).await.is_ok() {
        let pong_deadline = tokio::time::Instant::now() + Duration::from_millis(2000);
        while let Ok(Some(Ok(p_msg))) = tokio::time::timeout_at(pong_deadline, ws_rx.next()).await {
            if let Message::Pong(_) = p_msg {
                let _ = ws_tx.close().await;
                return Ok(ping_start.elapsed().as_millis() as u64);
            }
            if p_msg.is_close() {
                break;
            }
        }
    }
    let ms = ping_start.elapsed().as_millis() as u64;
    let _ = ws_tx.close().await;
    Ok(ms)
}

/// 站点拨测（仪表盘「链接状态」）：经指定 gate 端点完成 WS 升级 + 首帧 {"host","port"} 握手，
/// 随后在热态 WS 管道上测试真实往返延迟（RTT）。
pub async fn probe_via_gate(url: &str, token: &str, host: &str, port: u16) -> Result<u64, String> {
    let mut req = url
        .into_client_request()
        .map_err(|e| format!("bad tunnel url: {e}"))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );

    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let (ws, _resp) = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| format!("dial timeout after {DIAL_TIMEOUT:?}"))?
        .map_err(|e| e.to_string())?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    let first = serde_json::json!({ "host": host, "port": port }).to_string();
    let bind_start = tokio::time::Instant::now();
    ws_tx
        .send(Message::Text(first))
        .await
        .map_err(|e| e.to_string())?;

    let deadline = tokio::time::Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws_rx.next())
            .await
            .map_err(|_| "first-frame timeout".to_string())?
            .ok_or_else(|| "closed before ok".to_string())?
            .map_err(|e| e.to_string())?;
        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                    let ping_start = tokio::time::Instant::now();
                    if ws_tx.send(Message::Ping(vec![1, 2, 3, 4])).await.is_ok() {
                        let pong_deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
                        while let Ok(Some(Ok(p_msg))) = tokio::time::timeout_at(pong_deadline, ws_rx.next()).await {
                            if let Message::Pong(_) = p_msg {
                                let _ = ws_tx.close().await;
                                return Ok(ping_start.elapsed().as_millis() as u64);
                            }
                            if p_msg.is_close() { break; }
                        }
                    }
                    let ms = bind_start.elapsed().as_millis() as u64;
                    let _ = ws_tx.close().await;
                    return Ok(ms);
                }
                return Err(format!(
                    "denied: {}",
                    v.get("reason").and_then(|r| r.as_str()).unwrap_or("?")
                ));
            }
            Message::Ping(p) => {
                let _ = ws_tx.send(Message::Pong(p)).await;
            }
            Message::Close(c) => return Err(format!("closed: {c:?}")),
            _ => {}
        }
    }
}
