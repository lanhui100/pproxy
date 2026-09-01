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
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use super::engine::{EngineConfig, EngineStats, Kind, ReqHead};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("tunnel: {e}"))
}

/// WS Upgrade 被 gate 以 401 拒绝时的稳定错误标记（自愈判定的唯一依据）。
const AUTH_401_MARKER: &str = "tunnel_auth_401";

/// 401 自愈冷却：进程内距上次自愈至少间隔此时长（single-flight 负缓存，
/// 防止 token 轮换瞬间 N 个并发 CONNECT 各自同步读 keyring + 重复重试）。
#[cfg(not(test))]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const SELF_HEAL_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(0);
static LAST_SELF_HEAL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

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

/// WS 会话分裂后的写端/读端（池化预建与 bind_target 共用）。
type WsTx = futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, Message>;
type WsRx = futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>;

/// 单端点 WS Upgrade（仅建连，不声明目标）：供池化预建复用。
/// 401 打稳定标记（establish 的自愈只认此标记）。
async fn connect_ws(url_str: &str, token: &str) -> Result<(WsTx, WsRx), std::io::Error> {
    let mut req = url_str
        .into_client_request()
        .map_err(|e| io(format!("bad tunnel url: {e}")))?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(io)?,
    );
    // 拨号 + WS Upgrade 共享 DIAL_TIMEOUT 预算：静默端点不得挂死建连循环
    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let upgrade = tokio::time::timeout_at(dial_deadline, tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| io(format!("dial timeout after {DIAL_TIMEOUT:?}")))?;
    let (ws, _resp) = match upgrade {
        Ok(pair) => pair,
        Err(e) => {
            if let tokio_tungstenite::tungstenite::Error::Http(resp) = &e {
                if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED {
                    return Err(io(AUTH_401_MARKER));
                }
            }
            return Err(io(e));
        }
    };
    Ok(ws.split())
}

