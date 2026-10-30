//! 标准 HTTP/1.1 CONNECT 正向代理数据面（契约 C1；lib 模块，由 `main.rs` 与 `run_server`
//! 两条启动路径共同调用，独立裸 TCP 监听，地址取 env `GATE_CONNECT_LISTEN`）。
//!
//! 与 axum 路由面的关系：`/ws` 是 WS↔TCP 桥（Clash 等客户端需自行实现帧协议），
//! 而 Clash Meta `type: http` + `tls: true` 节点只会说标准 `CONNECT host:443`。
//! 本模块在裸 `TcpStream` 上完成首部解析 → 鉴权 → ACL → 建连 → 200 → 裸字节中继，
//! 不经过 hyper（CONNECT 200 之后是对端 TLS 密文，不是 HTTP 报文）。
//!
//! 令牌语义与 `lib.rs::handle_ws` 严格同源，不另造第二套：
//! - `usr_live_` 前缀 → `pproxy_core::TokenVerifier` Ed25519 验签 + `revoked_tokens` 撤销表；
//! - 其它 → `sha256(token) == tunnel_token_hash`（回退单口令模式，常量时间比较）。
//!
//! ACL（SSRF 防线）：**先解析再判定**，不看字符串前缀——
//! 目标 authority 先判是否为 IP 字面量（含十进制/十六进制/八进制 IPv4、IPv4-mapped IPv6、
//! `::1` 完整写法、`::` 未指定地址），是则对解析结果做公网判定；非字面量则 `lookup_host`
//! 解析出全部地址、**任一**非公网即拒；最终 `TcpStream::connect` 只用已校验的 `IpAddr`，
//! 不对域名二次解析（封 DNS rebinding 时间窗）。
//!
//! fail-closed：`GATE_CONNECT_LISTEN` 未设置/为空 → 不监听，仅打一行 info。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use pproxy_core::UserTokenClaims;
use crate::{is_valid_host, sha256_hex, ServerConfig};
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
/// 目标建连（含域名解析）超时
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

/// 目标解析失败类别：ACL 拒绝（403）vs 不可达（502）。
enum TargetError {
    AclDenied,
    Dial,
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
    let raw = match read_head(&mut stream).await {
        Ok(b) => b,
        Err(reason) => {
            tracing::debug!(%peer, reason, "CONNECT head rejected");
            let _ = stream.write_all(RESP_400).await;
            return;
        }
    };

    // 首部与首包可能同段到达（管线化 TLS ClientHello）：head 只取到 `\r\n\r\n`，
    // 其后字节必须原样作为 leftover 转发给上游，绝不丢弃。
    let (head_bytes, leftover) = match split_head(&raw) {
        Some(v) => v,
        None => {
            let _ = stream.write_all(RESP_400).await;
            return;
        }
    };
    let leftover = leftover.to_vec();

    let head = String::from_utf8_lossy(head_bytes);
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

    // 2. 目标 ACL：仅 443，且目标必须是「解析后判定为公网」的地址
    if port != 443 {
        tracing::warn!(%peer, host = %host, port, "CONNECT denied by ACL (only port 443)");
        let _ = stream.write_all(RESP_403).await;
        return;
    }
    let upstream_ip = match resolve_target(&host, port).await {
        Ok(ip) => ip,
        Err(TargetError::AclDenied) => {
            tracing::warn!(%peer, host = %host, port, "CONNECT denied by ACL (non-public target)");
            let _ = stream.write_all(RESP_403).await;
            return;
        }
        Err(TargetError::Dial) => {
            tracing::warn!(%peer, host = %host, port, "CONNECT target resolution failed");
            let _ = stream.write_all(RESP_502).await;
            return;
        }
    };

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

