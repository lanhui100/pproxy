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

use super::engine::{EngineConfig, EngineStats, Kind, ReqHead};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("tunnel: {e}"))
}

const RETRY: u32 = 2; // 总尝试次数（首次 + 1 次重试）

// 超时常量：测试下缩短，避免单测真等满 10 秒（对齐 engine_upstream 的做法）。
#[cfg(not(test))]
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// 首帧应答超时。gate "accept 后不回包/拨号静默挂死"的半死连接是常态：
/// connect_async 自身无超时，缺拨号保护时多端点 failover 永远不会触发
/// （首个端点挂死 → 后续端点永远轮不到 → 每个请求卡满整条建连循环）。
#[cfg(not(test))]
const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);
#[cfg(test)]
const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(600);

/// 建连阶段（写 200 之前）：WS Upgrade + 首帧 + 等 {"ok":true}。
/// 返回分裂后的 sink/stream，供 relay 使用。
type WsPair = (
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, Message>,
    futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>,
);

/// 单端点尝试建连（WS Upgrade + 首帧 + 等 {"ok":true}）
async fn try_establish_url(
    url_str: &str,
    token: &str,
    parsed: &ReqHead,
) -> Result<WsPair, std::io::Error> {
    let mut req = url_str
        .into_client_request()
        .map_err(|e| io(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(io)?,
    );
    // 拨号 + WS Upgrade 共享 DIAL_TIMEOUT 预算：静默端点不得挂死建连循环
    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let (ws, _resp) = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| io(format!("dial timeout after {DIAL_TIMEOUT:?}")))?
        .map_err(io)?;
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

/// 建连阶段：支持多中继端点自动切换（CF 节点不可达/被拒时无缝回退备用 Vercel/Node 节点）。
/// 返回成功使用的端点 URL，供流量统计按出口（CF/Vercel）归账。
async fn establish(
    cfg: &EngineConfig,
    parsed: &ReqHead,
) -> Result<(WsPair, String), std::io::Error> {
    let (url_raw, token) = {
        let (u, t) = cfg.tunnel.borrow().clone();
        (
            u.ok_or_else(|| io("tunnel_url not configured"))?,
            t.ok_or_else(|| io("tunnel token missing"))?,
        )
    };

    let urls: Vec<&str> = url_raw
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let mut last_err = None;
    for u in &urls {
        match try_establish_url(u, &token, parsed).await {
            Ok(pair) => return Ok((pair, (*u).to_string())),
            Err(e) => {
                log::warn!("tunnel establish on {u} for {} failed: {e}", parsed.host);
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| io("no valid tunnel urls configured")))
}

/// 出口归账口径：gate 端点域名含 vercel/vgate → Vercel 出口，其余（gate.ponyjob.top 等）→ CF。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Egress {
    Cf,
    Vercel,
}

pub fn classify_egress(url: &str) -> Egress {
    if url.contains("vercel") || url.contains("vgate") {
        Egress::Vercel
    } else {
        Egress::Cf
    }
}

/// 站点拨测（仪表盘「链接状态」）：经指定 gate 端点完成 WS 升级 + 首帧 {"host","port"} 握手，
/// 随后在热态 WS 管道上测试真实往返延迟（RTT）。
/// 剥离了后台已池化的冷建连开销，准确反映用户真实网络下的热态首包/往返延迟。
pub async fn probe_via_gate(url: &str, token: &str, host: &str, port: u16) -> Result<u64, String> {
    let parsed = ReqHead {
        kind: Kind::Connect,
        host: host.to_string(),
        port,
    };
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

    // 发送首帧声明目标，从此时开始计热态绑定与往返耗时
    let first = serde_json::json!({ "host": parsed.host, "port": parsed.port }).to_string();
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
                    // 首帧绑定成功，在热管道上发 Ping 测试纯净物理 1 RTT
                    let ping_start = tokio::time::Instant::now();
                    if ws_tx.send(Message::Ping(vec![1, 2, 3, 4])).await.is_ok() {
                        let pong_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1500);
                        while let Ok(Some(Ok(p_msg))) = tokio::time::timeout_at(pong_deadline, ws_rx.next()).await {
                            if let Message::Pong(_) = p_msg {
                                let _ = ws_tx.close().await;
                                return Ok(ping_start.elapsed().as_millis() as u64);
                            }
                            if p_msg.is_close() { break; }
                        }
                    }
                    // 若未收到 Pong，回退至首帧绑定耗时
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

/// 接口拨测（仪表盘「接口状态」）：经指定 gate 端点建立 WS 连接后，
/// 在热态 WS 管道上发送 Ping 测试到边缘节点的物理往返延迟（1 RTT）。
/// 剥离冷建连/HTTP 应用层开销，与下方站点测试保持完全一致的测量口径。
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
        let pong_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(2000);
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

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    cfg: &EngineConfig,
    stats: &EngineStats,
) -> std::io::Result<()> {
    // 建连阶段可重试（尚未向客户端写 200，浏览器感知不到）；重试前给
    // gate/edge 一点恢复时间。relay 一旦开始（已写 200）不再重试。
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RETRY {
        match establish(cfg, &parsed).await {
            Ok(((mut ws_tx, ws_rx), used_url)) => {
                // 按实际命中的 gate 端点归账（CF/Vercel 出口分别计数）
                let egress = classify_egress(&used_url);
                match egress {
                    Egress::Cf => &stats.cf_reqs,
                    Egress::Vercel => &stats.vercel_reqs,
                }
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // 4) CONNECT → 客户端回 200；absolute-form → 注入重写首行
                if parsed.kind == Kind::Connect {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;
                } else {
                    let full_req = rebuild_request(head);
                    let bytes = full_req.into_bytes();
                    match egress {
                        Egress::Cf => &stats.cf_up,
                        Egress::Vercel => &stats.vercel_up,
                    }
                    .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    ws_tx
                        .send(Message::Binary(bytes))
                        .await
                        .map_err(io)?;
                }
                return relay(client, ws_tx, ws_rx, stats, egress).await;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < RETRY {
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            }
        }
    }
    let err = last_err.unwrap_or_else(|| io("tunnel establish failed"));
    let msg = format!("502 Bad Gateway: tunnel failed for {}: {}", parsed.host, err);
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        msg.len(),
        msg
    );
    let _ = client.write_all(resp.as_bytes()).await;
    Err(err)
}