/// 在已建立（或池化取出）的 WS 会话上声明目标：Text 首帧 {host,port} → 等 {"ok":true}。
/// 池化预建会话与全新建连共用此绑定步骤。
async fn bind_target(
    mut ws_tx: WsTx,
    mut ws_rx: WsRx,
    parsed: &ReqHead,
) -> Result<(WsTx, WsRx), std::io::Error> {
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

/// 单端点全流程建连（WS Upgrade + 首帧 + 等 {"ok":true}）——冷建连路径。
async fn try_establish_url(
    url_str: &str,
    token: &str,
    parsed: &ReqHead,
) -> Result<WsPair, std::io::Error> {
    let (tx, rx) = connect_ws(url_str, token).await?;
    bind_target(tx, rx, parsed).await
}

/// 建连阶段：支持多中继端点自动切换（CF 节点不可达/被拒时无缝回退备用 Vercel/Node 节点）。
/// 返回成功使用的端点 URL，供流量统计按出口（CF/Vercel）归账。
///
/// 性能专项（方案 A）：优先从待命池 checkout（已预建 WS Upgrade，热态），
/// 命中则只做首帧声明（1 RTT）即可写 200；池 miss / 池会话死亡（bind 失败）
/// 回落到冷建连全流程。池命中失败绝不静默回落到错误端点——按 host 优先级取端点。
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
    // P2-3：按目标 host 重排端点优先级（Google → Vercel 优先；非 Google → CF 优先），
    // 与 gate-policy 门禁口径一致，避免非 Google 流量被默认 Vercel 优先拖慢、Google 流量先吃 CF denied。
    let urls = order_endpoints(urls, &parsed.host);

    // 方案 A：先取池会话（按 host 重排后的端点优先级），bind 热态首帧。
    // 池会话死亡（静默关闭）→ bind 失败 → 落到冷建连重试路径，不比无池化更差。
    if let Some((tx, rx, used_url)) = cfg.pool.checkout(&urls) {
        match bind_target(tx, rx, parsed).await {
            Ok(pair) => {
                log::debug!("tunnel established from pool via {used_url} for {}", parsed.host);
                return Ok((pair, used_url));
            }
            Err(e) => {
                log::warn!("pooled session bind failed for {} on {used_url}: {e}", parsed.host);
            }
        }
    }

    let mut last_err = None;
    let mut saw_401 = false;
    for u in &urls {
        match try_establish_url(u, &token, parsed).await {
            Ok(pair) => return Ok((pair, (*u).to_string())),
            Err(e) => {
                if e.to_string().contains(AUTH_401_MARKER) { saw_401 = true; }
                log::warn!("tunnel establish on {u} for {} failed: {e}", parsed.host);
                last_err = Some(e);
            }
        }
    }

    // 401 自愈：全部端点鉴权失败时，凭据可能已被外部更新（轮换）——重读凭据并刷新重试。
    // 自愈不变式（spec token-ux-simplification §9 P0-2）：重读为 None（含冷却中）绝不回写 watch；
    // 仅当与 watch 快照不同才回写（防并发回退用户新保存值）；进程内冷却 single-flight + spawn_blocking 读 keyring。
    if saw_401 {
        let cooled = {
            let mut g = LAST_SELF_HEAL.lock().unwrap_or_else(|p| p.into_inner());
            match *g {
                Some(t) if t.elapsed() < SELF_HEAL_COOLDOWN => false,
                _ => { *g = Some(std::time::Instant::now()); true }
            }
        };
        let (fresh_url, fresh_token) = if cooled {
            tokio::task::spawn_blocking(crate::tunnel_config_load).await.unwrap_or((None, None))
        } else {
            (None, None) // 冷却中：跳过自愈（single-flight）
        };
        if let (Some(u), Some(t)) = (fresh_url, fresh_token) {
            let current = cfg.tunnel.borrow().clone();
            if Some(t.clone()) != current.1 && t != token {
                log::info!("tunnel token rotated on disk, refreshing watch and retrying once");
                let _ = crate::ensure_tunnel_watch().send((Some(u.clone()), Some(t.clone())));
                let urls2: Vec<&str> = u.split([',', ';', '\n']).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                // P2-3：自愈重试同样按 host 重排端点优先级
                let urls2 = order_endpoints(urls2, &parsed.host);
                for u2 in &urls2 {
                    match try_establish_url(u2, &t, parsed).await {
                        Ok(pair) => return Ok((pair, (*u2).to_string())),
                        Err(e) => {
                            log::warn!("tunnel re-establish on {u2} for {} failed: {e}", parsed.host);
                            last_err = Some(e);
                        }
                    }
                }
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

/// Google 系 host 判定（P2-3 修复）：与 gate-policy.mjs 的 GOOGLE_SUFFIXES 保持一致，
/// 含 antigravity.google / labs.google（桌面端白名单默认项），避免"前端绿但 agy 红"的错位。
const GOOGLE_SUFFIXES: &[&str] = &[
    "google.com",
    "googleapis.com",
    "gstatic.com",
    "googleusercontent.com",
    "deepmind.google",
    "antigravity.google",
    "labs.google",
    "g.co",
    "goog",
];

pub fn is_google_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    GOOGLE_SUFFIXES.iter().any(|s| {
        if !h.ends_with(s) {
            return false;
        }
        let rest = &h[..h.len() - s.len()];
        rest.is_empty() || rest.ends_with('.')
    })
}

/// 端点优先级重排（P2-3 修复）：Google 系 host → Vercel 合规出口优先（避免先吃 CF colo denied 的
/// 往返，且保证 Google API 区域合规）；非 Google host → CF 低延迟优先（Vercel 兜底）。
/// 稳定排序：同优先级端点保持 tunnel.json 原始顺序；自定义端点按归类参与排序。
fn order_endpoints<'a>(urls: Vec<&'a str>, host: &str) -> Vec<&'a str> {
    let google = is_google_host(host);
    let mut list = urls;
    list.sort_by_key(|u| {
        let is_vercel = u.contains("vercel") || u.contains("vgate");
        match (google, is_vercel) {
            (true, true) => 0,   // Google + Vercel 最优先
            (true, false) => 1,  // Google + CF/自定义 兜底
            (false, true) => 1,  // 非 Google + Vercel 兜底
            (false, false) => 0, // 非 Google + CF/自定义 最优先
        }
    });
    list
}

/// 待命 WS 会话：已完成 TCP+TLS+WS Upgrade（跨洲 ~4 RTT），尚未发送首帧。
/// 首帧 `{host,port}` 在 checkout 时才声明目标，协议天然支持预建。
struct IdleSession {
    tx: WsTx,
    rx: WsRx,
    born: tokio::time::Instant,
}

/// 待命隧道池（性能专项，方案 A）：CONNECT 到达前按端点预建 WS 会话，
/// establish 从 ~5 RTT（TCP+TLS+Upgrade+首帧）压到 1 RTT（首帧声明）。
///
/// 与 server 端（crates/server connect.rs TunnelPool）对齐，桌面端差异：
/// - **按端点分组**（HashMap<endpoint, Vec<IdleSession>>）：桌面端按 host 重排端点
///   （P2-3），checkout 必须按 host 优先级取对应端点，否则 Google 流量可能命中
///   CF 池、非 Google 命中 Vercel 池，违背端点策略；
/// - **watch 热更新清池**：tunnel 配置（url/token）变化时清空重建，防旧端点/token
///   的待命会话被 checkout；
/// - **Weak 自引用**：maintain 任务不持有强引用，EngineConfig 被 drop（引擎关闭）
///   后自动退出，不泄漏后台任务。
pub struct TunnelPool {
    idle: std::sync::Mutex<std::collections::HashMap<String, Vec<IdleSession>>>,
    notify: tokio::sync::Notify,
    /// 每个端点的待命会话数。
    size: usize,
    /// tunnel 配置 watch（url/token），maintain 据此预建并监听变化清池。
    /// 用 tokio::sync::Mutex 包裹：changed() 需要 &mut，且为异步调用。
    tunnel_watch: tokio::sync::Mutex<watch::Receiver<(Option<String>, Option<String>)>>,
    /// maintain 幂等启动守卫（start_maintain 只在首次 spawn）。
    started: std::sync::atomic::AtomicBool,
}

/// 每个端点的待命会话数（默认双端点 ≈2 条 TCP+TLS 连接，内存/句柄可控）。
const POOL_SIZE_PER_ENDPOINT: usize = 1;
/// 待命会话最大存活：CF WS 空闲回收前主动轮换（30s，避免 checkout 到死会话）。
#[cfg(not(test))]
const IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
/// 补给巡检间隔。
#[cfg(not(test))]
const REFILL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(test)]
const REFILL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// 建连失败退避（避免 gate 不可达时热循环）。
#[cfg(not(test))]
const REFILL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(test)]
const REFILL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(150);

