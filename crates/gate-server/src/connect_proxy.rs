//! 标准 HTTP/1.1 CONNECT 正向代理数据面（契约 C1，`crates/gate-server/src/main.rs` 的
//! 独立裸 TCP 监听，地址取 env `GATE_CONNECT_LISTEN`）。
//!
//! 与 axum 路由面的关系：`/ws` 是 WS↔TCP 桥（Clash 等客户端需自行实现帧协议），
//! 而 Clash Meta `type: http` + `tls: true` 节点只会说标准 `CONNECT host:443`。
//! 本模块在裸 `TcpStream` 上完成首部解析 → 鉴权 → ACL → 建连 → 200 → 裸字节中继，
//! 不经过 hyper（CONNECT 200 之后是对端 TLS 密文，不是 HTTP 报文）。
//!
//! 令牌语义与 `lib.rs::handle_ws` 严格同源，不另造第二套：
//! - `usr_live_` 前缀 → `pproxy_core::TokenVerifier` Ed25519 验签 + `revoked_tokens` 撤销表；
//! - 其它 → `sha256(token) == tunnel_token_hash`（回退单口令模式）。
//!
//! Basic 解析复用 `pproxy_core::auth::parse_basic_auth`（user 或 pass 任一命中即通过）。
//!
//! fail-closed：`GATE_CONNECT_LISTEN` 未设置/为空 → 不监听，仅打一行 info。

use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use pproxy_core::UserTokenClaims;
use pproxy_gate_server::{is_valid_host, sha256_hex, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// 首部读取上限（契约 C1）
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// 首字节等待窗口
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(30);
/// 首部读取窗口（自首字节起算）
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// 空闲超时：任一方向 300 s 无字节即断开
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// 目标建连超时
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 单连接绝对生命周期上限（对齐 nginx stream `proxy_timeout 86400s`）。
/// 进程级可覆盖：`GATE_CONNECT_MAX_LIFETIME_SECS`（缺失/非法 → 86400）。
const DEFAULT_MAX_LIFETIME_SECS: u64 = 86_400;

const RESP_200: &[u8] = b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: pproxy-gate\r\n\r\n";
const RESP_400: &[u8] = b"HTTP/1.1 400 Bad Request\r\nX-Pony-Reason: bad_request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const RESP_403: &[u8] = b"HTTP/1.1 403 Forbidden\r\nX-Pony-Reason: acl_denied\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const RESP_403_QUOTA: &[u8] = b"HTTP/1.1 403 Forbidden\r\nX-Pony-Reason: quota_exceeded\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const RESP_407: &[u8] = b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"pproxy\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const RESP_502: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nX-Pony-Reason: connect_failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// 鉴权主体：单口令模式（无配额）或多租户用户（按 `claims.sub` 计量配额）。
enum Principal {
    Tunnel,
    User(Box<UserTokenClaims>),
}

impl Principal {
    fn sub(&self) -> Option<&str> {
        match self {
            Principal::Tunnel => None,
            Principal::User(c) => Some(&c.sub),
        }
    }

    fn quota(&self) -> Option<u64> {
        match self {
            Principal::Tunnel => None,
            // 与 lib.rs::quota_limit 同口径：管理员不受配额熔断
            Principal::User(c) if c.role == "admin" => None,
            Principal::User(c) => Some(c.quota_bytes),
        }
    }
}

/// 按 env 启动 CONNECT 监听；未配置则不启动（fail-closed，仅一行 info）。
pub fn spawn_from_env(cfg: Arc<ServerConfig>) {
    let raw = std::env::var("GATE_CONNECT_LISTEN").unwrap_or_default();
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        tracing::info!(
            "GATE_CONNECT_LISTEN unset/empty: HTTP CONNECT listener disabled (fail-closed)"
        );
        return;
    }
    let addr: SocketAddr = match raw.parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(value = %raw, error = %e, "GATE_CONNECT_LISTEN invalid: CONNECT listener disabled");
            return;
        }
    };
    tokio::spawn(async move {
        if let Err(e) = run(addr, cfg).await {
            tracing::error!(addr = %addr, error = %e, "CONNECT listener terminated");
        }
    });
}

