//! 引擎核心：127.0.0.1:18900 监听，CONNECT/absolute-form 分流。
//!
//! 决策：host 命中白名单 → [`tunnel`]（WS 隧道，M6 后续任务实现）；未命中 →
//! 本机直接 dial。失败语义：隧道不可用即关闭连接并计数，绝不静默回落直连
//! （spec §5，R4）。
//! T1: watch 通道热更新，handle_conn 与 /pac 每请求 clone 后立即 drop 再 matches/generate_pac，不持锁跨 await。
#![deny(clippy::await_holding_lock)]

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::pac;
use super::whitelist;

/// 远端上游代理（方案 B chained 模式）：白名单流量经该 HTTP 代理 CONNECT 出网。
///
/// 不派生 `Debug`：密码会以明文出现在任何 `{:?}` 里（含 panic 回溯与日志）。
#[derive(Clone, PartialEq)]
pub struct Upstream {
    /// host:port（不含 scheme）
    pub host: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("host", &self.host)
            .field("username", &self.username)
            // host/username 保留以便排障，password 一律脱敏
            .field("password", &"***")
            .finish()
    }
}

/// 会话有界生命周期（`None` = 无界）。
///
/// 回环引擎（127.0.0.1:18900）保持 `Default`（无界），行为零变更；
/// LAN 监听注入 idle 300s / absolute 86400s（对齐 gate 数据面口径）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayLimits {
    /// 双向连续无字节窗口（有字节即重置）
    pub idle: Option<std::time::Duration>,
    /// 自 relay 启动起的总时长硬界（不随活动重置）
    pub absolute: Option<std::time::Duration>,
}

/// 引擎配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub listen_addr: String,
    pub whitelist: watch::Receiver<Vec<String>>,
    pub mode: watch::Receiver<pac::ProxyMode>,
    /// 隧道凭据热更新通道 (url, token)
    pub tunnel: watch::Receiver<(Option<String>, Option<String>)>,
    /// 远端上游代理热更新通道（chained 模式；Some 时优先于 WS gate）
    pub upstream: watch::Receiver<Option<Upstream>>,
    /// 待命隧道池（性能专项）：预建 WS 会话，establish 冷建连压到热态首帧。
    pub pool: std::sync::Arc<super::engine_tunnel::TunnelPool>,
    /// 会话有界生命周期（LAN 监听注入；回环监听保持无界）
    pub limits: RelayLimits,
}

impl Default for EngineConfig {
    fn default() -> Self {
        // 创建空 watch channel 供测试使用
        let (_tx, rx) = watch::channel(Vec::new());
        let (_mtx, mrx) = watch::channel(pac::ProxyMode::Whitelist);
        let (_ttx, trx) = watch::channel((None, None));
        let (_utx, urx) = watch::channel(None);
        let (_ptx, prx) = watch::channel((None, None));
        EngineConfig {
            listen_addr: "127.0.0.1:18900".into(),
            whitelist: rx,
            mode: mrx,
            tunnel: trx,
            upstream: urx,
            pool: super::engine_tunnel::TunnelPool::with_size(prx, 0),
            limits: RelayLimits::default(),
        }
    }
}

/// 运行统计（Arc 共享，供 Tauri 命令查询）。
#[derive(Debug, Default)]
pub struct EngineStats {
    pub conns: std::sync::atomic::AtomicU64,
    pub tunneled: AtomicU64,
    pub direct: AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    /// 隧道出网按出口归账的字节/连接计数：CF gate 与 Vercel gate 分别统计，
    /// 供「Cloudflare / Vercel 用量」展示；直连与 chained 上游不消耗两家额度，不计入。
    pub cf_up: AtomicU64,
    pub cf_down: AtomicU64,
    pub cf_reqs: AtomicU64,
    pub vercel_up: AtomicU64,
    pub vercel_down: AtomicU64,
    pub vercel_reqs: AtomicU64,
    pub rn_up: AtomicU64,
    pub rn_down: AtomicU64,
    pub rn_reqs: AtomicU64,
    pub upstream_up: AtomicU64,
    pub upstream_down: AtomicU64,
    pub upstream_reqs: AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
}

impl pproxy_transport::TrafficCounter for EngineStats {
    fn record_up(&self, egress: pproxy_transport::Egress, bytes: u64) {
        match egress {
            pproxy_transport::Egress::Cf => &self.cf_up,
            pproxy_transport::Egress::Vercel => &self.vercel_up,
            pproxy_transport::Egress::NativeVps => &self.rn_up,
        }
        .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_down(&self, egress: pproxy_transport::Egress, bytes: u64) {
        match egress {
            pproxy_transport::Egress::Cf => &self.cf_down,
            pproxy_transport::Egress::Vercel => &self.vercel_down,
            pproxy_transport::Egress::NativeVps => &self.rn_down,
        }
        .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    }
}

pub type SharedStats = Arc<EngineStats>;

/// 启动引擎循环（由 tauri setup / 命令调用；返回前先绑定端口）。
pub async fn run(cfg: EngineConfig, stats: SharedStats) -> std::io::Result<()> {
    // 方案 A：启动待命隧道池补给（异步上下文，幂等）
    cfg.pool.start_maintain();
    let listener = TcpListener::bind(&cfg.listen_addr).await?;
    let cfg = Arc::new(cfg);
    loop {
        let (stream, _) = listener.accept().await?;
        stats
            .conns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cfg_clone = cfg.clone();
        let stats_clone = stats.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, &cfg_clone, &stats_clone).await {
                stats_clone
                    .errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut g) = stats_clone.last_error.lock() {
                    *g = Some(e.to_string());
                }
                log::warn!("proxy conn failed: {e}");
            }
        });
    }
}

#[derive(Debug, PartialEq)]
enum Route {
    Tunnel,
    Direct,
}