/// 双向透传（单任务 select 驱动）：客户端读 ↔ WS 读任一事件即处理，
/// 顺带应答 WS Ping/Pong（CF 侧空闲判定不误杀长连接）。
async fn relay(
    client: TcpStream,
    mut ws_tx: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, Message>,
    mut ws_rx: futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>,
    stats: &EngineStats,
    egress: Egress,
) -> std::io::Result<()> {
    let (mut cr, mut cw) = client.into_split();
    let mut buf = [0u8; 8192];
    loop {
        tokio::select! {
            // 客户端 → WS（隧道出口）
            n = cr.read(&mut buf) => {
                let n = n?;
                if n == 0 { break; }
                match egress {
                    Egress::Cf => &stats.cf_up,
                    Egress::Vercel => &stats.vercel_up,
                }
                .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                ws_tx.send(Message::Binary(buf[..n].to_vec())).await.map_err(io)?;
            }
            // WS（隧道入口） → 客户端
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        match egress {
                            Egress::Cf => &stats.cf_down,
                            Egress::Vercel => &stats.vercel_down,
                        }
                        .fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        cw.write_all(&b).await?;
                    }
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

/// 重写明文请求：首行 absolute-form → origin-form，其余头原样保留（含结尾空行与 body）。
///
/// 严禁用 `head.lines()` 逐行重建：`lines()` 会把结尾的 `\r\n\r\n` 拆出一个额外空元素，
/// 重建后变成 `...\r\n\r\n\r\n`，明文 POST 的 body 会被这 2 字节前缀破坏。
/// 这里用 `split_once("\r\n")` 分离首行与剩余，剩余原样拼回（见 engine_upstream::rebuild_request）。
fn rebuild_request(head: &str) -> String {
    // 退化输入（无 CRLF）时补一个空行，保证请求头完整
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, "\r\n\r\n"));
    format!("{}\r\n{}", rewrite_first_line(first), rest)
}

