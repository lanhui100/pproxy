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
/// 402 租约配额耗尽稳定标记（供客户端原生通知与阻断捕获）
pub const QUOTA_402_MARKER: &str = "HTTP error: 402 Payment Required: Quota Exceeded";

// 超时预算：跨国链路 + TLS/WSS 握手在公网抖动下需充足窗口，回退至 8000ms（对齐 v0.3.50/v0.3.53）
#[cfg(not(test))]
pub const DIAL_TIMEOUT: Duration = Duration::from_millis(8000);
#[cfg(not(test))]
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(8000);

#[cfg(test)]
pub const DIAL_TIMEOUT: Duration = Duration::from_millis(800);
#[cfg(test)]
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(600);

/// 后台补给（prewarm）拨号预算：比交互路径宽松，因为**没有人在等它**。
///
/// 交互路径的 `DIAL_TIMEOUT` 是用户可感知的延迟预算，必须守住；待命池补给是后台行为，
/// 一次慢拨号被砍掉只是少一条待命会话，下一轮 2s 后再补即可，代价远小于「被砍 → 池空 →
/// 后续请求退化为冷建连」。实测 dev→rn 跨境链路 WS 建连 p99≈7~11s、max≈20s，
/// 8s 会稳定切掉 p99 长尾，正是 `tunnel pool refill ... dial timeout after 8s` 的来源。
#[cfg(not(test))]
pub const PREWARM_DIAL_TIMEOUT: Duration = Duration::from_millis(15000);
#[cfg(test)]
pub const PREWARM_DIAL_TIMEOUT: Duration = Duration::from_millis(1500);

pub type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
pub type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
pub type WsPair = (WsSink, WsStream);

pub fn io_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> Error {
    Error::other(e)
}

/// 单端点 WS Upgrade（仅建立热态 WS 管道，不声明目标）：供待命池预建复用。
/// P0：token 先 trim + 非空校验（人肉粘贴/旧文件带入的尾换行不得变成难查的 HeaderValue 错误）。
pub async fn connect_ws(url_str: &str, token: &str) -> Result<WsPair> {
    connect_ws_with_timeout(url_str, token, DIAL_TIMEOUT).await
}

/// 同 [`connect_ws`]，但拨号预算由调用方给定。
///
/// 交互路径用 [`DIAL_TIMEOUT`]（有人在等，必须守住延迟预算）；
/// 后台补给用 [`PREWARM_DIAL_TIMEOUT`]（没人在等，宁可慢一点也别砍掉会话后让池见底）。
pub async fn connect_ws_with_timeout(
    url_str: &str,
    token: &str,
    budget: Duration,
) -> Result<WsPair> {
    let token = token.trim();
    if token.is_empty() {
        return Err(io_err("tunnel token is empty: refusing WS upgrade"));
    }
    let mut req = url_str
        .into_client_request()
        .map_err(|e| io_err(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(io_err)?,
    );

    let dial_deadline = tokio::time::Instant::now() + budget;
    let upgrade = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| io_err(format!("dial timeout after {budget:?}")))?;

    let (ws, _resp) = match upgrade {
        Ok(pair) => pair,
        Err(e) => {
            if let tokio_tungstenite::tungstenite::Error::Http(resp) = &e {
                if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED {
                    return Err(io_err(AUTH_401_MARKER));
                }
                if resp.status().as_u16() == 402 {
                    return Err(io_err(QUOTA_402_MARKER));
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