/// 分流判定（纯函数）：根据工作模式（白名单/全局）决定路由。
/// 严禁在隧道未配置时静默回落 Direct（R4 契约）；本地地址与 Bypass 管理面强制 Direct。
fn decide(host: &str, whitelist_snapshot: &[String], mode: pac::ProxyMode) -> Route {
    let norm = whitelist::normalize_host(host);
    // 防回环：本地与 Bypass 管理面强制 Direct
    let bypass_set = pac::collect_bypass_hosts();
    if norm == "localhost"
        || norm == "127.0.0.1"
        || norm == "::1"
        || norm.ends_with(".ts.net")
        || norm.ends_with(".local")
        || norm.ends_with(".internal")
        || bypass_set.contains(&norm)
    {
        return Route::Direct;
    }
    match mode {
        pac::ProxyMode::Global => Route::Tunnel,
        pac::ProxyMode::Whitelist => {
            if whitelist::matches(&norm, whitelist_snapshot) {
                Route::Tunnel
            } else {
                Route::Direct
            }
        }
    }
}

pub struct ReqHead {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, PartialEq)]
pub enum Kind {
    Connect,
    Plain,
}

/// 解析请求头第一行 + Host 头（纯函数便于测试）。
fn parse_head(head: &str) -> Option<ReqHead> {
    let first = head.lines().next()?;
    if let Some(rest) = first.strip_prefix("CONNECT ") {
        let target = rest.split(' ').next()?;
        let (h, p) = split_host_port(target, 443)?;
        return Some(ReqHead { kind: Kind::Connect, host: h, port: p });
    }
    // absolute-form：GET http://host/path HTTP/1.1
    let method_target: Vec<&str> = first.splitn(3, ' ').collect();
    if method_target.len() == 3 {
        if let Some(url) = method_target[1].strip_prefix("http://") {
            let authority = url.split(['/', '?']).next()?;
            let (h, p) = split_host_port(authority, 80)?;
            return Some(ReqHead { kind: Kind::Plain, host: h, port: p });
        }
    }
    None
}

fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 字面量 [::1]:port 或 [::1]
        let (h, rest) = stripped.split_once(']')?;
        if rest.is_empty() {
            return Some((h.to_string(), default_port));
        }
        let port_str = rest.strip_prefix(':')?;
        let port: u16 = port_str.parse().ok()?;
        return Some((h.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((authority.to_string(), default_port)),
    }
}

const CLIENT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 一次读头的产物：**头区**（可安全按 UTF-8 解析）与**头区之后的原始字节**。
///
/// 头之后的字节可能是二进制（如 CONNECT 后紧跟的 TLS ClientHello），必须按字节切片
/// 原样转发，严禁经 `from_utf8_lossy` 转换（替换字符会污染首包）。
pub(crate) struct ReadRequest {
    pub head: String,
    pub preamble: Vec<u8>,
}

/// 读取请求头（至 `\r\n\r\n`，上限 16 KiB，含 15 秒超时防 Slowloris 悬挂）。
/// 对端在头完成前关闭 → `Ok(None)`（无数据可处理）。
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<ReadRequest>> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + CLIENT_READ_TIMEOUT;
    loop {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut tmp))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "client head read timed out"))??;
        if n == 0 {
            return Ok(None); // 对端关闭
        }
        buf.extend_from_slice(&tmp[..n]);
        // 按字节定位头结束：头区走 UTF-8 解析，余下字节原样保留（二进制安全）
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let split = pos + 4;
            let head = String::from_utf8_lossy(&buf[..split]).into_owned();
            return Ok(Some(ReadRequest {
                head,
                preamble: buf[split..].to_vec(),
            }));
        }
        if buf.len() > 16 * 1024 {
            break;
        }
    }
    // 超限仍无终止符：整段按头处理（后续 parse_head 会 400），不猜测二进制边界
    Ok(Some(ReadRequest {
        head: String::from_utf8_lossy(&buf).into_owned(),
        preamble: Vec::new(),
    }))
}

/// 引擎自身的控制面：PAC 脚本（放行 /pac 或 /pac?t=...）。
fn is_pac_request(head: &str) -> bool {
    let Some(first) = head.lines().next() else {
        return false;
    };
    let path = first.strip_prefix("GET ").and_then(|r| r.split_whitespace().next()).unwrap_or("");
    path == "/pac"
        || path.starts_with("/pac?")
        || path.starts_with("http://127.0.0.1:18900/pac")
        || path.starts_with("http://localhost:18900/pac")
}

async fn handle_conn(
    mut stream: TcpStream,
    cfg: &EngineConfig,
    stats: &EngineStats,
) -> std::io::Result<()> {
    let Some(req) = read_head(&mut stream).await? else {
        return Ok(());
    };

    if is_pac_request(&req.head) {
        let wl_snapshot = {
            let guard = cfg.whitelist.borrow();
            guard.clone()
        };
        let mode_snapshot = *cfg.mode.borrow();
        let body = pac::generate_pac(&wl_snapshot, mode_snapshot);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        return stream.write_all(resp.as_bytes()).await;
    }

    dispatch(stream, cfg, stats, req).await
}

/// 解析已完成读头的请求并分派（回环引擎与 LAN 监听共用同一套 decide/建连逻辑）。
async fn dispatch(
    mut stream: TcpStream,
    cfg: &EngineConfig,
    stats: &EngineStats,
    req: ReadRequest,
) -> std::io::Result<()> {
    let Some(parsed) = parse_head(&req.head) else {
        return stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await;
    };

    // T1: decide 前先 clone whitelist & mode snapshot 并 drop
    let wl_snapshot = {
        let g = cfg.whitelist.borrow();
        g.clone()
    };
    let mode_snapshot = *cfg.mode.borrow();
    let route = decide(&parsed.host, &wl_snapshot, mode_snapshot);
    match route {
        Route::Direct => {
            stats.direct.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            direct_relay(stream, parsed, &req.head, &req.preamble, cfg.limits).await
        }
        Route::Tunnel => {
            stats.tunneled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let upstream = cfg.upstream.borrow().clone();
            let result = match upstream {
                Some(u) => super::engine_upstream::connect_and_relay(
                    stream,
                    parsed,
                    &req.head,
                    &req.preamble,
                    &u,
                    stats,
                    cfg.limits,
                )
                .await,
                None => super::engine_tunnel::connect_and_relay(
                    stream,
                    parsed,
                    &req.head,
                    &req.preamble,
                    cfg,
                    stats,
                )
                .await,
            };
            match result {
                Ok(()) => Ok(()),
                Err(e) => {
                    stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    *stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(e.to_string());
                    Err(e)
                }
            }
        }
    }
}

