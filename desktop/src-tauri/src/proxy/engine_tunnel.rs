//! WS 隧道客户端（M6 spec §3）：经 wss 连接 gate worker 中继 TLS 字节。
//!
//! 流程：Upgrade（Bearer tunnel_token）→ 首帧 JSON {"host","port"} →
//! {"ok":true} → 回 200/注入首行 → 双向透传。R4：任何失败即报错关闭，
//! 绝不静默回落直连。

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

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    cfg: &EngineConfig,
) -> std::io::Result<()> {
    let url = cfg
        .tunnel_url
        .as_deref()
        .ok_or_else(|| io("tunnel_url not configured"))?;
    let token = cfg
        .tunnel_token
        .as_deref()
        .ok_or_else(|| io("tunnel token missing"))?;

    // 1) WS Upgrade + Bearer 凭据
    let mut req = url
        .into_client_request()
        .map_err(|e| io(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| io(e))?,
    );
    let (mut ws_tx, mut ws_rx) =
        { let (ws, _resp) = tokio_tungstenite::connect_async(req).await.map_err(io)?; ws.split() };

    // 2) 首帧：目标 host/port
    let first = serde_json::json!({ "host": parsed.host, "port": parsed.port }).to_string();
    ws_tx.send(Message::Text(first)).await.map_err(io)?;

    // 3) 等待 {"ok":true}（10s 超时）
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
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
                    break;
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

    // 4) CONNECT → 客户端回 200；absolute-form → 注入重写首行
    if parsed.kind == Kind::Connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
    } else {
        let first_line = rewrite_first_line(head);
        ws_tx.send(Message::Binary(first_line.into_bytes()))
            .await
            .map_err(io)?;
    }

    // 5) 双向透传：client↔ws（select 驱动，任一方向结束即收尾）
    let (mut cr, mut cw) = client.into_split();
    let c2w = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = cr.read(&mut buf).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(());
            }
            ws_tx.send(Message::Binary(buf[..n].to_vec())).await.map_err(io)?;
        }
    };
    let w2c = async {
        while let Some(msg) = ws_rx.next().await {
            match msg.map_err(io)? {
                Message::Binary(b) => cw.write_all(&b).await?,
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<_, std::io::Error>(())
    };
    tokio::try_join!(c2w, w2c)?;
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
