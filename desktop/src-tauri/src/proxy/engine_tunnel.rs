//! WS 隧道客户端（M6 spec §3）：经 wss 连接 gate worker 中继 TLS 字节。
//!
//! 流程：Upgrade（Bearer tunnel_token）→ 首帧 JSON {"host","port"} →
//! {"ok":true} → 回 200/注入首行 → 双向透传。R4：任何失败即报错关闭，
//! 绝不静默回落直连。
//!
//! 底层核心（TunnelPool、Half-Close relay、智能排序与握手协议）已统一下沉至 `pproxy-transport`。

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[allow(unused_imports)]
pub use pproxy_transport::{
    bind_target, classify_egress, is_google_or_ai_host, order_endpoints, probe_gate_rtt,
    probe_via_gate, relay_bidir_ws, token_fp8_of, try_establish_url, AUTH_401_MARKER, Egress,
    TunnelPool, WsPair,
};

use super::engine::{EngineConfig, EngineStats, Kind, ReqHead};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("tunnel: {e}"))
}

/// 401 自愈冷却：进程内距上次自愈至少间隔此时长（single-flight 负缓存，
/// 防止 token 轮换瞬间 N 个并发 CONNECT 各自同步读 keyring + 重复重试）。
#[cfg(not(test))]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(0);
static LAST_SELF_HEAL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// 内存 token 缺失时的磁盘重读冷却（watch 播种被瞬时 keyring 故障毒化时，
/// 建连侧主动重读一次而非直接 502；冷却防每个 CONNECT 都读 keyring）。
#[cfg(not(test))]
const MISSING_REFRESH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const MISSING_REFRESH_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(0);
static LAST_MISSING_REFRESH: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// P0/E10：探针/流量错误分类（禁止把门禁 denied 误判为鉴权 401）。
/// 返回值：auth401（Upgrade 401）/ denied（门禁 acl/colo/egress）/ timeout / closed / other
pub fn classify_probe_error(e: &str) -> &'static str {
    if e.contains(AUTH_401_MARKER) {
        "auth401"
    } else if e.contains("denied:") {
        "denied"
    } else if e.contains("timeout") || e.contains("timed out") {
        "timeout"
    } else if e.contains("closed") || e.contains("close") {
        "closed"
    } else if e.contains("no token") {
        "no_token"
    } else {
        "other"
    }
}

/// P0：401 是否可重试——同 token 重试 5 次必败，遇 401 直接返回（由调用方 502），不进退避循环。
pub fn is_auth_failure(e: &std::io::Error) -> bool {
    e.to_string().contains(AUTH_401_MARKER)
}

/// token 指纹（只打指纹禁明文）：sha256 前 8 hex，用于 401 排障回答“当次哪一枚”。
/// 口径统一走 `pproxy_transport::token_fp8_of`（B-S-2：桌面端三处指纹实现收敛）。
fn token_fp8(token: &str) -> String {
    token_fp8_of(token)
}

const RETRY: u32 = 5; // 总尝试次数（首次 + 4 次重试）

