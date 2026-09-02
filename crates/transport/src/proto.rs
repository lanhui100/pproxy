//! WS 隧道底层协议与首帧握手。

use std::io::{Error, Result};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// 401 鉴权失败稳定标记（供自愈状态机捕获）
pub const AUTH_401_MARKER: &str = "HTTP error: 401 Unauthorized";

// 超时预算：生产环境下收敛超时预算，防止单端点故障拖跨整条链路
#[cfg(not(test))]
pub const DIAL_TIMEOUT: Duration = Duration::from_millis(4000);
#[cfg(not(test))]
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(3500);

#[cfg(test)]
pub const DIAL_TIMEOUT: Duration = Duration::from_millis(800);
#[cfg(test)]
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(600);

pub type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
pub type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
pub type WsPair = (WsSink, WsStream);

pub fn io_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> Error {
    Error::other(e)
}

/// 单端点 WS Upgrade（仅建立热态 WS 管道，不声明目标）：供待命池预建复用。
pub async fn connect_ws(url_str: &str, token: &str) -> Result<WsPair> {
    let mut req = url_str
        .into_client_request()
        .map_err(|e| io_err(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(io_err)?,
    );

    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let upgrade = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| io_err(format!("dial timeout after {DIAL_TIMEOUT:?}")))?;

    let (ws, _resp) = match upgrade {
        Ok(pair) => pair,
        Err(e) => {
            if let tokio_tungstenite::tungstenite::Error::Http(resp) = &e {
                if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED {
                    return Err(io_err(AUTH_401_MARKER));
                }
            }
            return Err(io_err(e));
        }
    };
    Ok(ws.split())
}

/// 在已建立（或待命池取出）的 WS 管道上声明目标：Text 首帧 {"host": ..., "port": ...} → 等 {"ok": true}。
pub async fn bind_target(
    mut ws_tx: WsSink,
    mut ws_rx: WsStream,
    host: &str,
    port: u16,
) -> Result<WsPair> {
    let first = serde_json::json!({ "host": host, "port": port }).to_string();
    ws_tx.send(Message::Text(first)).await.map_err(io_err)?;

    let deadline = tokio::time::Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws_rx.next())
            .await
            .map_err(|_| io_err("first-frame timeout"))?
            .ok_or_else(|| io_err("closed before ok"))?
            .map_err(io_err)?;
        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).map_err(io_err)?;
                if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                    return Ok((ws_tx, ws_rx));
                }
                return Err(io_err(format!(
                    "denied: {}",
                    v.get("reason").and_then(|r| r.as_str()).unwrap_or("?")
                )));
            }
            Message::Ping(p) => ws_tx.send(Message::Pong(p)).await.map_err(io_err)?,
            Message::Close(c) => return Err(io_err(format!("closed: {c:?}"))),
            _ => {}
        }
    }
}

/// 单端点冷建连（WS Upgrade + 首帧目标声明）
pub async fn try_establish_url(
    url_str: &str,
    token: &str,
    host: &str,
    port: u16,
) -> Result<WsPair> {
    let (tx, rx) = connect_ws(url_str, token).await?;
    bind_target(tx, rx, host, port).await
}