/// 直连路径：本机 dial 目标后双向拷贝。CONNECT 需先回 200 Established；
/// absolute-form 则把请求头重写为 origin-form 转发。
///
/// 头区之后已读到的字节（`preamble`）按原始字节转发，保证 CONNECT 后的二进制首包
/// （如 TLS ClientHello）不被 UTF-8 转换污染。
async fn direct_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    preamble: &[u8],
    limits: RelayLimits,
) -> std::io::Result<()> {
    let target = match TcpStream::connect((parsed.host.as_str(), parsed.port)).await {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("502 Bad Gateway: direct dial {} failed: {}", parsed.host, e);
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                msg.len(),
                msg
            );
            let _ = client.write_all(resp.as_bytes()).await;
            return Err(e);
        }
    };
    let mut target = target;
    if parsed.kind == Kind::Connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if !preamble.is_empty() {
            target.write_all(preamble).await?;
        }
        relay_bidir_with_stats(client, target, None, limits).await
    } else {
        let forwarded = rebuild_request(head);
        target.write_all(forwarded.as_bytes()).await?;
        if !preamble.is_empty() {
            target.write_all(preamble).await?;
        }
        relay_bidir_with_stats(client, target, None, limits).await
    }
}

/// 重写明文请求：首行 absolute-form → origin-form，其余头原样保留（含结尾空行与 body）。
///
/// 严禁用 `head.lines()` 逐行重建：`lines()` 会把结尾的 `\r\n\r\n` 拆出一个额外空元素，
/// 重建后变成 `...\r\n\r\n\r\n`，明文 POST 的 body 会被这 2 字节前缀破坏。
/// 这里用 `split_once("\r\n")` 分离首行与剩余，剩余原样拼回（同 engine_upstream::rebuild_request）。
fn rebuild_request(head: &str) -> String {
    // 退化输入（无 CRLF）时补一个空行，保证请求头完整
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, "\r\n\r\n"));
    format!("{}\r\n{}", rewrite_first_line_absolute(first), rest)
}