    // 4. 建连：只用已校验的 IpAddr，不再按域名二次解析（封 DNS rebinding 窗口）
    let upstream = match tokio::time::timeout(
        DIAL_TIMEOUT,
        TcpStream::connect(SocketAddr::new(upstream_ip, port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(host = %host, ip = %upstream_ip, port, error = %e, "CONNECT upstream connect failed");
            let _ = stream.write_all(RESP_502).await;
            return;
        }
        Err(_) => {
            tracing::warn!(host = %host, ip = %upstream_ip, port, "CONNECT upstream connect timeout");
            let _ = stream.write_all(RESP_502).await;
            return;
        }
    };
    let _ = upstream.set_nodelay(true);

    if stream.write_all(RESP_200).await.is_err() {
        return;
    }
    tracing::info!(%peer, host = %host, ip = %upstream_ip, port, "CONNECT established");

    // 5. 中继（leftover = 首部同包到达的 payload 首段）
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
///
/// 返回**本次读到的全部字节**（可能含首部之后的 payload）；切分交给 [`split_head`]，
/// 绝不在此截断——否则与首部同包到达的 TLS ClientHello 会被静默丢弃。
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, &'static str> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];

    // 阶段 1：首个字节（本段可能已含完整首部 + payload，必须整段保留）
    let n = match tokio::time::timeout(FIRST_BYTE_TIMEOUT, stream.read(&mut chunk)).await {
        Err(_) => return Err("first_byte_timeout"),
        Ok(Err(_)) => return Err("io_error"),
        Ok(Ok(0)) => return Err("eof"),
        Ok(Ok(n)) => n,
    };
    buf.extend_from_slice(&chunk[..n]);
    if head_end_offset(&buf).is_some() {
        // 整段返回（含首部之后的 payload），由 split_head 切分
        return Ok(buf);
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
            return Ok(buf);
        }
    }
}