async fn run(addr: SocketAddr, cfg: Arc<ServerConfig>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "HTTP CONNECT proxy listening (standard CONNECT forward proxy)");

    let max_lifetime = std::env::var("GATE_CONNECT_MAX_LIFETIME_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_LIFETIME_SECS);

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let cfg = cfg.clone();
                // 每连接一个任务，连接结束即释放（内含绝对生命周期上限，不会悬挂）
                tokio::spawn(async move {
                    serve(stream, peer, cfg, max_lifetime).await;
                });
            }
            Err(e) => {
                // accept 级错误（EMFILE 等）不终止监听循环
                tracing::warn!(error = %e, "CONNECT accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn serve(mut stream: TcpStream, peer: SocketAddr, cfg: Arc<ServerConfig>, max_lifetime: u64) {
    let raw_head = match read_head(&mut stream).await {
        Ok(h) => h,
        Err(reason) => {
            tracing::debug!(%peer, reason, "CONNECT head rejected");
            let _ = stream.write_all(RESP_400).await;
            return;
        }
    };

    let head = String::from_utf8_lossy(&raw_head);
    let (host, port) = match parse_connect_head(&head) {
        Some(v) => v,
        None => {
            tracing::debug!(%peer, "CONNECT: malformed request line");
            let _ = stream.write_all(RESP_400).await;
            return;
        }
    };

    // 1. 鉴权：缺少或错误凭据一律 407（契约 C1 失败码顺序：先鉴权后 ACL）
    let principal = match authenticate(&cfg, &head) {
        Some(p) => p,
        None => {
            tracing::warn!(%peer, host = %host, port, "CONNECT denied: missing or invalid credentials");
            let _ = stream.write_all(RESP_407).await;
            return;
        }
    };

    // 2. 目标 ACL：仅 443 + 合法 host
    if port != 443 || !acl_host_ok(&host) {
        tracing::warn!(%peer, host = %host, port, "CONNECT denied by ACL (only port 443 + valid host)");
        let _ = stream.write_all(RESP_403).await;
        return;
    }

    // 3. 配额入口检查（与 /ws 同口径：已用量 >= 配额即拒绝）
    let uid = principal.sub().map(|s| s.to_string());
    let quota = principal.quota();
    if let (Some(uid), Some(quota)) = (uid.as_deref(), quota) {
        let used = used_bytes(&cfg, uid);
        if used >= quota {
            tracing::warn!(uid = %uid, used, quota, "CONNECT denied: quota exceeded");
            let _ = stream.write_all(RESP_403_QUOTA).await;
            return;
        }
    }

    // 4. 建连目标
    let dial = match host.parse::<Ipv6Addr>() {
        Ok(_) => format!("[{host}]:{port}"),
        Err(_) => format!("{host}:{port}"),
    };
    let upstream = match tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(&dial)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(host = %host, port, error = %e, "CONNECT upstream connect failed");
            let _ = stream.write_all(RESP_502).await;
            return;
        }
        Err(_) => {
            tracing::warn!(host = %host, port, "CONNECT upstream connect timeout");
            let _ = stream.write_all(RESP_502).await;
            return;
        }
    };
    let _ = upstream.set_nodelay(true);

    if stream.write_all(RESP_200).await.is_err() {
        return;
    }
    tracing::info!(%peer, host = %host, port, "CONNECT established");

    // 5. 中继（含：客户端可能已把 TLS 首个字节与首部同包送达的 leftover）
    let leftover = head_end_offset(&raw_head)
        .map(|i| raw_head[i..].to_vec())
        .unwrap_or_default();
    relay(stream, upstream, leftover, cfg, uid, quota, max_lifetime).await;
}