impl std::fmt::Debug for TunnelPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelPool")
            .field("size_per_endpoint", &self.size)
            .finish()
    }
}

impl TunnelPool {
    /// 生产构造：立即在后台预建并维持待命会话。返回 Arc，EngineConfig 持有。
    pub fn new(tunnel_watch: watch::Receiver<(Option<String>, Option<String>)>) -> Arc<Self> {
        Self::with_size(tunnel_watch, POOL_SIZE_PER_ENDPOINT)
    }

    /// 指定每端点池大小；size=0 禁池化（checkout 恒 None，maintain 空转）。
    /// 不在此 spawn maintain：`with_size` 可能在非 Tokio 上下文被调用（同步测试 / 托盘
    /// 菜单处理器），tokio::spawn 会 panic "no reactor running"。由 `start_maintain`
    /// 在异步上下文（engine::run / async 测试）显式启动。
    pub fn with_size(
        tunnel_watch: watch::Receiver<(Option<String>, Option<String>)>,
        size: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            idle: std::sync::Mutex::new(std::collections::HashMap::new()),
            notify: tokio::sync::Notify::new(),
            size,
            tunnel_watch: tokio::sync::Mutex::new(tunnel_watch),
            started: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// 启动后台补给任务（幂等）。必须从 Tokio 运行时上下文调用：
    /// engine::run 启动时、或 #[tokio::test] 内。size=0 时无操作。
    pub fn start_maintain(self: &Arc<Self>) {
        if self.size == 0 {
            return;
        }
        use std::sync::atomic::Ordering as AOrd;
        if self.started.swap(true, AOrd::SeqCst) {
            return; // 已启动
        }
        // Weak 自引用：EngineConfig drop 后唯一强引用消失，maintain 退出
        let weak = Arc::downgrade(self);
        tokio::spawn(Self::maintain(weak));
    }

    /// 按 host 端点优先级取一条未过期待命会话；过期会话 drop。返回命中端点的 URL。
    fn checkout(&self, ordered_urls: &[&str]) -> Option<(WsTx, WsRx, String)> {
        let mut guard = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        for u in ordered_urls {
            if let Some(vec) = guard.get_mut(*u) {
                while let Some(s) = vec.pop() {
                    if s.born.elapsed() < IDLE_TTL {
                        drop(guard);
                        self.notify.notify_one(); // 唤醒补给立即回填
                        return Some((s.tx, s.rx, (*u).to_string()));
                    }
                }
            }
        }
        None
    }

    /// 当前池内会话总数（测试断言用）。
    #[cfg(test)]
    fn idle_total(&self) -> usize {
        self.idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|v| v.len())
            .sum()
    }