fn rewrite_first_line_absolute(first: &str) -> String {
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

/// 双向透传（单循环双方向，半关闭语义正确）。
///
/// 设计要点（吸取两版教训）：
/// - **禁止 `try_join!`**：一端关闭而另一端保持连接（keep-alive）时永远等不到 EOF，任务泄漏；
/// - **禁止 `select!` 取消另一方向**：被取消的 copy 已读未写的数据会静默丢弃，响应被截断；
/// - 正确做法：单一循环里同时监听两端读，单端 EOF 时对该方向对端写半区发 FIN（半关闭），
///   另一端继续转发直到也 EOF，两端都 EOF 才退出。
/// - `limits` 提供可选 idle / absolute 硬界（LAN 注入；回环监听为 `Default` 即无界）。
pub(crate) async fn relay_bidir_with_stats(
    a: TcpStream,
    b: TcpStream,
    stats: Option<(&AtomicU64, &AtomicU64)>,
    limits: RelayLimits,
) -> std::io::Result<()> {
    let (mut ar, mut aw) = a.into_split();
    let (mut br, mut bw) = b.into_split();
    let mut a_eof = false;
    let mut b_eof = false;
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];
    // 无界时给一个远端 deadline，select 分支由守卫条件禁用（与 transport 口径一致）
    let idle_window = limits.idle.unwrap_or(std::time::Duration::from_secs(86400 * 365));
    let started = tokio::time::Instant::now();
    let mut idle_deadline = started + idle_window;
    let absolute_deadline = started + limits.absolute.unwrap_or(std::time::Duration::from_secs(86400 * 365));
    loop {
        if a_eof && b_eof {
            break;
        }
        tokio::select! {
            // 空闲超时：双向连续无字节窗口到点 → 双端关闭（防关闭开关后僵尸会话续命）
            _ = tokio::time::sleep_until(idle_deadline), if limits.idle.is_some() => {
                log::info!("relay closed: idle timeout reached after {idle_window:?}");
                let _ = aw.shutdown().await;
                let _ = bw.shutdown().await;
                return Ok(());
            }
            // 绝对上限：单次 relay 总时长硬界（不随活动重置）
            _ = tokio::time::sleep_until(absolute_deadline), if limits.absolute.is_some() => {
                log::info!("relay closed: absolute lifetime reached");
                let _ = aw.shutdown().await;
                let _ = bw.shutdown().await;
                return Ok(());
            }
            n = ar.read(&mut buf_a), if !a_eof => {
                let n = n?;
                if n == 0 {
                    a_eof = true;
                    let _ = bw.shutdown().await;
                } else {
                    idle_deadline = tokio::time::Instant::now() + idle_window;
                    if let Some((up, _)) = stats {
                        up.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                    bw.write_all(&buf_a[..n]).await?;
                }
            }
            n = br.read(&mut buf_b), if !b_eof => {
                let n = n?;
                if n == 0 {
                    b_eof = true;
                    let _ = aw.shutdown().await;
                } else {
                    idle_deadline = tokio::time::Instant::now() + idle_window;
                    if let Some((_, down)) = stats {
                        down.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                    aw.write_all(&buf_b[..n]).await?;
                }
            }
        }
    }
    Ok(())
}

// ============================================================================
// LAN CONNECT 监听（C4）
//
// 契约边界（勿混同）：
// - 回环 127.0.0.1:18900 的行为**完全不变**（免鉴权，系统代理/PAC 依赖它）；
// - 本模块只负责新增的 `0.0.0.0:<port>` 监听：非回环来源强制 Basic 鉴权，
//   失败 407；回环来源免鉴权。建连后出海复用 [`dispatch`]（decide + tunnel/upstream），
//   不另写第二套隧道客户端。
// - LAN 模式扫码订阅 URL 指向本监听 `/clash.yaml`，因此这里同时提供该订阅端点
//   （否则手机扫码导入必然 404，局域网模式仍然死的）。
// ============================================================================

/// LAN CONNECT 监听缺省端口；可用 env `PPROXY_LAN_PORT` 覆盖（C4）。
pub const LAN_DEFAULT_PORT: u16 = 8899;

/// LAN 监听端口：env `PPROXY_LAN_PORT` 优先，缺省 [`LAN_DEFAULT_PORT`]；非法/0 回落缺省。
pub fn lan_port_from_env() -> u16 {
    std::env::var("PPROXY_LAN_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(LAN_DEFAULT_PORT)
}

/// 优雅停止的等待上限；超时兜底 abort，保证不泄漏监听任务。
const LAN_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// LAN 会话空闲超时（对齐 gate CONNECT 数据面 300s）：双向连续无字节即回收。
pub const LAN_SESSION_IDLE: std::time::Duration = std::time::Duration::from_secs(300);
/// LAN 会话绝对上限（对齐 gate）：单次连接总时长硬界。
pub const LAN_SESSION_ABSOLUTE: std::time::Duration = std::time::Duration::from_secs(86400);
/// accept 连续失败阈值：短暂 backoff 重试，连续失败达阈值才退出（对齐 gate 重试语义）。
const LAN_ACCEPT_MAX_CONSECUTIVE_ERRORS: u32 = 10;
const LAN_ACCEPT_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_millis(50);
const LAN_ACCEPT_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(1);

type TokenProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;
type YamlProvider = Arc<dyn Fn() -> String + Send + Sync>;

/// LAN 监听运行配置（由 lib.rs 注入；令牌/YAML 每次请求实时读取，跟手热更新）。
pub struct LanConfig {
    /// 监听地址，如 `0.0.0.0:8899`
    pub listen_addr: String,
    /// 当前隧道令牌（缺失 → 非回环一律 407，fail-closed）
    pub token_provider: TokenProvider,
    /// 当前 LAN 模式 Clash YAML（订阅端点响应体）
    pub subscription_yaml: YamlProvider,
    /// 复用回环引擎的分派依赖（watch 通道 / 待命池 / 统计）
    pub engine: EngineConfig,
    pub stats: SharedStats,
}

/// LAN 监听句柄：生命周期由持有方（lib.rs 开关命令 / 退出钩子）控制。
pub struct LanListener {
    pub port: u16,
    stop: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl LanListener {
    /// 绑定端口并启动 accept 循环；**绑定失败立即返回错误**（端口占用可回显给 UI）。
    ///
    /// LAN 会话强制注入 idle/absolute 有界生命周期（回环引擎不入此路径，保持无界）。
    pub async fn start(mut cfg: LanConfig) -> std::io::Result<Self> {
        cfg.engine.limits = RelayLimits {
            idle: Some(LAN_SESSION_IDLE),
            absolute: Some(LAN_SESSION_ABSOLUTE),
        };
        let listener = TcpListener::bind(&cfg.listen_addr).await?;
        let port = listener.local_addr()?.port();
        let (stop, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(lan_accept_loop(listener, Arc::new(cfg), stop_rx));
        Ok(Self { port, stop, handle })
    }

    /// 监听任务是否仍在运行（accept 循环致命退出后为 false，供 UI 状态对齐实况）。
    pub fn is_alive(&self) -> bool {
        !self.handle.is_finished()
    }

    /// 优雅停止：通知 accept 循环与全部在飞会话退出，等待监听任务结束；超时兜底 abort。
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let mut handle = self.handle;
        if tokio::time::timeout(LAN_STOP_GRACE, &mut handle).await.is_err() {
            handle.abort();
        }
    }

    /// 同步兜底（进程退出路径，不可 await）：直接 abort。
    pub fn abort(self) {
        self.handle.abort();
    }
}

/// 等待取消信号：stop 置 true 或发送端被丢弃（进程退出）都返回。
async fn wait_cancelled(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn lan_accept_loop(listener: TcpListener, cfg: Arc<LanConfig>, mut stop: watch::Receiver<bool>) {
    let mut consecutive_errors: u32 = 0;
    loop {
        tokio::select! {
            changed = stop.changed() => {
                // 发送端丢弃（Err）或置 true 都退出
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    consecutive_errors = 0;
                    let cfg = Arc::clone(&cfg);
                    let peer_is_loopback = peer.ip().is_loopback();
                    let mut session_stop = stop.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            // 开关关闭 → 取消在飞会话（drop future 即关闭双端 socket）
                            _ = wait_cancelled(&mut session_stop) => {
                                log::info!("lan session cancelled by toggle-off: {peer}");
                            }
                            r = handle_lan_conn(stream, peer_is_loopback, &cfg) => {
                                if let Err(e) = r {
                                    log::warn!("lan proxy conn failed from {peer}: {e}");
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    // 有限重试：短暂 backoff 后继续 accept，连续失败达阈值才退出（不因单次 EMFILE 永久停摆）
                    consecutive_errors += 1;
                    if consecutive_errors >= LAN_ACCEPT_MAX_CONSECUTIVE_ERRORS {
                        log::warn!("lan accept failed {consecutive_errors} times consecutively, listener exiting: {e}");
                        break;
                    }
                    let backoff = std::cmp::min(
                        LAN_ACCEPT_BACKOFF_BASE * (1 << (consecutive_errors - 1).min(5)),
                        LAN_ACCEPT_BACKOFF_MAX,
                    );
                    log::warn!("lan accept failed ({consecutive_errors}/{LAN_ACCEPT_MAX_CONSECUTIVE_ERRORS}), retry in {backoff:?}: {e}");
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    // 退出即代表不再监听：下游以 LanListener::is_alive() 对齐实况
    log::info!("lan accept loop exited");
}

/// LAN 连接处理：订阅端点 → 非回环鉴权 → 剥掉代理凭据后走既有分派。
async fn handle_lan_conn(
    mut stream: TcpStream,
    peer_is_loopback: bool,
    cfg: &LanConfig,
) -> std::io::Result<()> {
    let Some(mut req) = read_head(&mut stream).await? else {
        return Ok(());
    };

    // 1) 订阅端点：LAN 模式的 clash://install-config URL 指向本监听，扫码后 Clash 在此拉 YAML。
    if let Some(query_token) = subscription_request(&req.head) {
        let expected = (cfg.token_provider)().filter(|t| !t.trim().is_empty());
        if let Some(tok) = expected {
            match query_token {
                Some(q) if q == tok => {}
                Some(_) => return write_plain(&mut stream, "401 Unauthorized", "invalid token").await,
                None => return write_plain(&mut stream, "401 Unauthorized", "missing token").await,
            }
        }
        let yaml = (cfg.subscription_yaml)();
        return write_yaml(&mut stream, &yaml).await;
    }

    // 2) 鉴权：非回环来源强制 Basic（username 或 password 任一等于隧道令牌）；回环免鉴权。
    if !peer_is_loopback {
        let token = (cfg.token_provider)().filter(|t| !t.trim().is_empty());
        let authorized = token
            .as_deref()
            .map(|t| lan_auth_ok(&req.head, t))
            .unwrap_or(false);
        if !authorized {
            return write_407(&mut stream).await;
        }
    }

    // 3) 客户端给本监听的 Proxy-Authorization 不外泄给上游/目标；preamble 保持原始字节。
    req.head = strip_proxy_authorization(&req.head);
    dispatch(stream, &cfg.engine, &cfg.stats, req).await
}

/// 解析 `Proxy-Authorization: Basic base64(user:pass)`（头名大小写不敏感）。
fn basic_proxy_auth(head: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    for line in head.lines() {
        let (name, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        if !name.trim().eq_ignore_ascii_case("proxy-authorization") {
            continue;
        }
        let raw = value.trim();
        let b64 = raw
            .get(..6)
            .filter(|p| p.eq_ignore_ascii_case("basic "))
            .and_then(|_| raw.get(6..))
            .map(str::trim)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64))
            .ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (user, pass) = match text.split_once(':') {
            Some((u, p)) => (u.to_string(), p.to_string()),
            None => (text, String::new()),
        };
        return Some((user, pass));
    }
    None
}

/// C4 鉴权判定：`username` 或 `password` 任一等于隧道令牌即通过；空令牌一律拒绝。
fn lan_auth_ok(head: &str, token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }
    match basic_proxy_auth(head) {
        Some((user, pass)) => user == token || pass == token,
        None => false,
    }
}

/// 去掉请求头里的 `Proxy-Authorization`（保留首行、其余头原样与结尾空行/body）。
fn strip_proxy_authorization(head: &str) -> String {
    let (header_part, body) = match head.split_once("\r\n\r\n") {
        Some((h, b)) => (h, Some(b)),
        None => (head, None),
    };
    let mut lines = header_part.split("\r\n");
    let mut out = String::with_capacity(head.len());
    if let Some(first) = lines.next() {
        out.push_str(first);
    }
    for line in lines {
        let is_proxy_auth = line
            .split_once(':')
            .map(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
            .unwrap_or(false);
        if is_proxy_auth {
            continue;
        }
        out.push_str("\r\n");
        out.push_str(line);
    }
    out.push_str("\r\n\r\n");
    if let Some(b) = body {
        out.push_str(b);
    }
    out
}

/// 识别订阅请求 `GET /clash.yaml[?token=...]`（兼容 absolute-form）。
/// 返回值：`None` = 不是订阅请求；`Some(None)` = 订阅请求但未带 token。
fn subscription_request(head: &str) -> Option<Option<String>> {
    let first = head.lines().next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    if !method.eq_ignore_ascii_case("GET") {
        return None;
    }
    let path = match target.strip_prefix("http://") {
        Some(rest) => format!("/{}", rest.split_once('/').map(|(_, p)| p).unwrap_or("")),
        None => target.to_string(),
    };
    let (path, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
    if path != "/clash.yaml" {
        return None;
    }
    let token = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    Some(token)
}

async fn write_plain(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).await
}

/// 407：带标准 `Proxy-Authenticate` 质询，客户端可据此重试带凭据。
async fn write_407(stream: &mut TcpStream) -> std::io::Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"pproxy\"\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
}

async fn write_yaml(stream: &mut TcpStream, yaml: &str) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/yaml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        yaml.len(),
        yaml
    );
    stream.write_all(resp.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_whitelist(list: Vec<String>, tunnel: Option<String>) -> EngineConfig {
        let (_tx, rx) = watch::channel(list);
        let (_ttx, trx) = watch::channel((tunnel, Some("mock-token".into())));
        EngineConfig { whitelist: rx, tunnel: trx, ..Default::default() }
    }

    #[test]
    fn parse_connect_with_default_443() {
        let h = "CONNECT www.youtube.com:443 HTTP/1.1\r\nHost: www.youtube.com\r\n\r\n";
        let p = parse_head(h).unwrap();
        assert_eq!(p.kind, Kind::Connect);
        assert_eq!(p.host, "www.youtube.com");
        assert_eq!(p.port, 443);
    }

    #[test]
    fn parse_absolute_form_reads_host_header_port() {
        let h = "GET http://example.com:8080/path?q=1 HTTP/1.1\r\nHost: example.com:8080\r\n\r\n";
        let p = parse_head(h).unwrap();
        assert_eq!(p.kind, Kind::Plain);
        assert_eq!((p.host.as_str(), p.port), ("example.com", 8080));
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_head("not a request").is_none());
        assert!(parse_head("").is_none());
    }

    #[test]
    fn decide_whitelist_with_tunnel() {
        let cfg = cfg_with_whitelist(vec!["youtube.com".into()], Some("wss://gate/ws".into()));
        let snap = cfg.whitelist.borrow().clone();
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Whitelist), Route::Tunnel);
        assert_eq!(decide("baidu.com", &snap, pac::ProxyMode::Whitelist), Route::Direct);
    }

    #[test]
    fn decide_global_mode_tunnels_all() {
        let cfg = cfg_with_whitelist(vec![], Some("wss://gate/ws".into()));
        let snap = cfg.whitelist.borrow().clone();
        assert_eq!(decide("baidu.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
        assert_eq!(decide("google.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
    }

    #[test]
    fn decide_no_tunnel_url_still_routes_to_tunnel_to_prevent_silent_fallback() {
        let cfg = cfg_with_whitelist(vec!["youtube.com".into()], None);
        let snap = cfg.whitelist.borrow().clone();
        // R4 契约：白名单/全局目标必须 Route::Tunnel（进入建连阶段后因无配置而关闭连接，绝不回落 Direct 裸连）
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Whitelist), Route::Tunnel);
        assert_eq!(decide("www.youtube.com", &snap, pac::ProxyMode::Global), Route::Tunnel);
        // 本地回环与 Bypass 目标仍必须走 Direct 防自锁
        assert_eq!(decide("localhost", &snap, pac::ProxyMode::Global), Route::Direct);
        assert_eq!(decide("127.0.0.1", &snap, pac::ProxyMode::Global), Route::Direct);
    }

    #[test]
    fn rebuild_request_keeps_single_blank_line_and_body() {
        // F2：重建后不得多出 CRLF，否则明文 POST 的 body 被 2 字节前缀破坏
        assert_eq!(
            rebuild_request("POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"),
            "POST /y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
        assert_eq!(
            rebuild_request("GET http://x/y?a=1 HTTP/1.1\r\nHost: x\r\n\r\n"),
            "GET /y?a=1 HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }

    #[test]
    fn upstream_debug_redacts_password() {
        // F3：任何 {:?} 都不得输出明文密码
        let u = Upstream {
            host: "proxy.example.com:8899".into(),
            username: "alice".into(),
            password: "super-secret-pw".into(),
        };
        let shown = format!("{u:?}");
        assert!(!shown.contains("super-secret-pw"), "密码泄露: {shown}");
        assert!(shown.contains("***"), "密码位应脱敏: {shown}");
        assert!(shown.contains("proxy.example.com:8899") && shown.contains("alice"), "host/username 应保留便于排障: {shown}");
    }

    #[test]
    fn engine_config_debug_does_not_leak_upstream_password() {
        // EngineConfig 派生 Debug 会委托给 Upstream 的手写实现，逐层确认不泄露
        let (_tx, rx) = watch::channel(Some(Upstream {
            host: "h:1".into(),
            username: "u".into(),
            password: "top-secret".into(),
        }));
        let cfg = EngineConfig { upstream: rx, ..Default::default() };
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("top-secret"), "密码泄露: {shown}");
    }

    #[test]
    fn rewrite_absolute_to_origin_form() {
        assert_eq!(
            rewrite_first_line_absolute("GET http://example.com/path?a=1 HTTP/1.1"),
            "GET /path?a=1 HTTP/1.1"
        );
        assert_eq!(
            rewrite_first_line_absolute("GET http://example.com?a=1 HTTP/1.1"),
            "GET /?a=1 HTTP/1.1"
        );
    }

    // ---- LAN CONNECT 鉴权（C4） ----

    fn basic_header(user: &str, pass: &str) -> String {
        use base64::Engine as _;
        let cred = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Proxy-Authorization: Basic {cred}")
    }

    #[test]
    fn lan_auth_accepts_either_username_or_password() {
        let tok = "tunnel-secret";
        let head = format!("CONNECT x:443 HTTP/1.1\r\n{}\r\n\r\n", basic_header(tok, "ignored"));
        assert!(lan_auth_ok(&head, tok), "username 等于令牌应通过");
        let head = format!("CONNECT x:443 HTTP/1.1\r\n{}\r\n\r\n", basic_header("ignored", tok));
        assert!(lan_auth_ok(&head, tok), "password 等于令牌应通过");
    }

    #[test]
    fn lan_auth_rejects_missing_wrong_and_empty_token() {
        let tok = "tunnel-secret";
        let bare = "CONNECT x:443 HTTP/1.1\r\nHost: x\r\n\r\n";
        assert!(!lan_auth_ok(bare, tok), "无凭据必须拒绝");
        let wrong = format!("CONNECT x:443 HTTP/1.1\r\n{}\r\n\r\n", basic_header("a", "b"));
        assert!(!lan_auth_ok(&wrong, tok), "错误凭据必须拒绝");
        let good = format!("CONNECT x:443 HTTP/1.1\r\n{}\r\n\r\n", basic_header(tok, tok));
        assert!(!lan_auth_ok(&good, "  "), "空令牌必须 fail-closed");
    }

    #[test]
    fn lan_auth_header_name_is_case_insensitive() {
        use base64::Engine as _;
        let cred = base64::engine::general_purpose::STANDARD.encode("tok:tok");
        let head = format!("CONNECT x:443 HTTP/1.1\r\nproxy-authorization: Basic {cred}\r\n\r\n");
        assert!(lan_auth_ok(&head, "tok"));
    }

    #[test]
    fn strip_proxy_authorization_keeps_first_line_body_and_blank_line() {
        let head = format!(
            "CONNECT x:443 HTTP/1.1\r\nHost: x\r\n{}\r\n\r\nBODY",
            basic_header("tok", "tok")
        );
        let cleaned = strip_proxy_authorization(&head);
        assert_eq!(cleaned, "CONNECT x:443 HTTP/1.1\r\nHost: x\r\n\r\nBODY");
        assert!(!cleaned.to_lowercase().contains("proxy-authorization"));
    }

    #[test]
    fn subscription_request_parses_path_and_token() {
        assert_eq!(subscription_request("GET /clash.yaml?token=abc HTTP/1.1\r\n\r\n"), Some(Some("abc".into())));
        assert_eq!(subscription_request("GET /clash.yaml HTTP/1.1\r\n\r\n"), Some(None));
        assert_eq!(
            subscription_request("GET http://192.168.1.5:8899/clash.yaml?token=t HTTP/1.1\r\n\r\n"),
            Some(Some("t".into()))
        );
        assert_eq!(subscription_request("CONNECT x:443 HTTP/1.1\r\n\r\n"), None);
        assert_eq!(subscription_request("GET /pac HTTP/1.1\r\n\r\n"), None);
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::sync::Arc;

    async fn socket_pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let c = TcpStream::connect(addr).await.unwrap();
        let s = l.accept().await.unwrap().0;
        (c, s)
    }

    #[tokio::test]
    async fn direct_relay_end_to_end_echo() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = echo.accept().await {
                    tokio::spawn(async move {
                        let (mut r, mut w) = s.into_split();
                        tokio::io::copy(&mut r, &mut w).await.ok();
                    });
                }
            }
        });

        let cfg = EngineConfig::default(); // 空白名单 → 全直连
        let stats = Arc::new(EngineStats::default());
        let (mut client, srv) = socket_pair().await;
        let st2 = Arc::clone(&stats);
        tokio::spawn(async move {
            handle_conn(srv, &cfg, &st2).await.ok();
        });

        let mut buf = [0u8; 256];
        client
            .write_all(format!("CONNECT {echo_addr} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let n = client.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 Connection Established"));
        client.write_all(b"PING").await.unwrap();
        loop {
            let n = client.read(&mut buf).await.unwrap();
            if n == 0 {
                panic!("echo 连接提前关闭");
            }
            if &buf[..n] == b"PING" {
                break;
            }
        }
        assert_eq!(
            stats.direct.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "direct 计数应+1"
        );
    }

    #[tokio::test]
    async fn r4_tunnel_failure_never_silently_falls_back() {
        let (_tx, rx) = watch::channel(vec!["www.youtube.com".into()]);
        let (_ttx, trx) = watch::channel((Some("ws://127.0.0.1:9/unreachable".into()), Some("mock-token".into())));
        let cfg = EngineConfig {
            whitelist: rx,
            tunnel: trx,
            ..Default::default()
        };
        let stats = Arc::new(EngineStats::default());
        let (client, srv) = socket_pair().await;
        let mut client = client;
        let st2 = Arc::clone(&stats);
        tokio::spawn(async move {
            handle_conn(srv, &cfg, &st2).await.ok();
        });
        client
            .write_all(b"CONNECT www.youtube.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let n = client.read(&mut buf).await.unwrap_or(0);
        let body = String::from_utf8_lossy(&buf[..n]);
        assert!(
            !body.contains("200 Connection Established"),
            "隧道失败不得回 200 Established"
        );
        assert!(
            n == 0 || body.contains("denied") || body.contains("tunnel"),
            "失败语义应为显式错误/关闭，实际: {body}"
        );
        assert_eq!(
            stats.errors.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "错误计数应+1"
        );
    }

    // ---- LAN CONNECT 监听（C4） ----

    fn basic_auth_header(user: &str, pass: &str) -> String {
        use base64::Engine as _;
        let cred = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Proxy-Authorization: Basic {cred}")
    }

    fn lan_cfg(token: Option<&str>, yaml: &'static str) -> LanConfig {
        let tok = token.map(str::to_string);
        LanConfig {
            listen_addr: "127.0.0.1:0".into(),
            token_provider: Arc::new(move || tok.clone()),
            subscription_yaml: Arc::new(move || yaml.to_string()),
            engine: EngineConfig::default(),
            stats: Arc::new(EngineStats::default()),
        }
    }

    async fn echo_listener() -> std::net::SocketAddr {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = echo.accept().await {
                    tokio::spawn(async move {
                        let (mut r, mut w) = s.into_split();
                        tokio::io::copy(&mut r, &mut w).await.ok();
                    });
                }
            }
        });
        addr
    }

    async fn read_response(c: &mut TcpStream) -> String {
        let mut buf = [0u8; 1024];
        let n = c.read(&mut buf).await.unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[tokio::test]
    async fn lan_non_loopback_without_auth_gets_407() {
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, false, &cfg).await.ok();
        });
        client
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.starts_with("HTTP/1.1 407 Proxy Authentication Required"), "非回环无凭据必须 407，实际: {resp}");
        assert!(resp.contains("Proxy-Authenticate: Basic realm=\"pproxy\""), "必须带 Basic 质询，实际: {resp}");
    }

    #[tokio::test]
    async fn lan_non_loopback_wrong_token_gets_407() {
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, false, &cfg).await.ok();
        });
        let head = format!(
            "CONNECT example.com:443 HTTP/1.1\r\n{}\r\n\r\n",
            basic_auth_header("nope", "nope")
        );
        client.write_all(head.as_bytes()).await.unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.starts_with("HTTP/1.1 407"), "错误凭据必须 407，实际: {resp}");
    }

    #[tokio::test]
    async fn lan_valid_token_relays_through_existing_dispatch() {
        let echo_addr = echo_listener().await;
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, false, &cfg).await.ok();
        });
        let head = format!(
            "CONNECT {echo_addr} HTTP/1.1\r\n{}\r\n\r\n",
            basic_auth_header("tok", "tok")
        );
        client.write_all(head.as_bytes()).await.unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.contains("200 Connection Established"), "有效令牌应建连成功，实际: {resp}");
        client.write_all(b"PING").await.unwrap();
        let mut buf = [0u8; 64];
        let n = client.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"PING", "双向中继应原样回显");
    }

    #[tokio::test]
    async fn lan_loopback_peer_skips_auth() {
        let echo_addr = echo_listener().await;
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, true, &cfg).await.ok();
        });
        client
            .write_all(format!("CONNECT {echo_addr} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.contains("200 Connection Established"), "回环来源免鉴权，实际: {resp}");
    }

    #[tokio::test]
    async fn lan_subscription_requires_matching_token() {
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, false, &cfg).await.ok();
        });
        client
            .write_all(b"GET /clash.yaml HTTP/1.1\r\nHost: lan\r\n\r\n")
            .await
            .unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.starts_with("HTTP/1.1 401"), "订阅缺 token 应 401，实际: {resp}");
        assert!(resp.contains("missing token"), "401 应明示 missing token，实际: {resp}");
    }

    #[tokio::test]
    async fn lan_subscription_with_token_serves_yaml() {
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_lan_conn(srv, false, &cfg).await.ok();
        });
        client
            .write_all(b"GET /clash.yaml?token=tok HTTP/1.1\r\nHost: lan\r\n\r\n")
            .await
            .unwrap();
        let resp = read_response(&mut client).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "有效 token 应 200，实际: {resp}");
        assert!(resp.contains("mixed-port: 7890"), "应返回 YAML 正文，实际: {resp}");
    }

    #[tokio::test]
    async fn lan_listener_binds_and_stops_without_leak() {
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let listener = LanListener::start(LanConfig { listen_addr: "127.0.0.1:0".into(), ..cfg })
            .await
            .expect("LAN 监听应能绑定");
        let addr = listener.port;
        assert!(addr > 0);
        assert!(listener.is_alive(), "运行中应报在监听");
        listener.stop().await;
    }

    // ---- 有界会话生命周期（L3 必修 2）与二进制首包（L3 必修 4） ----

    #[tokio::test]
    async fn relay_idle_timeout_closes_idle_session() {
        let echo_addr = echo_listener().await;
        let (mut client, srv) = socket_pair().await;
        let target = TcpStream::connect(echo_addr).await.unwrap();
        let limits = RelayLimits { idle: Some(std::time::Duration::from_millis(80)), absolute: None };
        let h = tokio::spawn(async move { relay_bidir_with_stats(srv, target, None, limits).await });
        // 双方都不发数据 → idle 到点必须关闭，而不是永久挂着
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("idle 超时应关闭会话")
            .unwrap();
        assert_eq!(n, 0, "idle 超时后客户端应收到 EOF");
        h.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn relay_absolute_limit_closes_long_lived_session() {
        let echo_addr = echo_listener().await;
        let (mut client, srv) = socket_pair().await;
        let target = TcpStream::connect(echo_addr).await.unwrap();
        let limits = RelayLimits { idle: None, absolute: Some(std::time::Duration::from_millis(80)) };
        let h = tokio::spawn(async move { relay_bidir_with_stats(srv, target, None, limits).await });
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("absolute 上限应关闭会话")
            .unwrap();
        assert_eq!(n, 0, "absolute 上限后客户端应收到 EOF");
        h.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn relay_activity_resets_idle_deadline() {
        let echo_addr = echo_listener().await;
        let (mut client, srv) = socket_pair().await;
        let target = TcpStream::connect(echo_addr).await.unwrap();
        let idle = std::time::Duration::from_millis(600);
        let limits = RelayLimits { idle: Some(idle), absolute: None };
        tokio::spawn(async move { relay_bidir_with_stats(srv, target, None, limits).await });
        // 每 200ms 一次往返、持续 ~600ms：若活动不重置 idle，早该被关；能通即证明重置生效
        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            client.write_all(b"PING").await.unwrap();
            let mut buf = [0u8; 64];
            let n = tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut buf))
                .await
                .expect("活动会话不应被判空闲")
                .unwrap();
            assert_eq!(&buf[..n], b"PING", "活动期间会话必须保持可用");
        }
    }

    #[tokio::test]
    async fn direct_connect_forwards_binary_preamble_unchanged() {
        let echo_addr = echo_listener().await;
        let cfg = EngineConfig::default();
        let stats = Arc::new(EngineStats::default());
        let (mut client, srv) = socket_pair().await;
        tokio::spawn(async move {
            handle_conn(srv, &cfg, &stats).await.ok();
        });
        // CONNECT 头 + 二进制首包（模拟 TLS ClientHello 首段）同一包发出
        let binary = [0xFFu8, 0x00, 0xFE, 0x01, 0x80, 0xC3, 0x28];
        let mut req = format!("CONNECT {echo_addr} HTTP/1.1\r\n\r\n").into_bytes();
        req.extend_from_slice(&binary);
        client.write_all(&req).await.unwrap();

        let mut all = Vec::new();
        loop {
            let mut tmp = [0u8; 256];
            let n = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut tmp))
                .await
                .expect("应收到 200 与回显")
                .unwrap();
            assert!(n > 0, "会话提前关闭");
            all.extend_from_slice(&tmp[..n]);
            if let Some(pos) = all.windows(4).position(|w| w == b"\r\n\r\n") {
                if all.len() >= pos + 4 + binary.len() {
                    break;
                }
            }
        }
        let pos = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&all[..pos]).to_string();
        assert!(head.contains("200 Connection Established"), "应先回 200，实际: {head}");
        assert_eq!(
            &all[pos + 4..pos + 4 + binary.len()],
            &binary,
            "二进制首包不得被 UTF-8 转换污染"
        );
    }

    #[tokio::test]
    async fn lan_toggle_off_cancels_inflight_sessions() {
        let echo_addr = echo_listener().await;
        let cfg = lan_cfg(Some("tok"), "mixed-port: 7890\n");
        let listener = LanListener::start(LanConfig { listen_addr: "127.0.0.1:0".into(), ..cfg })
            .await
            .unwrap();
        assert!(listener.is_alive());
        let mut client = TcpStream::connect(("127.0.0.1", listener.port)).await.unwrap();
        // 回环来源免鉴权，直接建立到 echo 的隧道
        client
            .write_all(format!("CONNECT {echo_addr} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let n = client.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 Connection Established"));
        client.write_all(b"PING").await.unwrap();
        let n = tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"PING", "会话在飞时应可正常中继");

        // 关闭「允许局域网设备接入」：在飞会话必须被取消关闭（不是继续代理）
        listener.stop().await;
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("stop 后在飞会话必须被取消");
        match closed {
            Ok(0) => {}
            Ok(n) => panic!("stop 后仍收到 {n} 字节，会话未被取消"),
            Err(_) => {} // RST 同样视为已关闭
        }
    }
}