/// 建连阶段：支持多中继端点自动切换（CF 节点不可达/被拒时无缝回退备用 Vercel/Node 节点）。
/// 返回成功使用的端点 URL，供流量统计按出口（CF/Vercel）归账。
///
/// 优先从待命池 checkout（已预建 WS Upgrade，热态），命中则只做首帧声明（1 RTT）；
/// 池 miss / 池会话死亡（bind 失败）回落到冷建连全流程。
async fn establish(
    cfg: &EngineConfig,
    parsed: &ReqHead,
) -> Result<(WsPair, String), std::io::Error> {
    let (url_raw, token) = {
        let (u, t) = cfg.tunnel.borrow().clone();
        match (u, t) {
            (Some(u), Some(t)) => (u, t),
            // 内存缺失：watch 可能被瞬时 keyring 故障毒化——冷却外主动重读一次磁盘
            _ => {
                let should_refresh = {
                    let g = LAST_MISSING_REFRESH.lock().unwrap_or_else(|p| p.into_inner());
                    match *g {
                        Some(t) => t.elapsed() >= MISSING_REFRESH_COOLDOWN,
                        None => true,
                    }
                };
                if should_refresh {
                    *LAST_MISSING_REFRESH.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(std::time::Instant::now());
                    let fresh =
                        tokio::task::spawn_blocking(crate::tunnel_config_load).await.unwrap_or((None, None));
                    if let (Some(u), Some(t)) = fresh {
                        if !t.trim().is_empty() {
                            log::info!("tunnel watch missing: refreshed from disk (disk_fp8={}), resuming establish", token_fp8(&t));
                            let _ = crate::ensure_tunnel_watch().send((Some(u.clone()), Some(t.clone())));
                            (u, t)
                        } else {
                            return Err(io("tunnel token missing"));
                        }
                    } else {
                        return Err(io("tunnel token missing"));
                    }
                } else {
                    return Err(io("tunnel token missing"));
                }
            }
        }
    };

    let urls: Vec<&str> = url_raw
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    // 按目标 host 重排端点优先级（Google/AI 目标 → Vercel 优先；其他 → CF 优先）
    let urls = order_endpoints(urls, &parsed.host);
    // 合规出口专项（2026-09-19，与 server/engine connect.rs 同源修复）：Cloud Code 系
    // host 必须经 Vercel 物理出口，两道防线：
    // 1. 跳过池化——conserve 模式下池里只有 CF 会话，checkout 会在 vgate 无会话时
    //    降级命中 CF 会话；
    // 2. 端点列表过滤为仅 Vercel；无 Vercel 端点时 fail-closed 拒绝（不降级 CF——
    //    CF 是轮换 anycast，降级回去仍被 Google 400，与故障现场不可区分）。
    let urls = if pproxy_transport::requires_compliant_egress(&parsed.host) {
        let vercel_only = pproxy_transport::compliant_egress_endpoints(&urls);
        if vercel_only.is_empty() {
            log::warn!(
                "compliant-egress host {} but no Vercel gate endpoint configured; refusing (CF egress is geo-rejected by Google)",
                parsed.host
            );
            return Err(io("compliant-egress host requires a Vercel gate endpoint; none configured"));
        }
        vercel_only
    } else {
        urls
    };
    // Must-fix G/E8：拨号顺序可观测（host→有序端点 + 内存 token 指纹，只打指纹禁明文）
    log::debug!("tunnel order for {}: {:?} (mem_fp8={})", parsed.host, urls, token_fp8(&token));

    // 方案 A：先取池会话，bind 热态首帧（按当次内存 token 指纹匹配，失配即 miss）
    let mem_fp8 = token_fp8(&token);
    if let Some((tx, rx, used_url)) = cfg.pool.checkout(&urls, Some(&mem_fp8)) {
        match bind_target(tx, rx, &parsed.host, parsed.port).await {
            Ok(pair) => {
                log::debug!("tunnel established from pool via {used_url} for {}", parsed.host);
                return Ok((pair, used_url));
            }
            Err(e) => {
                log::warn!("pooled session bind failed for {} on {used_url}: {e}", parsed.host);
            }
        }
    }

    let mut first_err: Option<std::io::Error> = None;
    let mut first_401: Option<std::io::Error> = None;
    let mut last_err = None;
    let mut saw_401 = false;
    let mut saw_402 = false;
    let mut quota_err: Option<std::io::Error> = None;

    for u in &urls {
        match try_establish_url(u, &token, &parsed.host, parsed.port).await {
            Ok(pair) => return Ok((pair, (*u).to_string())),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("402") || msg.contains("Quota Exceeded") {
                    saw_402 = true;
                    quota_err = Some(io(msg));
                    break; // 审查修复（P0-1 & P0-2）：账户配额耗尽，立即短路退出端点重试风暴
                }
                let egress = classify_egress(u);
                if msg.contains(AUTH_401_MARKER) {
                    saw_401 = true;
                    if first_401.is_none() { first_401 = Some(io(e.to_string())); }
                }
                if first_err.is_none() { first_err = Some(io(e.to_string())); }
                log::warn!("tunnel establish on {u} for {} failed (egress={egress:?}, mem_fp8={mem_fp8}): {e}", parsed.host);
                last_err = Some(e);
            }
        }
    }

    if saw_402 {
        let err_desc = quota_err.map(|e| e.to_string()).unwrap_or_else(|| "Quota Exceeded".into());
        return Err(io(format!("402: quota_exceeded —— 您的出海流量配额已耗尽，请联系管理员扩充配额 ({err_desc})")));
    }

    // 401 自愈：全部端点鉴权失败时，凭据可能已被外部更新（轮换）——重读凭据并刷新重试。
    // Must-fix G（负缓存兑现）：成功轮换与无新值/同值统一置位冷却——持续 401 时每个 CONNECT
    // 都读 keyring 是风暴放大器；冷却命中/重读结果一律打日志。
    if saw_401 {
        let should_check = {
            let g = LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner());
            match *g {
                Some(t) => {
                    let ok = t.elapsed() >= SELF_HEAL_COOLDOWN;
                    if !ok {
                        log::debug!("self-heal cooldown active: skipping keyring re-read");
                    } else {
                        log::debug!("self-heal cooldown expired: re-reading keyring");
                    }
                    ok
                }
                None => {
                    log::debug!("self-heal first 401: re-reading keyring");
                    true
                }
            }
        };
        let (fresh_url, fresh_token) = if should_check {
            tokio::task::spawn_blocking(crate::tunnel_config_load).await.unwrap_or((None, None))
        } else {
            (None, None)
        };
        // 负缓存：重读无新值/同值同样置位（30s 内不再读 keyring），并打结果日志
        let mut healed = false;
        if let (Some(u), Some(t)) = (fresh_url, fresh_token) {
            let current = cfg.tunnel.borrow().clone();
            // 严防倒灌：仅当重读出来的 token 与当前内存 token 不同、且确有值时才轮换；
            // 且必须确保 fresh_token 经过基本有效性检验（不能是空串）。
            if Some(t.clone()) != current.1 && t != token && !t.trim().is_empty() {
                log::info!("tunnel token rotated on disk (mem_fp8={} disk_fp8={}), refreshing watch and retrying once", token_fp8(&token), token_fp8(&t));
                // P0：自愈只广播 watch 不落盘会造成 probe/流量分叉 + 重启丢失——成功即落盘
                //（落盘失败不阻断本次内存重试，但必须告警——内存-磁盘分裂特批口）。
                if let Err(e) = crate::persist_healed_tunnel_token(&t, "self_heal_401") {
                    log::warn!("self-heal persist failed (memory-only retry): {e}");
                }
                *LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
                healed = true;
                // 调序（B-P2-3）：先排空旧池再广播 watch——否则 maintain 可能在中间按新 token
                // 预建又被清空，多余 churn 一次
                cfg.pool.invalidate();
                let _ = crate::ensure_tunnel_watch().send((Some(u.clone()), Some(t.clone())));
                let urls2: Vec<&str> = u.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                let urls2 = order_endpoints(urls2, &parsed.host);
                // 合规出口：自愈重试同样过滤为仅 Vercel（与主路径一致，见上方注释）
                let urls2 = if pproxy_transport::requires_compliant_egress(&parsed.host) {
                    pproxy_transport::compliant_egress_endpoints(&urls2)
                } else {
                    urls2
                };
                log::debug!("self-heal re-establish order for {}: {:?}", parsed.host, urls2);
                for u2 in &urls2 {
                    match try_establish_url(u2, &t, &parsed.host, parsed.port).await {
                        Ok(pair) => return Ok((pair, (*u2).to_string())),
                        Err(e) => {
                            let egress = classify_egress(u2);
                            log::warn!("tunnel re-establish on {u2} for {} failed (egress={egress:?}, disk_fp8={}): {e}", parsed.host, token_fp8(&t));
                            last_err = Some(e);
                        }
                    }
                }
            } else {
                log::debug!("self-heal re-read: no-change (mem_fp8={})", token_fp8(&token));
            }
        } else {
            log::debug!("self-heal re-read: no fresh credential on disk");
        }
        if !healed {
            // 负缓存兑现：无新值/同值同样进 30s 冷却，防 keyring 风暴
            *LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
        }
    }

    // 无新值可 heal：磁盘==内存==旧值（服务端已轮换）→ 明确 needs-reinput，
    // 保留首错 + 首个 401，不静默成普通 502。
    // 消费点（A-P1-9）：`connect_and_relay` 识别此前缀并映射为 502 响应体文案
    //（含重贴指引，不含明文）；日志侧保留 mem_fp8/egress 取证行。
    if saw_401 {
        if let Some(e401) = first_401 {
            let first = first_err.map(|e| e.to_string()).unwrap_or_default();
            return Err(io(format!(
                "needs-reinput: tunnel token rejected by gate (mem_fp8={mem_fp8}): {e401}; first_error: {first} —— 请在「设置 → 隧道中继」重贴授权码"
            )));
        }
    }

    // 审查加固（P0-2 & P0-3）：捕获 402 Quota Exceeded，阻断无效重试并向用户友好回显
    if let Some(ref err) = last_err {
        let msg = err.to_string();
        if msg.contains("402") || msg.contains("Quota Exceeded") {
            return Err(io("402: quota_exceeded —— 您的出海流量配额已耗尽，请联系管理员扩充配额"));
        }
    }

    Err(last_err.unwrap_or_else(|| io("no valid tunnel urls configured")))
}

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    cfg: &EngineConfig,
    stats: &EngineStats,
) -> std::io::Result<()> {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RETRY {
        match establish(cfg, &parsed).await {
            Ok(((mut ws_tx, ws_rx), used_url)) => {
                let egress = classify_egress(&used_url);
                match egress {
                    Egress::Cf => &stats.cf_reqs,
                    Egress::Vercel => &stats.vercel_reqs,
                    Egress::NativeVps => &stats.rn_reqs,
                }
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                        Egress::NativeVps => &stats.rn_up,
                    }
                    .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    use futures_util::SinkExt as _;
                    ws_tx
                        .send(tokio_tungstenite::tungstenite::Message::Binary(bytes))
                        .await
                        .map_err(io)?;
                }
                return relay_bidir_ws(client, ws_tx, ws_rx, egress, stats).await;
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("402") || msg.contains("quota_exceeded") {
                    last_err = Some(e);
                    break; // 审查修复（P0-1）：遇 402 立即短路退出，杜绝 5 次无效重试与延迟风暴
                }
                // P0：401 同 token 重试必败——跳过 RETRY 退避，直接 502（省 ~1.5s 延迟与 keyring 风暴）
                if is_auth_failure(&e) {
                    last_err = Some(e);
                    break;
                }
                last_err = Some(e);
                if attempt + 1 < RETRY {
                    let backoff = std::time::Duration::from_millis(
                        50 * (1 << attempt.min(6)) + (rand::random::<u64>() % 50),
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    let err = last_err.unwrap_or_else(|| io("tunnel establish failed"));
    // A-P1-9 消费点：needs-reinput 前缀映射为面向用户的 502 文案（含重贴指引）；
    // 内部取证行（mem_fp8/egress/逐端点）只进日志，不进响应体（禁指纹外泄到客户端）。
    let err_s = err.to_string();
    let user_msg = if err_s.starts_with("needs-reinput:") {
        format!("502 Bad Gateway: tunnel token rejected for {} —— 请在「设置 → 隧道中继」重贴授权码", parsed.host)
    } else {
        format!("502 Bad Gateway: tunnel failed for {}: {}", parsed.host, err)
    };
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        user_msg.len(),
        user_msg
    );
    let _ = client.write_all(resp.as_bytes()).await;
    Err(err)
}

/// 重写明文请求：首行 absolute-form → origin-form，其余头原样保留（含结尾空行与 body）。
fn rebuild_request(head: &str) -> String {
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
        Some(idx) => &after_scheme[idx..],
        None => "/",
    };
    format!("{} {} {}", parts[0], path, parts[2])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    async fn spawn_fake_gate(ok: bool) -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut ws) = accept_async(stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            match msg {
                                Message::Text(t) => {
                                    let _v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                                    let resp = if ok {
                                        serde_json::json!({ "ok": true }).to_string()
                                    } else {
                                        serde_json::json!({ "ok": false, "reason": "mock_denied" }).to_string()
                                    };
                                    let _ = ws.send(Message::Text(resp)).await;
                                }
                                Message::Binary(b) => {
                                    let _ = ws.send(Message::Binary(b)).await;
                                }
                                Message::Ping(p) => {
                                    let _ = ws.send(Message::Pong(p)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        Ok((addr, handle))
    }

    fn cfg_with_endpoints(urls: String) -> EngineConfig {
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(crate::proxy::pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        let (_utx, urx) = watch::channel(None);
        let (_ptx, prx) = watch::channel((None, None));
        EngineConfig {
            listen_addr: "127.0.0.1:0".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
            pool: TunnelPool::with_size(prx, 0),
        }
    }

    fn cfg_with_pool(urls: String, pool: Arc<TunnelPool>) -> EngineConfig {
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(crate::proxy::pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        let (_utx, urx) = watch::channel(None);
        EngineConfig {
            listen_addr: "127.0.0.1:0".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
            pool,
        }
    }

    #[tokio::test]
    async fn establish_falls_over_to_second_endpoint_on_denied() {
        let (a_addr, a_handle) = spawn_fake_gate(false).await.unwrap();
        let (b_addr, b_handle) = spawn_fake_gate(true).await.unwrap();
        let cfg = cfg_with_endpoints(format!("ws://{a_addr},ws://{b_addr}"));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };

        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "首端点 denied 后应落到次端点: {result:?}");

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

    #[tokio::test]
    async fn establish_falls_over_on_silent_endpoint() {
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

    #[test]
    fn google_host_detection_matches_gate_policy() {
        for h in [
            "google.com",
            "www.google.com",
            "google.com.hk",
            "www.google.co.jp",
            "accounts.google.com",
            "generativelanguage.googleapis.com",
            "oauth2.googleapis.com",
            "www.gstatic.com",
            "deepmind.google",
            "antigravity.google",
            "api.antigravity.google",
            "labs.google",
            "g.co",
            "openai.com",
            "api.openai.com",
            "chatgpt.com",
            "claude.ai",
            "anthropic.com",
        ] {
            assert!(is_google_or_ai_host(h), "应识别为 Google/AI 系: {h}");
        }
        for h in ["github.com", "youtube.com", "notgoogleapis.com", "googleapis.com.evil.cn", "baidu.com"] {
            assert!(!is_google_or_ai_host(h), "不应识别为 Google/AI 系: {h}");
        }
    }

    #[test]
    fn endpoint_order_prefers_vercel_for_google_and_ai() {
        std::env::set_var("PPROXY_CONSERVE_VERCEL", "0");
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];
        let ordered = order_endpoints(urls.clone(), "oauth2.googleapis.com");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "Google 应 Vercel 优先");
        assert_eq!(ordered[1], "wss://gate.example.com/ws");

        let ordered = order_endpoints(urls.clone(), "google.com.hk");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "Google 国别域应 Vercel 优先");

        let ordered = order_endpoints(urls.clone(), "antigravity.google");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "agy 域名应 Vercel 优先");

        let ordered = order_endpoints(urls.clone(), "api.openai.com");
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws", "OpenAI 应 Vercel 优先");
        assert_eq!(ordered[1], "wss://gate.example.com/ws");

        let ordered = order_endpoints(urls.clone(), "github.com");
        assert_eq!(ordered[0], "wss://gate.example.com/ws", "常规非 Google/AI 应 CF 优先");
        assert_eq!(ordered[1], "wss://vgate.example.com/api/ws");
        std::env::remove_var("PPROXY_CONSERVE_VERCEL");
    }

    #[test]
    fn endpoint_order_stable_for_custom_and_unknown() {
        let urls = vec![
            "wss://custom-a.example/ws",
            "wss://custom-b.example/ws",
        ];
        let ordered = order_endpoints(urls, "random.host.io");
        assert_eq!(ordered[0], "wss://custom-a.example/ws");
        assert_eq!(ordered[1], "wss://custom-b.example/ws");
    }

    async fn spawn_counting_gate(
        first_ok: bool,
        conns: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::io::Result<std::net::SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                conns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    if let Ok(mut ws) = accept_async(stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            match msg {
                                Message::Text(_) => {
                                    let resp = if first_ok {
                                        serde_json::json!({ "ok": true }).to_string()
                                    } else {
                                        serde_json::json!({ "ok": false, "reason": "mock_denied" }).to_string()
                                    };
                                    let _ = ws.send(Message::Text(resp)).await;
                                }
                                Message::Binary(b) => {
                                    let _ = ws.send(Message::Binary(b)).await;
                                }
                                Message::Ping(p) => {
                                    let _ = ws.send(Message::Pong(p)).await;
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
        Ok(addr)
    }

    fn test_pool(rx: watch::Receiver<(Option<String>, Option<String>)>, size: usize) -> Arc<TunnelPool> {
        TunnelPool::with_timing(
            rx,
            size,
            // 30s TTL：指纹 miss 用例里失配会话必须留存（短 TTL 会让过期与 miss 混淆成 flake）
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(80),
            std::time::Duration::from_millis(150),
        )
    }

    #[tokio::test]
    async fn tunnel_pool_preconnects_without_traffic() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未在 5s 内预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tunnel_pool_checkout_follows_endpoint_order() {
        let a_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let b_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let a_url = format!("ws://{a_addr}/ws1");
        let b_url = format!("ws://{b_addr}/ws2");
        let urls = format!("{a_url},{b_url}");
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let ordered_b = order_endpoints(vec![&a_url, &b_url], "github.com");
        let item = pool.checkout(&ordered_b, None);
        assert!(item.is_some());
        assert_eq!(item.unwrap().2, a_url);
    }

    #[tokio::test]
    async fn tunnel_pool_checkout_rejects_stale_fingerprint() {
        let a_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let a_url = format!("ws://{a_addr}/ws");
        let urls = a_url.clone();
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // 旧指纹会话在期望指纹变化后 checkout 为 miss，且会话不被消耗
        let miss = pool.checkout(&[&a_url], Some("deadbeef"));
        assert!(miss.is_none(), "指纹失配必须 miss");
        assert_eq!(pool.idle_total(), 1, "失配会话不得被消耗");
        // invalidate 后排空
        pool.invalidate();
        assert_eq!(pool.idle_total(), 0, "invalidate 必须排空");
        let stats = pool.pool_stats();
        assert_eq!(stats.idle, 0);
        assert!(stats.misses >= 1, "miss 计数必须递增");
    }

    #[tokio::test]
    async fn establish_reuses_pooled_hot_session() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未在 5s 内预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        let cfg = cfg_with_pool(urls, Arc::clone(&pool));
        let parsed = ReqHead { kind: Kind::Connect, host: "www.google.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "establish 应命中池并成功: {result:?}");
        assert!(conns.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn tunnel_pool_clears_and_rebuilds_on_watch_change() {
        let conns_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr_a = spawn_counting_gate(true, Arc::clone(&conns_a)).await.unwrap();
        let (tx, rx) = watch::channel((Some(format!("ws://{addr_a}")), Some("mock-token-a".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let conns_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr_b = spawn_counting_gate(true, Arc::clone(&conns_b)).await.unwrap();
        tx.send((Some(format!("ws://{addr_b}")), Some("mock-token-b".into()))).unwrap();

        let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while conns_b.load(std::sync::atomic::Ordering::SeqCst) < 1 {
            assert!(tokio::time::Instant::now() < deadline2, "watch 变化后未向新端点建连");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn establish_falls_back_when_pooled_session_bind_fails() {
        let a_addr = spawn_counting_gate(false, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let b_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let urls = format!("ws://{a_addr},ws://{b_addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let cfg = cfg_with_pool(urls, pool);
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "池会话 bind 失败应回落并命中次端点: {result:?}");
    }

    #[tokio::test]
    async fn tunnel_pool_refills_expired_sessions_automatically() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        // 本用例专测过期补建，用短 TTL 池（公共 test_pool 为 30s，防指纹 miss 用例 flake）
        let pool = TunnelPool::with_timing(
            rx,
            1,
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(80),
            std::time::Duration::from_millis(150),
        );
        pool.start_maintain();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while conns.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            assert!(tokio::time::Instant::now() < deadline2);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn tunnel_pool_maintain_exits_when_owner_dropped() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = test_pool(rx, 1);
        let handle = pool.start_maintain().unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(pool);

        let res = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
        assert!(res.is_ok(), "maintain 任务未在 pool drop 后及时退出");
    }
}