    /// 后台补给：读 watch → 端点变化清池 → 逐端点补足 → 等待（定时/唤醒/watch 变化）。
    async fn maintain(weak: std::sync::Weak<Self>) {
        let Some(pool) = weak.upgrade() else { return };
        let mut last_key: Option<Vec<String>> = None;
        loop {
            // 若持有者（EngineConfig）已 drop，退出后台任务
            if weak.upgrade().is_none() {
                return;
            }
            let (url_raw, token) = {
                let rx = pool.tunnel_watch.lock().await;
                let (u, t) = rx.borrow().clone();
                (u, t)
            };
            match (url_raw, token) {
                (Some(url_raw), Some(token)) => {
                    let endpoints: Vec<String> = url_raw
                        .split([',', ';', '\n'])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    // 端点集合或顺序变化 → 清池重建（旧端点的待命会话失效）
                    if last_key.as_ref() != Some(&endpoints) {
                        pool.idle.lock().unwrap_or_else(|p| p.into_inner()).clear();
                        last_key = Some(endpoints.clone());
                    }
                    // 逐端点补足到 size
                    let mut healthy = true;
                    for ep in &endpoints {
                        let need = {
                            let g = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                            pool.size.saturating_sub(g.get(ep).map(|v| v.len()).unwrap_or(0))
                        };
                        for _ in 0..need {
                            match connect_ws(ep, &token).await {
                                Ok((tx, rx)) => {
                                    pool.idle
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .entry(ep.clone())
                                        .or_default()
                                        .push(IdleSession {
                                            tx,
                                            rx,
                                            born: tokio::time::Instant::now(),
                                        });
                                }
                                Err(e) => {
                                    log::warn!("tunnel pool refill {ep} failed: {e}");
                                    healthy = false;
                                    break;
                                }
                            }
                        }
                    }
                    let wait = if healthy { REFILL_INTERVAL } else { REFILL_BACKOFF };
                    // watch 配置变化立即醒来清池重建（send 后 changed() 返回）。
                    // async 块把锁与 changed() 封装在一起：select 取消分支时 guard 随 future 释放。
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = pool.notify.notified() => {}
                        _ = async {
                            let mut rx = pool.tunnel_watch.lock().await;
                            let _ = rx.changed().await;
                        } => {}
                    }
                }
                // 未配置隧道：清池并等待配置出现
                _ => {
                    pool.idle.lock().unwrap_or_else(|p| p.into_inner()).clear();
                    last_key = None;
                    // sender 全 drop（watch 关闭）→ changed() 立即 Err，退避睡眠避免空转
                    let closed = {
                        let mut rx = pool.tunnel_watch.lock().await;
                        rx.changed().await.is_err()
                    };
                    if closed {
                        tokio::time::sleep(REFILL_BACKOFF).await;
                    }
                }
            }
        }
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

