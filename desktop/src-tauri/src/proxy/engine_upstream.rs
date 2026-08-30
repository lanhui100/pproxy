//! 远端上游代理中继（方案 B chained 模式）：白名单流量经用户自建 HTTP 代理出网。
//!
//! 流程：CONNECT → 上游回 2xx → 回客户端 200 → 双向透传；absolute-form 请求
//! 注入 Proxy-Authorization 后原样交上游。R4：任何失败即报错关闭，绝不静默回落直连。
//!
//! 与 [`super::engine_tunnel`]（WS gate）二选一：upstream watch 有值时优先走本模块。

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::engine::{relay_bidir, Kind, ReqHead, Upstream};

fn io(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("upstream: {e}"))
}

const RETRY: u32 = 2; // 总尝试次数（首次 + 1 次重试），与 WS gate 建连语义一致

// 超时常量：测试下缩短，避免单测真等满 10 秒（生产值不变）。
#[cfg(not(test))]
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// 等上游 CONNECT 应答的超时，对齐 [`super::engine_tunnel::FIRST_FRAME_TIMEOUT`]。
/// 链式场景里「accept 后不回包」的半死连接是常态，缺超时会让 tokio 任务永久挂起、
/// 每个请求泄漏一个任务且无上限（浏览器侧表现为同步卡死）。
#[cfg(not(test))]
const FIRST_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(test)]
const FIRST_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);

fn proxy_auth_header(up: &Upstream) -> String {
    if up.username.is_empty() {
        return String::new();
    }
    let cred = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", up.username, up.password));
    format!("Proxy-Authorization: Basic {cred}\r\n")
}

/// 重建 absolute-form 明文请求：首行 + 认证头 + 其余头（含结尾空行，可能还带 body）。
///
/// 严禁改用 `head.lines()` 逐行重建：`lines()` 会把结尾的 `\r\n\r\n` 拆出一个额外的
/// 空元素，重建后变成 `...\r\n\r\n\r\n`。GET 无感，但明文 POST 的 body 会被这 2 字节
/// 前缀破坏。这里用 `split_once("\r\n")` 分离首行与剩余，剩余原样拼回。
fn rebuild_request(head: &str, auth: &str) -> String {
    // 退化输入（无 CRLF）时补一个空行，保证请求头完整
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, "\r\n\r\n"));
    format!("{first}\r\n{auth}{rest}")
}

/// 建连：dial 上游后按请求类型处理——
/// - `Kind::Connect`：发 CONNECT 并校验 2xx 应答（HTTPS 隧道）
/// - `Kind::Plain`（absolute-form 明文 HTTP）：**无需 CONNECT 握手**，请求由调用方直接转发，
///   上游按 HTTP 代理语义直接返回响应
///
/// 错误按 `ErrorKind` 分类，供调用方决定是否重试：网络级（TimedOut/ConnectionRefused）可重试，
/// 确定性拒绝（PermissionDenied，如 403/407）不重试。
async fn try_establish(up: &Upstream, parsed: &ReqHead) -> Result<TcpStream, std::io::Error> {
    let dial_deadline = tokio::time::Instant::now() + DIAL_TIMEOUT;
    let mut s = tokio::time::timeout_at(dial_deadline, TcpStream::connect(&up.host))
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("upstream: dial {} timed out after {DIAL_TIMEOUT:?}", up.host),
            )
        })?
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("upstream: dial {} failed: {e}", up.host),
            )
        })?;

    if parsed.kind != Kind::Connect {
        return Ok(s);
    }

    let auth = proxy_auth_header(up);
    let req = format!(
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n{auth}\r\n",
        host = parsed.host,
        port = parsed.port,
    );
    s.write_all(req.as_bytes()).await.map_err(|e| {
        std::io::Error::new(e.kind(), format!("upstream: write CONNECT failed: {e}"))
    })?;

    // 应答超时从「已发出 CONNECT」起算，覆盖整段读取（含逐字节滴水的半死连接）
    let resp_deadline = tokio::time::Instant::now() + FIRST_RESPONSE_TIMEOUT;
    let mut buf = Vec::with_capacity(128);
    let mut tmp = [0u8; 1024];
    loop {
        let n = tokio::time::timeout_at(resp_deadline, s.read(&mut tmp))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "upstream: {} no response in {:?}",
                        up.host, FIRST_RESPONSE_TIMEOUT
                    ),
                )
            })?
            .map_err(|e| {
                std::io::Error::new(e.kind(), format!("upstream: read failed: {e}"))
            })?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                format!("upstream: {} closed before response", up.host),
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let status = head.lines().next().unwrap_or("");
    if !(status.contains(" 2") && (status.starts_with("HTTP/1.1 ") || status.starts_with("HTTP/1.0 "))) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "upstream: {} refused: {}",
                up.host,
                status.split_once(' ').map(|x| x.1).unwrap_or(status)
            ),
        ));
    }
    Ok(s)
}