/// 双向裸字节中继。任一侧 EOF → 半关闭对端写端；任一方向出错/超额/空闲超时 → 双向取消。
async fn relay(
    client: TcpStream,
    upstream: TcpStream,
    leftover: Vec<u8>,
    cfg: Arc<ServerConfig>,
    uid: Option<String>,
    quota: Option<u64>,
    max_lifetime: u64,
) {
    let (client_read, client_write) = client.into_split();
    let (upstream_read, mut upstream_write) = upstream.into_split();

    if !leftover.is_empty() && upstream_write.write_all(&leftover).await.is_err() {
        return;
    }

    let cancel = CancellationToken::new();
    let meter_up = Meter::new(uid.clone(), quota, cfg.clone());
    let meter_down = Meter::new(uid, quota, cfg.clone());

    let up = pump(client_read, upstream_write, meter_up, cancel.clone());
    let down = pump(upstream_read, client_write, meter_down, cancel.clone());

    let lifetime = Duration::from_secs(max_lifetime);
    match tokio::time::timeout(lifetime, async {
        tokio::join!(up, down);
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            tracing::info!(secs = max_lifetime, "CONNECT lifetime cap reached, closing");
            cancel.cancel();
        }
    }
}

struct Meter {
    uid: Option<String>,
    quota: Option<u64>,
    cfg: Arc<ServerConfig>,
}

impl Meter {
    fn new(uid: Option<String>, quota: Option<u64>, cfg: Arc<ServerConfig>) -> Self {
        Self { uid, quota, cfg }
    }

    /// 记账并返回是否已耗尽配额（`true` → 调用方应中止双向中继）。
    /// 顺序与 /ws 一致：先交付成功再计费。
    fn charge(&self, n: u64) -> bool {
        let (Some(uid), Some(quota)) = (self.uid.as_deref(), self.quota) else {
            return false;
        };
        let entry = self
            .cfg
            .user_used_bytes
            .entry(uid.to_string())
            .or_insert_with(|| std::sync::atomic::AtomicU64::new(0));
        let total = entry.fetch_add(n, Ordering::Relaxed) + n;
        if total >= quota {
            tracing::warn!(uid = %uid, used = total, quota, "CONNECT quota exceeded in-flight, aborting relay");
            true
        } else {
            false
        }
    }
}

async fn pump<R, W>(mut reader: R, mut writer: W, meter: Meter, cancel: CancellationToken)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => break,
            r = tokio::time::timeout(IDLE_TIMEOUT, reader.read(&mut buf)) => r,
        };
        match read {
            // 空闲超时：双向取消，避免半开连接悬挂
            Err(_) => {
                tracing::debug!("CONNECT idle timeout (300s), closing");
                cancel.cancel();
                break;
            }
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                if writer.write_all(&buf[..n]).await.is_err() {
                    cancel.cancel();
                    break;
                }
                if meter.charge(n as u64) {
                    cancel.cancel();
                    break;
                }
            }
            Ok(Err(_)) => {
                cancel.cancel();
                break;
            }
        }
    }
    // 半关闭：让对端读到 EOF（正常 EOF 路径不取消另一方向，保留半开读）
    let _ = writer.shutdown().await;
}

/// 首部读取：首字节 30 s，其余首部自首字节起 30 s，上限 16 KiB。
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, &'static str> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];

    // 阶段 1：首个字节
    let n = match tokio::time::timeout(FIRST_BYTE_TIMEOUT, stream.read(&mut chunk)).await {
        Err(_) => return Err("first_byte_timeout"),
        Ok(Err(_)) => return Err("io_error"),
        Ok(Ok(0)) => return Err("eof"),
        Ok(Ok(n)) => n,
    };
    buf.extend_from_slice(&chunk[..n]);
    if let Some(i) = head_end_offset(&buf) {
        return Ok(buf[..i].to_vec());
    }

    // 阶段 2：剩余首部
    let deadline = tokio::time::Instant::now() + HEAD_TIMEOUT;
    loop {
        if buf.len() > MAX_HEAD_BYTES {
            return Err("head_too_large");
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("head_timeout");
        }
        let n = match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
            Err(_) => return Err("head_timeout"),
            Ok(Err(_)) => return Err("io_error"),
            Ok(Ok(0)) => return Err("eof"),
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if head_end_offset(&buf).is_some() {
            if buf.len() > MAX_HEAD_BYTES {
                return Err("head_too_large");
            }
            return Ok(buf);
        }
    }
}