    // ---- P2-3：Google 系 host 判定与端点优先级 ----

    #[test]
    fn google_host_detection_matches_gate_policy() {
        // 与 gate-policy.mjs GOOGLE_SUFFIXES 保持一致，含 agy 域名
        for h in [
            "google.com",
            "www.google.com",
            "accounts.google.com",
            "generativelanguage.googleapis.com",
            "oauth2.googleapis.com",
            "www.gstatic.com",
            "deepmind.google",
            "antigravity.google",
            "api.antigravity.google",
            "labs.google",
            "g.co",
        ] {
            assert!(is_google_host(h), "应识别为 Google 系: {h}");
        }
        // 非 Google / 陷阱域
        for h in ["github.com", "youtube.com", "notgoogleapis.com", "googleapis.com.evil.cn", "baidu.com"] {
            assert!(!is_google_host(h), "不应识别为 Google 系: {h}");
        }
    }

    #[test]
    fn endpoint_order_prefers_vercel_for_google() {
        let urls = vec![
            "wss://gate.ponyjob.top/ws",
            "wss://vgate.ponyjob.top/api/ws",
        ];
        let ordered = order_endpoints(urls.clone(), "oauth2.googleapis.com");
        assert_eq!(ordered[0], "wss://vgate.ponyjob.top/api/ws", "Google 应 Vercel 优先");
        assert_eq!(ordered[1], "wss://gate.ponyjob.top/ws");

        let ordered = order_endpoints(urls.clone(), "antigravity.google");
        assert_eq!(ordered[0], "wss://vgate.ponyjob.top/api/ws", "agy 域名应 Vercel 优先");

        let ordered = order_endpoints(urls.clone(), "api.openai.com");
        assert_eq!(ordered[0], "wss://gate.ponyjob.top/ws", "非 Google 应 CF 优先");
        assert_eq!(ordered[1], "wss://vgate.ponyjob.top/api/ws");
    }

    #[test]
    fn endpoint_order_stable_for_custom_and_unknown() {
        // 同优先级保持原始相对顺序；未知域名按非 Google 处理（CF/自定义优先）
        let urls = vec![
            "wss://custom-a.example/ws",
            "wss://custom-b.example/ws",
        ];
        let ordered = order_endpoints(urls, "random.host.io");
        assert_eq!(ordered[0], "wss://custom-a.example/ws");
        assert_eq!(ordered[1], "wss://custom-b.example/ws");
    }

    // ---- 方案 A：待命隧道池 ----