fn rewrite_first_line(first: &str) -> String {
    let parts: Vec<&str> = first.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return first.to_string();
    }
    let after_scheme = parts[1].strip_prefix("http://").unwrap_or(parts[1]);
    let path_start = after_scheme.find(['/', '?']);
    let path = match path_start {
        Some(i) if after_scheme.as_bytes()[i] == b'?' => {
            format!("/{}", &after_scheme[i..])
        }
        Some(i) => after_scheme[i..].to_string(),
        None => "/".to_string(),
    };
    format!("{} {} {}", parts[0], path, parts[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use tokio_tungstenite::tungstenite::Message;

    /// 起一个最小 WS gate 假端点：读首帧后按 first_reply 应答（Ok(true)→{"ok":true}）。
    async fn spawn_fake_gate(
        first_ok: bool,
    ) -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(_) = msg {
                    let reply = if first_ok {
                        serde_json::json!({ "ok": true }).to_string()
                    } else {
                        serde_json::json!({ "ok": false, "reason": "cf blocked" }).to_string()
                    };
                    ws.send(Message::Text(reply)).await.unwrap();
                    if first_ok {
                        // 模拟 gate 侧 TCP 中继：回显后续二进制
                        while let Some(Ok(msg)) = ws.next().await {
                            if let Message::Binary(b) = msg {
                                let _ = ws.send(Message::Binary(b)).await;
                            } else if msg.is_close() {
                                break;
                            }
                        }
                    }
                    let _ = ws.close(None).await;
                    break;
                }
            }
        });
        Ok((addr, handle))
    }

    fn cfg_with_endpoints(urls: String) -> EngineConfig {
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        EngineConfig { tunnel: trx, ..Default::default() }
    }

    /// 多端点 failover（R4）：首端点拒绝（模拟 CF gate 对 CF 托管目标的平台拒绝）
    /// 必须无缝落到次端点，而不是把拒绝上抛给浏览器。
    #[tokio::test]
    async fn establish_falls_over_to_second_endpoint_on_denied() {
        let (a_addr, a_handle) = spawn_fake_gate(false).await.unwrap();
        let (b_addr, b_handle) = spawn_fake_gate(true).await.unwrap();
        let cfg = cfg_with_endpoints(format!("ws://{a_addr},ws://{b_addr}"));
        let parsed = ReqHead { kind: Kind::Connect, host: "openai.com".into(), port: 443 };

        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "次端点应接住首端点的拒绝: {result:?}");

        // 落在次端点后中继应双向可用：发 PING 收 PING（次端点回显）
        let ((mut ws_tx, mut ws_rx), _used) = result.unwrap();
        ws_tx.send(Message::Binary(b"PING".to_vec())).await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(3), ws_rx.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply.into_data(), b"PING");

        a_handle.abort();
        b_handle.abort();
    }

    /// 端点拨号静默挂死同样触发 failover：connect_async 无内建超时，
    /// 缺 DIAL_TIMEOUT 保护时首个挂死端点会卡死整条建连循环。
    #[tokio::test]
    async fn establish_falls_over_on_silent_endpoint() {
        // 静默端点：只 accept TCP，从不响应（模拟被墙/半死的边缘节点）
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        let (b_addr, b_handle) = spawn_fake_gate(true).await.unwrap();
        let cfg = cfg_with_endpoints(format!("ws://{dead_addr},ws://{b_addr}"));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };

        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "静默端点后应落到次端点: {result:?}");
        b_handle.abort();
    }

    #[test]
    fn rebuild_request_keeps_single_blank_line_and_body() {
        // F2：重建后不得多出 CRLF，否则明文 POST 的 body 被 2 字节前缀破坏
        assert_eq!(
            rebuild_request("POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"),
            "POST /y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
    }

    #[test]
    fn rebuild_request_without_body_ends_with_blank_line() {
        assert_eq!(
            rebuild_request("GET http://x/y HTTP/1.1\r\nHost: x\r\n\r\n"),
            "GET /y HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }
}