pub async fn connect_and_relay(
    mut client: TcpStream,
    parsed: ReqHead,
    head: &str,
    up: &Upstream,
) -> std::io::Result<()> {
    // 建连阶段可重试（尚未向客户端写 200）；一旦开始 relay 不再重试
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RETRY {
        match try_establish(up, &parsed).await {
            Ok(target) => {
                if parsed.kind == Kind::Connect {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;
                    return relay_bidir(client, target).await;
                }
                // absolute-form 原样交上游 HTTP 代理，仅注入认证头
                let mut target = target;
                let forwarded = rebuild_request(head, &proxy_auth_header(up));
                target.write_all(forwarded.as_bytes()).await?;
                return relay_bidir(client, target).await;
            }
            Err(e) => {
                // 仅网络级错误值得重试（半死连接、抖动）；确定性拒绝（403/407 认证失败等）
                // 重试只会放大延迟，直接放弃（对抗审核建议）
                let retriable = matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                );
                last_err = Some(e);
                if !retriable || attempt + 1 >= RETRY {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }
    }
    let err = last_err.unwrap_or_else(|| io("upstream establish failed"));
    let msg = format!(
        "502 Bad Gateway: upstream failed for {}: {}",
        parsed.host, err
    );
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        msg.len(),
        msg
    );
    let _ = client.write_all(resp.as_bytes()).await;
    Err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假上游代理：读完 CONNECT 请求后按 script 应答，随后进入 echo 模式。
    async fn fake_upstream(script: &'static str) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = l.accept().await {
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1024];
                        let mut head = Vec::new();
                        loop {
                            let n = s.read(&mut buf).await.unwrap_or(0);
                            if n == 0 { return; }
                            head.extend_from_slice(&buf[..n]);
                            if head.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                        }
                        if s.write_all(script.as_bytes()).await.is_err() { return; }
                        loop {
                            let n = s.read(&mut buf).await.unwrap_or(0);
                            if n == 0 { break; }
                            if s.write_all(&buf[..n]).await.is_err() { break; }
                        }
                    });
                }
            }
        });
        addr.to_string()
    }

    /// 抓包型上游：读一次请求后回固定应答并关闭，返回 (addr, 收到的请求字节)。
    async fn capturing_upstream(resp: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let got: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let got2 = std::sync::Arc::clone(&got);
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            loop {
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 { break; }
                got2.lock().unwrap().extend_from_slice(&buf[..n]);
                if got2.lock().unwrap().windows(4).any(|w| w == b"\r\n\r\n") { break; }
            }
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.shutdown().await;
        });
        (addr.to_string(), got)
    }

    /// 半死上游：accept 后持有连接、不读不写不关，用于验证读超时（F1）。
    /// 必须持有（`std::mem::forget`）而非 drop——drop 未读的 socket 会立刻发 RST，
    /// 客户端拿到的是连接重置而不是超时，测不出 F1 的兜底逻辑。
    async fn silent_upstream() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = l.accept().await {
                    std::mem::forget(s);
                }
            }
        });
        addr.to_string()
    }

    fn upstream_of(host: String) -> Upstream {
        Upstream { host, username: "u".into(), password: "p".into() }
    }

    /// 本机 TCP socket pair（connect_and_relay 需要 TcpStream）。
    async fn socket_pair() -> (TcpStream, TcpStream) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let c = TcpStream::connect(addr).await.unwrap();
        let s = l.accept().await.unwrap().0;
        (c, s)
    }

    #[tokio::test]
    async fn connect_ok_on_2xx_and_relays() {
        let host = fake_upstream("HTTP/1.1 200 Connection Established\r\n\r\n").await;
        let up = upstream_of(host);
        let (mut a, client) = socket_pair().await;
        let up2 = up.clone();
        let handle = tokio::spawn(async move {
            connect_and_relay(
                client,
                ReqHead { kind: Kind::Connect, host: "github.com".into(), port: 443 },
                "CONNECT github.com:443 HTTP/1.1\r\n\r\n",
                &up2,
            )
            .await
        });
        // 先收到引擎回给客户端的 200 Established
        let mut head = Vec::new();
        let mut tmp = [0u8; 128];
        loop {
            let n = a.read(&mut tmp).await.unwrap();
            head.extend_from_slice(&tmp[..n]);
            if head.windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }
        assert!(
            String::from_utf8_lossy(&head).contains("200 Connection Established"),
            "客户端应先收到 200 Established: {}",
            String::from_utf8_lossy(&head)
        );
        // 之后进入双向透传：PING 被假上游 echo 回来
        a.write_all(b"PING").await.unwrap();
        let n = a.read(&mut tmp).await.unwrap();
        assert_eq!(&tmp[..n], b"PING");
        // 必须关闭客户端写端，否则 relay_bidir 的两端拷贝永远等不到 EOF（B1 挂死根因）
        drop(a);
        handle.await.unwrap().expect("链路建立成功");
    }

    #[tokio::test]
    async fn silent_upstream_times_out_instead_of_hanging() {
        // F1：上游 accept 后不回包，必须有超时兜底而不是永久挂起
        let host = silent_upstream().await;
        let up = upstream_of(host);
        let (a, client) = socket_pair().await;
        let started = std::time::Instant::now();
        let err = connect_and_relay(
            client,
            ReqHead { kind: Kind::Connect, host: "github.com".into(), port: 443 },
            "CONNECT github.com:443 HTTP/1.1\r\n\r\n",
            &up,
        )
        .await
        .expect_err("半死上游必须超时报错");
        drop(a);
        assert!(
            err.to_string().contains("no response"),
            "错误应标明上游无应答: {err}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "不得长时间挂起，实际耗时 {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn rebuild_request_keeps_single_blank_line() {
        // F2：重建后不得多出 CRLF，否则明文 POST 的 body 被 2 字节前缀破坏
        let head = "POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY";
        let auth = "Proxy-Authorization: Basic dTpw\r\n";
        assert_eq!(
            rebuild_request(head, auth),
            "POST http://x/y HTTP/1.1\r\nProxy-Authorization: Basic dTpw\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
        // 未配置用户名时不注入认证头，其余部分不变
        assert_eq!(
            rebuild_request("GET http://x/y HTTP/1.1\r\nHost: x\r\n\r\n", ""),
            "GET http://x/y HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }

    #[tokio::test]
    async fn absolute_form_forwards_verbatim_with_auth() {
        // F2 端到端：上游收到的字节必须与原请求一致（仅多认证头），不得多出 CRLF
        let (host, got) = capturing_upstream("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK").await;
        let up = upstream_of(host);
        let (mut a, client) = socket_pair().await;
        let head = "POST http://x/y HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY";
        let up2 = up.clone();
        let handle = tokio::spawn(async move {
            connect_and_relay(
                client,
                ReqHead { kind: Kind::Plain, host: "x".into(), port: 80 },
                head,
                &up2,
            )
            .await
        });
        // 上游写完应答即 shutdown；relay 不会主动关闭客户端写端，这里只读一次应答，
        // 随后关闭客户端让 relay 两端归零（同 B1：不 drop 就永远等不到 EOF）
        let mut tmp = [0u8; 256];
        let n = a.read(&mut tmp).await.unwrap();
        let resp = String::from_utf8_lossy(&tmp[..n]).to_string();
        assert!(resp.ends_with("OK"), "应收到上游应答，实际: {resp}");
        drop(a);
        let _ = handle.await;

        let sent = String::from_utf8(got.lock().unwrap().clone()).unwrap();
        assert_eq!(
            sent,
            "POST http://x/y HTTP/1.1\r\nProxy-Authorization: Basic dTpw\r\nHost: x\r\nContent-Length: 4\r\n\r\nBODY"
        );
        assert!(!sent.ends_with("\r\n\r\n\r\n"), "请求头结尾不得多出 CRLF: {sent:?}");
    }

    #[tokio::test]
    async fn connect_fails_on_refusal_without_fallback() {
        let host = fake_upstream("HTTP/1.1 403 Denied\r\n\r\n").await;
        let up = upstream_of(host);
        let (_a, client) = socket_pair().await;
        let err = connect_and_relay(
            client,
            ReqHead { kind: Kind::Connect, host: "github.com".into(), port: 443 },
            "CONNECT github.com:443 HTTP/1.1\r\n\r\n",
            &up,
        )
        .await
        .expect_err("非 2xx 必须报错");
        assert!(err.to_string().contains("403"), "错误应携带拒绝状态: {err}");
    }
}