    /// 计数型 fake gate：接受连接数可断言（验证 establish 复用了池会话而非冷建连）。
    /// 每连接 spawn 独立任务，支持并发：池预建会话等待首帧时仍可接受冷建连。
    async fn spawn_counting_gate(
        first_ok: bool,
        conns: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::io::Result<std::net::SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let conns2 = Arc::clone(&conns);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                conns2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(msg)) = ws.next().await {
                        if let Message::Text(_) = msg {
                            let reply = if first_ok {
                                serde_json::json!({ "ok": true }).to_string()
                            } else {
                                serde_json::json!({ "ok": false, "reason": "denied" }).to_string()
                            };
                            let _ = ws.send(Message::Text(reply)).await;
                            if first_ok {
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
            }
        });
        Ok(addr)
    }

    fn cfg_with_pool(urls: String, pool: Arc<TunnelPool>) -> EngineConfig {
        let (_ttx, trx) = watch::channel((Some(urls), Some("mock-token".into())));
        EngineConfig { tunnel: trx, pool, ..Default::default() }
    }

    /// 池应后台预建待命会话（无需任何 CONNECT 流量）。
    #[tokio::test]
    async fn tunnel_pool_preconnects_without_traffic() {
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_counting_gate(true, Arc::clone(&conns)).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls), Some("mock-token".into())));
        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未在 5s 内预建会话");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(conns.load(std::sync::atomic::Ordering::SeqCst) >= 1, "gate 应收到预建连接");
    }

    /// checkout 按传入（host 已重排）的端点顺序取池；同端点同序稳定。
    #[tokio::test]
    async fn tunnel_pool_checkout_follows_endpoint_order() {
        let (a_addr, _) = spawn_fake_gate(true).await.unwrap();
        let (b_addr, _) = spawn_fake_gate(true).await.unwrap();
        let urls = format!("ws://{a_addr},ws://{b_addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点会话: {}", pool.idle_total());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // 传入顺序 [b, a] → 应命中 b 端点
        let url_a = format!("ws://{a_addr}");
        let url_b = format!("ws://{b_addr}");
        let got = pool.checkout(&[url_b.as_str(), url_a.as_str()]);
        assert!(got.is_some(), "应按序命中端点");
        let (_, _, used) = got.unwrap();
        assert_eq!(used, url_b, "checkout 应优先返回传入顺序靠前的端点");
    }

    /// watch 配置变化 → 清池重建（旧端点待命会话失效）。
    #[tokio::test]
    async fn tunnel_pool_clears_and_rebuilds_on_watch_change() {
        let (a_addr, _) = spawn_fake_gate(true).await.unwrap();
        let (b_addr, _) = spawn_fake_gate(true).await.unwrap();
        let (tx, rx) = watch::channel((Some(format!("ws://{a_addr}")), Some("mock-token".into())));
        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // 切换端点：a → b
        tx.send((Some(format!("ws://{b_addr}")), Some("mock-token".into()))).unwrap();
        let deadline2 = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let got = pool.checkout(&[format!("ws://{b_addr}").as_str()]);
            if got.is_some() {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline2, "watch 变化后未重建为新端点会话");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// establish 命中池会话（热态）：单连接 fake gate 下冷建连必然超时失败，
    /// 成功即证明复用池会话（而非新建连接）。
    #[tokio::test]
    async fn establish_reuses_pooled_hot_session() {
        let (addr, _handle) = spawn_fake_gate(true).await.unwrap();
        let urls = format!("ws://{addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 1 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let cfg = cfg_with_pool(urls, pool);
        let parsed = ReqHead { kind: Kind::Connect, host: "oauth2.googleapis.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        // 单连接 gate：冷建连会因无第二次 accept 而 DIAL_TIMEOUT 失败；成功=复用池会话
        assert!(result.is_ok(), "应经池会话建立（冷建连会超时）: {result:?}");
    }

    /// 池会话 bind 失败（死会话/denied）→ 回落到冷建连路径，不比其他更差。
    #[tokio::test]
    async fn establish_falls_back_when_pooled_session_bind_fails() {
        // 端点 A：denied（池预建其会话，bind 时被拒）；端点 B：ok
        let a_addr = spawn_counting_gate(false, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let b_addr = spawn_counting_gate(true, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await.unwrap();
        let urls = format!("ws://{a_addr},ws://{b_addr}");
        let (_tx, rx) = watch::channel((Some(urls.clone()), Some("mock-token".into())));
        let pool = TunnelPool::with_size(rx, 1);
        pool.start_maintain();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool.idle_total() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "池未预建双端点: {}", pool.idle_total());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let cfg = cfg_with_pool(urls, pool);
        // 非 Google host：按原始顺序 [A, B]，checkout 先取 A（denied）→ 回落冷建连命中 B
        let parsed = ReqHead { kind: Kind::Connect, host: "www.youtube.com".into(), port: 443 };
        let result = establish(&cfg, &parsed).await;
        assert!(result.is_ok(), "池会话 bind 失败应回落并命中次端点: {result:?}");
    }
}