/// 在 `\r\n\r\n` 处切分：`(首部含空行, 其后 payload)`。首部超限返回 `None`。
fn split_head(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = head_end_offset(buf)?;
    if end > MAX_HEAD_BYTES {
        return None;
    }
    Some((&buf[..end], &buf[end..]))
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

/// 把目标 host 解析成一个**已校验为公网**的 IP，供建连使用（SSRF 防线）。
///
/// 顺序：IP 字面量（含十进制/十六进制/八进制 IPv4、IPv4-mapped IPv6）→ 直接判定；
/// 否则 DNS 解析全部地址 → 任一非公网即拒 → 返回首个地址。
async fn resolve_target(host: &str, port: u16) -> Result<IpAddr, TargetError> {
    if let Some(ip) = parse_ip_literal(host) {
        return if is_public_ip(ip) {
            Ok(ip)
        } else {
            Err(TargetError::AclDenied)
        };
    }

    // 域名：字符集与基本形态校验（拒 localhost / 内网后缀 / 超长）
    if !is_domain_host(host) || !is_valid_host(host) {
        return Err(TargetError::AclDenied);
    }

    let addrs = match tokio::time::timeout(DIAL_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
    {
        Ok(Ok(it)) => it.map(|sa| sa.ip()).collect::<Vec<_>>(),
        _ => return Err(TargetError::Dial),
    };
    if addrs.is_empty() {
        return Err(TargetError::Dial);
    }
    // Lead 裁决口径从严：任一解析结果非公网即整体拒绝
    if addrs.iter().any(|ip| !is_public_ip(*ip)) {
        return Err(TargetError::AclDenied);
    }
    Ok(addrs[0])
}

/// IP 字面量解析：标准写法 + 十进制/十六进制/八进制 IPv4（`2130706433`、`0x7f000001`、`0177.0.0.1`）。
fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    parse_legacy_ipv4(host).map(IpAddr::V4)
}

fn is_domain_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// 兼容 inet_aton 风格的 IPv4 字面量（`a`/`a.b`/`a.b.c`/`a.b.c.d`，各段可为十进制/0x 十六进制/前导 0 八进制）。
fn parse_legacy_ipv4(s: &str) -> Option<Ipv4Addr> {
    if s.is_empty() || s.contains(':') {
        return None;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 4 {
        return None;
    }
    let nums: Vec<u32> = parts
        .iter()
        .map(|p| parse_u32_auto(p))
        .collect::<Option<Vec<_>>>()?;
    let last = nums.len() - 1;
    let mut acc: u32 = 0;
    for (i, n) in nums.iter().enumerate() {
        if i == last {
            // 末段占用剩余全部位宽（8..=32 位），用 u64 避免 1u32 << 32 溢出
            let bits: u32 = 8 * (4 - last as u32);
            let space: u64 = 1u64 << bits;
            if (*n as u64) >= space {
                return None;
            }
            acc = (((acc as u64) << bits) | (*n as u64)) as u32;
        } else {
            if *n > 255 {
                return None;
            }
            acc = (acc << 8) | n;
        }
    }
    Some(Ipv4Addr::from(acc))
}

fn parse_u32_auto(p: &str) -> Option<u32> {
    if p.is_empty() {
        return None;
    }
    if let Some(h) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
        if h.is_empty() {
            return None;
        }
        u32::from_str_radix(h, 16).ok()
    } else if p.len() > 1 && p.starts_with('0') {
        u32::from_str_radix(&p[1..], 8).ok()
    } else {
        p.parse::<u32>().ok()
    }
}

/// 公网可达性判定：loopback / private / link-local / unspecified / multicast /
/// unique-local / CGNAT / 保留与文档段 全部视为非公网。
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
    {
        return false;
    }
    let o = ip.octets();
    if o[0] == 0 {
        return false; // 0.0.0.0/8
    }
    if o[0] == 100 && (64..=127).contains(&o[1]) {
        return false; // 100.64.0.0/10 CGNAT / Tailscale
    }
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return false; // 198.18.0.0/15 benchmark
    }
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return false; // 192.0.0.0/24 IETF
    }
    if o[0] == 192 && o[1] == 88 && o[2] == 99 {
        return false; // 192.88.99.0/24 6to4 relay anycast
    }
    if o[0] >= 240 {
        return false; // 240.0.0.0/4 保留（含 255.255.255.255）
    }
    true
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    // IPv4-mapped（::ffff:a.b.c.d）→ 按内层 IPv4 判定
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let seg = ip.segments();
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    if (seg[0] & 0xfe00) == 0xfc00 {
        return false; // fc00::/7 unique local
    }
    if (seg[0] & 0xffc0) == 0xfe80 {
        return false; // fe80::/10 link local
    }
    if (seg[0] & 0xffc0) == 0xfec0 {
        return false; // fec0::/10 site local（已弃用）
    }
    if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        return false; // 2001:db8::/32 文档段
    }
    if seg[0] == 0x0064 && seg[1] == 0xff9b {
        return false; // 64:ff9b::/96 NAT64（可映射任意 v4）
    }
    // ::/96 已弃用的 IPv4-compatible 与未指定段：一律拒绝
    if seg[..6].iter().all(|s| *s == 0) {
        return false;
    }
    true
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
        // 常量时间比较（与 lib.rs 管理端点同口径），杜绝时序侧信道
        if pproxy_core::user::constant_time_eq(sha256_hex(token).as_bytes(), expected.as_bytes()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn pipe_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    /// B1 回归：首部与 payload 同一次 write 到达时，payload 必须原样保留为 leftover。
    #[tokio::test]
    async fn head_and_payload_in_single_write_keeps_leftover() {
        let (mut client, mut server) = pipe_pair().await;
        let head = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        let payload = [0x16u8, 0x03, 0x01, 0x02, 0x00, 0x01, 0xAA, 0xBB];
        let mut one_write = head.to_vec();
        one_write.extend_from_slice(&payload);
        client.write_all(&one_write).await.unwrap();

        let raw = read_head(&mut server).await.expect("head ok");
        let (head_part, leftover) = split_head(&raw).expect("head terminated");
        assert!(head_part.ends_with(b"\r\n\r\n"));
        assert_eq!(leftover, &payload, "payload sent with head must be preserved");
        assert_eq!(head_part, head, "head must be sliced exactly at CRLFCRLF");
    }

    /// 首部拆两次 write 仍正常（不得因改动回归）。
    #[tokio::test]
    async fn head_split_across_writes_is_ok() {
        let (mut client, mut server) = pipe_pair().await;
        let (a, b) = b"CONNECT example.com:443 HTTP/1.1\r\nHost: ex".split_at(20);
        client.write_all(a).await.unwrap();
        client.write_all(b).await.unwrap();
        client
            .write_all(b"ample.com:443\r\n\r\n")
            .await
            .unwrap();
        let raw = read_head(&mut server).await.expect("head ok");
        let (_, leftover) = split_head(&raw).unwrap();
        assert!(leftover.is_empty());
    }

    /// 首部超 16 KiB → Err（不得无界缓冲）。
    #[tokio::test]
    async fn oversized_head_is_rejected() {
        let (mut client, mut server) = pipe_pair().await;
        let mut raw = b"CONNECT example.com:443 HTTP/1.1\r\nX-Junk: ".to_vec();
        raw.extend(std::iter::repeat_n(b'A', 20 * 1024));
        raw.extend_from_slice(b"\r\n\r\n");
        client.write_all(&raw).await.unwrap();
        assert!(read_head(&mut server).await.is_err());
    }

    /// B2 回归：编码变体必须解析为对应 IP（后续 is_public_ip 拒绝）。
    #[test]
    fn ip_literal_encodings_are_parsed() {
        let cases: &[(&str, &str)] = &[
            ("127.0.0.1", "127.0.0.1"),
            ("2130706433", "127.0.0.1"),
            ("0x7f000001", "127.0.0.1"),
            ("0177.0.0.1", "127.0.0.1"),
            ("0.0.0.0", "0.0.0.0"),
            ("0", "0.0.0.0"),
            ("::1", "::1"),
            ("0:0:0:0:0:0:0:1", "::1"),
            ("::", "::"),
            ("::ffff:127.0.0.1", "::ffff:127.0.0.1"),
            ("::ffff:169.254.169.254", "::ffff:169.254.169.254"),
        ];
        for (input, expect) in cases {
            let ip = parse_ip_literal(input).unwrap_or_else(|| panic!("{input} must parse as IP"));
            assert_eq!(ip.to_string(), *expect, "input={input}");
            assert!(!is_public_ip(ip), "{input} -> {ip} must be non-public");
        }
    }

    #[test]
    fn public_literals_pass_acl() {
        for s in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            let ip = parse_ip_literal(s).expect("literal");
            assert!(is_public_ip(ip), "{s} should be public");
        }
        for s in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
        ] {
            let ip = parse_ip_literal(s).expect("literal");
            assert!(!is_public_ip(ip), "{s} must be non-public");
        }
        for s in ["fc00::1", "fe80::1", "::ffff:10.0.0.1", "2001:db8::1"] {
            let ip = parse_ip_literal(s).expect("literal");
            assert!(!is_public_ip(ip), "{s} must be non-public");
        }
    }

    /// 域名路径：带内网解析的名字不得被当作 IP 字面量放过，且 ACL 判定必须落到 IP。
    #[tokio::test]
    async fn literal_targets_resolve_without_dns() {
        let err = resolve_target("0x7f000001", 443).await;
        assert!(matches!(err, Err(TargetError::AclDenied)));
        let err = resolve_target("0.0.0.0", 443).await;
        assert!(matches!(err, Err(TargetError::AclDenied)));
        let err = resolve_target("::ffff:127.0.0.1", 443).await;
        assert!(matches!(err, Err(TargetError::AclDenied)));
    }
}