/// 返回首部结束（`\r\n\r\n` 之后第一个字节）的偏移；未结束返回 `None`。
fn head_end_offset(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// 解析 `CONNECT host:port HTTP/1.1`，支持 `[v6]:port`。
fn parse_connect_head(head: &str) -> Option<(String, u16)> {
    let first = head.lines().next()?.trim();
    let rest = first.strip_prefix("CONNECT ").or_else(|| first.strip_prefix("connect "))?;
    let authority = rest.split_whitespace().next()?;
    parse_authority(authority)
}

fn parse_authority(authority: &str) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse::<u16>().ok()?;
        return Some((host.to_string(), port));
    }
    let (host, port) = authority.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse::<u16>().ok()?))
}

/// ACL host 校验：`is_valid_host`（拒内网/回环，与 /ws 同源）+
/// 字符集白名单（`[A-Za-z0-9.-]` 或 IPv6 字面量），堵死花式注入。
fn acl_host_ok(host: &str) -> bool {
    if host.is_empty() || !is_valid_host(host) {
        return false;
    }
    if host.contains(':') {
        return host.parse::<Ipv6Addr>().is_ok();
    }
    host.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// 凭据校验：`X-Pony-Token` 优先，其次 `Proxy-Authorization: Basic`（user 或 pass 任一命中）。
fn authenticate(cfg: &ServerConfig, head: &str) -> Option<Principal> {
    let mut basic: Option<(String, String)> = None;
    let mut pony_token: Option<String> = None;

    for line in head.lines().skip(1) {
        let line = line.trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "proxy-authorization" => {
                basic = pproxy_core::auth::parse_basic_auth(value);
            }
            "x-pony-token" => pony_token = Some(value.trim().to_string()),
            _ => {}
        }
    }

    if let Some(tok) = pony_token.as_deref() {
        if let Some(p) = principal_for_token(cfg, tok) {
            return Some(p);
        }
    }
    if let Some((user, pass)) = basic {
        if let Some(p) = principal_for_token(cfg, &user) {
            return Some(p);
        }
        if let Some(p) = principal_for_token(cfg, &pass) {
            return Some(p);
        }
    }
    None
}

/// 令牌语义与 `lib.rs::handle_ws` 同源：`usr_live_` 走验签+撤销表，其余走 sha256 单口令。
fn principal_for_token(cfg: &ServerConfig, token: &str) -> Option<Principal> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if token.starts_with("usr_live_") {
        let verifier = match cfg.verifier.as_ref() {
            Some(v) => v,
            None => {
                tracing::warn!("CONNECT: user token presented but USER_VERIFYING_KEY not configured");
                return None;
            }
        };
        match verifier.verify_token(token) {
            Ok(claims) => {
                if cfg.revoked_tokens.contains(&claims.jti)
                    || cfg.revoked_tokens.contains(&claims.sub)
                {
                    tracing::warn!(uid = %claims.sub, jti = %claims.jti, "CONNECT: user token revoked");
                    return None;
                }
                Some(Principal::User(Box::new(claims)))
            }
            Err(e) => {
                tracing::warn!("CONNECT: user token verification failed: {e}");
                None
            }
        }
    } else {
        let expected = cfg.tunnel_token_hash.trim().to_ascii_lowercase();
        if expected.is_empty() {
            return None;
        }
        if sha256_hex(token) == expected {
            Some(Principal::Tunnel)
        } else {
            None
        }
    }
}

fn used_bytes(cfg: &ServerConfig, uid: &str) -> u64 {
    cfg.user_used_bytes
        .get(uid)
        .map(|v| v.load(Ordering::Relaxed))
        .unwrap_or(0)
}
