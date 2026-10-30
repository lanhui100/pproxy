//! CONNECT 数据面集成测试（契约 C1 / 门禁 G1–G3 + 首部上限 / 无凭据 / ACL）。
//!
//! 方式：起真实 `pproxy-gate-server` 二进制（独立端口 + `GATE_CONNECT_LISTEN`），
//! 直接对裸 TCP 口发 HTTP CONNECT 首部，断言状态行与响应头。
//!
//! 离线可跑（不依赖公网/外网 DNS 成功）：
//!   * 407 路径不发上游建连；
//!   * 403 路径在 ACL 阶段即拒绝；
//!   * 502 路径用 RFC 2606 保留域 `*.invalid`（必然解析失败，快速失败、确定性）；
//!   * 真实出网 204（契约 G1）需公网，由人工 curl 收据验证，不在本文件内联网。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const TOKEN: &str = "it-token-1234567890";

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

struct GateServer {
    child: Child,
    connect_port: u16,
    http_port: u16,
}

impl GateServer {
    fn start(with_connect_listener: bool) -> Self {
        let http_port = free_port();
        let connect_port = free_port();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pproxy-gate-server"));
        cmd.env("HOST", "127.0.0.1")
            .env("PORT", http_port.to_string())
            .env("TUNNEL_TOKEN_HASH", sha256_hex(TOKEN))
            .env("PROXY_SECRET", "it-proxy-secret")
            .env("GATE_ADMIN_TOKEN", "it-admin")
            .env("RUST_LOG", "pproxy_gate_server=debug")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if with_connect_listener {
            cmd.env("GATE_CONNECT_LISTEN", format!("127.0.0.1:{connect_port}"));
        } else {
            cmd.env_remove("GATE_CONNECT_LISTEN");
        }
        let child = cmd.spawn().expect("spawn pproxy-gate-server");

        let served = Self {
            child,
            connect_port,
            http_port,
        };
        served.wait_http_ready();
        if with_connect_listener {
            served.wait_port_ready(connect_port);
        }
        served
    }

    fn wait_http_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.http_port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("gate-server http listener did not come up on {}", self.http_port);
    }

    fn wait_port_ready(&self, port: u16) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("gate-server CONNECT listener did not come up on {port}");
    }

    /// 发一段 CONNECT 首部，读回响应首部（最多 8 KiB / 5 s）。
    fn request(&self, raw: &[u8]) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", self.connect_port)).expect("connect CONNECT port");
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw).expect("write head");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }
}

impl Drop for GateServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `base64("<TOKEN>:<TOKEN>")`（离线预计算，避免为一个断言引入 base64 依赖）。
const TOKEN_B64: &str = "aXQtdG9rZW4tMTIzNDU2Nzg5MDppdC10b2tlbi0xMjM0NTY3ODkw";
/// `base64("wrong-token:wrong-token")`
const WRONG_B64: &str = "d3JvbmctdG9rZW46d3JvbmctdG9rZW4=";

fn connect_head(authority: &str, extra_headers: &str) -> Vec<u8> {
    format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{extra_headers}\r\n").into_bytes()
}

fn status_line(resp: &str) -> &str {
    resp.lines().next().unwrap_or("")
}

/// G2：无凭据 → 407 + Proxy-Authenticate（不得先建连、不得泄漏上游信息）。
#[test]
fn missing_credentials_returns_407() {
    let g = GateServer::start(true);
    let resp = g.request(&connect_head("example.com:443", ""));
    assert_eq!(status_line(&resp), "HTTP/1.1 407 Proxy Authentication Required", "{resp}");
    assert!(
        resp.to_ascii_lowercase()
            .contains("proxy-authenticate: basic realm=\"pproxy\""),
        "missing Proxy-Authenticate: {resp}"
    );
}

/// 错误令牌 → 407（与 /ws 同源的 sha256 单口令语义）。
#[test]
fn wrong_credentials_returns_407() {
    let g = GateServer::start(true);
    let resp = g.request(&connect_head(
        "example.com:443",
        &format!("Proxy-Authorization: Basic {WRONG_B64}\r\n"),
    ));
    assert_eq!(status_line(&resp), "HTTP/1.1 407 Proxy Authentication Required", "{resp}");
}

/// G3：有效凭据但目标非 443 → 403（ACL）。
#[test]
fn non_443_target_returns_403() {
    let g = GateServer::start(true);
    let resp = g.request(&connect_head(
        "example.com:80",
        &format!("Proxy-Authorization: Basic {TOKEN_B64}\r\n"),
    ));
    assert_eq!(status_line(&resp), "HTTP/1.1 403 Forbidden", "{resp}");
    assert!(resp.to_ascii_lowercase().contains("x-pony-reason: acl_denied"), "{resp}");
}

/// ACL：回环/内网目标（即便 443）→ 403。
#[test]
fn internal_target_returns_403() {
    let g = GateServer::start(true);
    let resp = g.request(&connect_head(
        "127.0.0.1:443",
        &format!("Proxy-Authorization: Basic {TOKEN_B64}\r\n"),
    ));
    assert_eq!(status_line(&resp), "HTTP/1.1 403 Forbidden", "{resp}");
}

/// B2 回归（SSRF）：数字/别名/IPv6 编码变体一律 403，绝不允许放行到 dial。
#[test]
fn acl_encoding_bypass_variants_return_403() {
    let g = GateServer::start(true);
    let cases = [
        "127.0.0.1:443",            // 基线：被拒
        "0.0.0.0:443",              // unspecified
        "2130706433:443",           // 十进制 IPv4
        "0x7f000001:443",           // 十六进制 IPv4
        "0177.0.0.1:443",           // 八进制 IPv4
        "[::ffff:127.0.0.1]:443",   // IPv4-mapped IPv6
        "[::]:443",                 // 未指定 IPv6
        "[0:0:0:0:0:0:0:1]:443",    // ::1 完整写法
        "[::ffff:169.254.169.254]:443", // 云元数据（link-local 映射）
        "[::1]:443",
        "[fe80::1]:443",            // link-local
        "[fc00::1]:443",            // unique-local
    ];
    for authority in cases {
        let resp = g.request(&connect_head(
            authority,
            &format!("Proxy-Authorization: Basic {TOKEN_B64}\r\n"),
        ));
        assert_eq!(
            status_line(&resp),
            "HTTP/1.1 403 Forbidden",
            "authority={authority} must be ACL-denied, got: {resp}"
        );
    }
}

/// 凭证只填 pass（Clash 的 username/password 同值）与 `X-Pony-Token` 两条通道均被接受：
/// 用「合法 443 目标但不可解析」证明其已越过鉴权与 ACL，停在建连阶段（502）。
#[test]
fn x_pony_token_and_basic_pass_are_accepted() {
    let g = GateServer::start(true);

    let via_header = g.request(&connect_head(
        "connect-proxy-test.invalid:443",
        &format!("X-Pony-Token: {TOKEN}\r\n"),
    ));
    assert_eq!(status_line(&via_header), "HTTP/1.1 502 Bad Gateway", "{via_header}");

    let via_basic = g.request(&connect_head(
        "connect-proxy-test.invalid:443",
        &format!("Proxy-Authorization: Basic {TOKEN_B64}\r\n"),
    ));
    assert_eq!(status_line(&via_basic), "HTTP/1.1 502 Bad Gateway", "{via_basic}");
}

/// 首部上限 16 KiB：超限首部 → 400（不得无界缓冲）。
#[test]
fn oversized_head_returns_400() {
    let g = GateServer::start(true);
    let mut raw = b"CONNECT example.com:443 HTTP/1.1\r\nX-Junk: ".to_vec();
    raw.extend(std::iter::repeat_n(b'A', 20 * 1024));
    raw.extend_from_slice(b"\r\n\r\n");
    let resp = g.request(&raw);
    assert_eq!(status_line(&resp), "HTTP/1.1 400 Bad Request", "{resp}");
}

/// fail-closed：`GATE_CONNECT_LISTEN` 未设置 → 不监听 CONNECT 口（既有 HTTP 面照常）。
#[test]
fn connect_listener_is_fail_closed_without_env() {
    let g = GateServer::start(false);
    let port = g.connect_port;
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "CONNECT port {port} must stay closed when GATE_CONNECT_LISTEN is unset"
    );
}
