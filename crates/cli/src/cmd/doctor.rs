//! `pproxy doctor`（M2 §4.6）：顺序执行、失败不中断、末尾汇总。
//!
//! v0.2 扩展（pproxy-connect-tunnel spec §3.7）：第 5 段 CONNECT 隧道探针。
//! 零机密设计：裸 TCP CONNECT → 数据面，读响应状态行判链路，无需 token/env。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::client::{AdminClient, ApiError};
use crate::cmd::route::test_all_inner;
use crate::cmd::service::tokio_block;
use crate::EXIT_FAILURE;
use crate::EXIT_OK;

/// doctor 数据面探测结果判定（纯函数，可测）：2xx/3xx/404/405 等非 401/403
/// 视为链路通——目标服务对 GET / 的响应码各异，只有鉴权类失败才判 fail。
fn probe_pass(status: u16) -> bool {
    status != 401 && status != 403
}

pub(crate) fn run(
    http: &AdminClient,
    probe_token: Option<&str>,
    data_plane_override: Option<String>,
    tunnel_host: &str,
) -> Result<i32, String> {
    tokio_block(async move {
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut skipped = 0u32;

        // 1. health
        let admin_ok = match http.health().await {
            Ok(v) => {
                let db = v.get("db").and_then(|x| x.as_str()).unwrap_or("?");
                let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("?");
                if status == "ok" && db == "ok" {
                    println!("[pass] admin api health (status={status}, db={db})");
                    passed += 1;
                    true
                } else {
                    println!("[fail] admin api health (status={status}, db={db})");
                    failed += 1;
                    false
                }
            }
            Err(e @ ApiError::Connection(_)) => {
                println!("[fail] admin api unreachable: {e}");
                failed += 1;
                false
            }
            Err(e) => {
                println!("[fail] admin api health: {e}");
                failed += 1;
                false
            }
        };

        // 2. routes
        let routes = if admin_ok {
            match http.list_routes().await {
                Ok(rows) => {
                    let enabled = rows.iter().filter(|r| r.enabled).count();
                    let disabled = rows.len() - enabled;
                    println!("[pass] routes: {} total, {enabled} enabled, {disabled} disabled", rows.len());
                    passed += 1;
                    rows
                }
                Err(e) => {
                    println!("[fail] routes list: {e}");
                    failed += 1;
                    vec![]
                }
            }
        } else {
            println!("[skip] routes list: admin api 不可用");
            skipped += 1;
            vec![]
        };
        let enabled_names: Vec<String> =
            routes.iter().filter(|r| r.enabled).map(|r| r.name.clone()).collect();

        // 3. 并发 test 全部 enabled 路由
        if enabled_names.is_empty() {
            println!("[skip] route tests: no enabled routes");
            skipped += 1;
        } else {
            let (all_ok, results) = test_all_inner(http).await;
            for (name, r) in &results {
                if r.ok {
                    println!(
                        "[pass] test {name}: status={} latency_ms={}",
                        r.status.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
                        r.latency_ms.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
                    );
                } else {
                    println!(
                        "[fail] test {name}: {}",
                        r.error.clone().unwrap_or_else(|| format!("status {:?}", r.status))
                    );
                }
            }
            if all_ok {
                passed += results.len() as u32;
            } else {
                let ok_n = results.iter().filter(|(_, r)| r.ok).count() as u32;
                passed += ok_n;
                failed += results.len() as u32 - ok_n;
            }
        }

        // 4. 数据面抽样
        let data_plane = data_plane_override.or_else(|| derive_from_base(http.base_url()));
        match (probe_token, &data_plane, enabled_names.first()) {
            (Some(tok), Some(dp), Some(first_route)) => {
                match probe_data_plane(http, dp, tok, first_route).await {
                    Ok(status) => {
                        if probe_pass(status) {
                            println!("[pass] data plane probe: HTTP {status} via {first_route}");
                            passed += 1;
                        } else {
                            let hint = data_plane_hint(status, tok, http.admin_token());
                            println!("[fail] data plane probe: HTTP {status}{hint} via {first_route}");
                            failed += 1;
                        }
                    }
                    Err(e) => {
                        println!("[fail] data plane probe: {e}");
                        failed += 1;
                    }
                }
            }
            _ => {
                println!("[skip] data plane probe: --probe-token 未提供或无 enabled 路由 (提示: 可运行 'pproxy token create <name>' 创建令牌测试)");
                skipped += 1;
            }
        }

        // 5. CONNECT 隧道探针（零机密，spec §3.7）
        match tunnel_probe(&data_plane, tunnel_host) {
            TunnelProbeResult::Pass => {
                println!("[pass] CONNECT tunnel probe: {tunnel_host} → 200");
                passed += 1;
            }
            TunnelProbeResult::Skip(reason) => {
                println!("[skip] CONNECT tunnel probe: {reason}");
                skipped += 1;
            }
            TunnelProbeResult::Fail(reason) => {
                println!("[fail] CONNECT tunnel probe: {tunnel_host} → {reason}");
                failed += 1;
            }
        }

        print_summary(passed, failed, skipped);
        Ok(if failed > 0 { EXIT_FAILURE } else { EXIT_OK })
    })
}

/// CONNECT 隧道探针结果。
pub(crate) enum TunnelProbeResult {
    Pass,
    Skip(&'static str),
    Fail(String),
}

/// 执行 CONNECT 隧道探针：裸 TCP 连数据面，发 CONNECT 读响应行。
/// 不依赖 token/env，零机密（spec §3.7）。
fn tunnel_probe(data_plane: &Option<String>, host: &str) -> TunnelProbeResult {
    // 严格安全校验：防止 CRLF 注入 / HTTP 请求走私
    if host.is_empty()
        || host.contains('\r')
        || host.contains('\n')
        || host.contains(' ')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '[' | ']'))
    {
        return TunnelProbeResult::Fail("非法 tunnel_host 参数格式（包含非法字符或换行符）".into());
    }

    let addr = match data_plane {
        Some(dp) => match normalize_host_port(dp) {
            Some(a) => a,
            None => return TunnelProbeResult::Skip("数据面地址格式无效"),
        },
        None => return TunnelProbeResult::Skip("数据面地址不可推导"),
    };

    let sock_addr: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(_) => {
            // 域名情况：先 DNS 解析
            let addr_ref: &str = &addr;
            match addr_ref.to_socket_addrs() {
                Ok(mut iter) => match iter.next() {
                    Some(a) => a,
                    None => return TunnelProbeResult::Fail("DNS 解析为空".into()),
                },
                Err(e) => return TunnelProbeResult::Fail(format!("DNS 解析失败: {e}")),
            }
        }
    };

    let mut stream = match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => {
            let loopback: std::net::SocketAddr = "127.0.0.1:8899".parse().unwrap();
            if sock_addr != loopback {
                if let Ok(s) = TcpStream::connect_timeout(&loopback, Duration::from_secs(5)) {
                    s
                } else {
                    return TunnelProbeResult::Fail(format!("TCP 连接失败: {e}"));
                }
            } else {
                return TunnelProbeResult::Fail(format!("TCP 连接失败: {e}"));
            }
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let req = format!("CONNECT {host} HTTP/1.1\r\n\r\n");
    if let Err(e) = stream.write_all(req.as_bytes()) {
        return TunnelProbeResult::Fail(format!("写 CONNECT 失败: {e}"));
    }

    let mut reader = BufReader::new(&stream);
    let mut status_line = String::new();
    match reader.read_line(&mut status_line) {
        Ok(0) | Err(_) => return TunnelProbeResult::Fail("无响应".into()),
        Ok(_) => {}
    }

    // 读剩余响应头（含 x-pproxy-reason）
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line.trim().is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
        }
    }

    let status = status_line.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok());
    let sl = status_line.trim();
    classify_probe_result(status, &headers, sl)
}

/// 纯函数：将 CONNECT 探针响应（状态码 + x-pproxy-reason 响应头）映射为结果。
/// spec §3.7 判定矩阵：
///   200                              → Pass
///   403 + tunnel_not_configured      → Skip（服务端未启用，无 env）
///   403 + port_not_allowed           → Fail（allowlist 配置问题）
///   403（其他）                       → Fail（host 未在 allowlist）
///   502                              → Fail（worker 链路问题）
///   其他/无法解析                     → Fail
pub(crate) fn classify_probe_result(
    status: Option<u16>,
    headers: &str,
    status_line: &str,
) -> TunnelProbeResult {
    match status {
        Some(200) => TunnelProbeResult::Pass,
        Some(403) => {
            if headers.contains("tunnel_not_configured") {
                TunnelProbeResult::Skip("服务端未启用隧道 (tunnel_not_configured)")
            } else if headers.contains("port_not_allowed") {
                TunnelProbeResult::Fail("端口非 443 (port_not_allowed)".into())
            } else {
                TunnelProbeResult::Fail("host 未在 allowlist 中 (no_tunnel_route)".into())
            }
        }
        Some(502) => TunnelProbeResult::Fail("隧道建连失败 (tunnel_failed)".into()),
        Some(other) => TunnelProbeResult::Fail(format!("HTTP {other}: {status_line}")),
        None => TunnelProbeResult::Fail(format!("异常响应行: {status_line}")),
    }
}


fn print_summary(passed: u32, failed: u32, skipped: u32) {
    println!("{passed} passed, {failed} failed, {skipped} skipped");
}

/// 提取并补齐合法的 host:port 供 SocketAddr / ToSocketAddrs 解析。纯函数可测。
pub(crate) fn normalize_host_port(url_or_addr: &str) -> Option<String> {
    let s = url_or_addr.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix("https://") {
        let host_port = rest.split('/').next()?;
        if host_port.contains(':') {
            Some(host_port.to_string())
        } else {
            Some(format!("{host_port}:443"))
        }
    } else if let Some(rest) = s.strip_prefix("http://") {
        let host_port = rest.split('/').next()?;
        if host_port.contains(':') {
            Some(host_port.to_string())
        } else {
            Some(format!("{host_port}:8899"))
        }
    } else {
        let host_port = s.split('/').next()?;
        if host_port.contains(':') {
            Some(host_port.to_string())
        } else {
            Some(format!("{host_port}:8899"))
        }
    }
}

/// 管理面 base URL → 数据面 base 推导兜底（config.data_plane 缺失时）。
/// 同 config::derive_data_plane 的规则，但此处不产生错误（doctor 尽力而为）。
fn derive_from_base(server: &str) -> Option<String> {
    let host = host_of(server)?;
    let scheme = if server.starts_with("https://") { "https" } else { "http" };
    Some(format!("{scheme}://{host}:8899"))
}

/// 提取 base URL 的 host（`http(s)://host[:port]/path` → host）。纯函数可测。
fn host_of(base: &str) -> Option<&str> {
    let rest = base.strip_prefix("http://").or_else(|| base.strip_prefix("https://"))?;
    let host_port = rest.split('/').next()?;
    let host = match host_port.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host_port,
    };
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// host 是否回环地址（含 localhost / IPv6 字面量）。纯函数可测。
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// 数据面 base 的 host 非回环时，返回本机回退 base。
/// 与 CONNECT 隧道探针同一策略（af2ee89）：管理面常因 tailnet 绑非回环
/// 地址而推导出不可达的数据面地址，本机数据面通常只绑 127.0.0.1:8899。
/// 纯函数可测。
fn loopback_fallback(base: &str) -> Option<String> {
    match host_of(base) {
        Some(host) if !is_loopback_host(host) => Some("http://127.0.0.1:8899".to_string()),
        _ => None,
    }
}

/// 数据面抽样 URL：`{base}/{token}/{route}/`（base 尾斜杠容忍）。
fn probe_url(base: &str, token: &str, route: &str) -> String {
    format!("{}/{}/{}/", base.trim_end_matches('/'), token, route)
}

/// 数据面 HTTP 抽样：先试 data_plane，连接类失败且 host 非回环时回退本机
/// 127.0.0.1:8899 重试（同一数据面、同一 URL 路径）。回退仅发生在连接失败
/// （服务端可达时返回的状态码不回退）。
async fn probe_data_plane(
    http: &AdminClient,
    data_plane: &str,
    token: &str,
    route: &str,
) -> Result<u16, String> {
    let client = http.with_timeout(30);
    let url = probe_url(data_plane, token, route);
    match client.probe_get(&url).await {
        Ok(status) => Ok(status),
        Err(ApiError::Connection(_)) => match loopback_fallback(data_plane) {
            Some(fb) => {
                let fb_url = probe_url(&fb, token, route);
                match client.probe_get(&fb_url).await {
                    Ok(status) => Ok(status),
                    Err(fb_e) => Err(format!(
                        "无法连接数据面 {data_plane}，回退 {fb} 也失败: {fb_e}"
                    )),
                }
            }
            None => Err(format!("无法连接数据面 {data_plane}（本机数据面未监听?）")),
        },
        Err(e) => Err(format!("数据面请求失败: {e}")),
    }
}

/// 数据面 401/403 拒绝时的诊断提示（纯函数可测）。常见误用：拿管理面 admin
/// token 当 probe token，但数据面按 S-P2-1 设计拒绝 admin token（crates/core
/// token.rs `verify`），无痕迹提示会误导成"服务挂了"。
fn data_plane_hint(status: u16, probe_token: &str, admin_token: &str) -> &'static str {
    match status {
        401 if probe_token == admin_token => {
            " (probe token == admin token；数据面按设计拒绝 admin token，请用 `pproxy token create` 新建数据 token 再传 --probe-token)"
        }
        401 => " (token 被数据面拒绝：已吊销/过期，或误用 admin token)",
        _ => " (token rejected?)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- classify_probe_result 判定矩阵（spec §6.1）----

    fn is_pass(r: &TunnelProbeResult) -> bool {
        matches!(r, TunnelProbeResult::Pass)
    }
    fn is_skip(r: &TunnelProbeResult) -> bool {
        matches!(r, TunnelProbeResult::Skip(_))
    }
    fn is_fail(r: &TunnelProbeResult) -> bool {
        matches!(r, TunnelProbeResult::Fail(_))
    }
    fn fail_text(r: &TunnelProbeResult) -> &str {
        match r {
            TunnelProbeResult::Fail(s) => s,
            _ => "",
        }
    }
    fn skip_text(r: &TunnelProbeResult) -> &str {
        match r {
            TunnelProbeResult::Skip(s) => s,
            _ => "",
        }
    }

    #[test]
    fn classify_200_is_pass() {
        assert!(is_pass(&classify_probe_result(Some(200), "", "")));
    }

    #[test]
    fn classify_403_not_configured_is_skip() {
        let r = classify_probe_result(
            Some(403),
            "x-pproxy-reason: tunnel_not_configured\r\n",
            "HTTP/1.1 403 Forbidden",
        );
        assert!(is_skip(&r), "应为 skip");
        assert!(skip_text(&r).contains("tunnel_not_configured"));
    }

    #[test]
    fn classify_403_port_not_allowed_is_fail() {
        let r = classify_probe_result(
            Some(403),
            "x-pproxy-reason: port_not_allowed\r\n",
            "HTTP/1.1 403 Forbidden",
        );
        assert!(is_fail(&r));
        assert!(fail_text(&r).contains("port_not_allowed"));
    }

    #[test]
    fn classify_403_no_tunnel_route_is_fail() {
        let r = classify_probe_result(
            Some(403),
            "x-pproxy-reason: no_tunnel_route\r\n",
            "HTTP/1.1 403 Forbidden",
        );
        assert!(is_fail(&r));
        assert!(fail_text(&r).contains("no_tunnel_route"));
    }

    #[test]
    fn classify_502_is_fail() {
        let r = classify_probe_result(Some(502), "", "HTTP/1.1 502 Bad Gateway");
        assert!(is_fail(&r));
        assert!(fail_text(&r).contains("tunnel_failed"));
    }

    #[test]
    fn classify_other_status_is_fail() {
        let r = classify_probe_result(Some(500), "", "HTTP/1.1 500 Internal Server Error");
        assert!(is_fail(&r));
        assert!(fail_text(&r).contains("500"));
    }

    #[test]
    fn classify_none_status_is_fail() {
        let r = classify_probe_result(None, "", "garbage");
        assert!(is_fail(&r));
        assert!(fail_text(&r).contains("garbage"));
    }

    /// 安全：Fail/Skip 输出内容不含 token/Authorization（spec §3.7 最后一条）
    #[test]
    fn classify_output_never_contains_token() {
        for status in [Some(403u16), Some(502), Some(500)] {
            let r = classify_probe_result(
                status,
                "authorization: Bearer secret-tok\r\n",
                "HTTP/1.1 403 Forbidden",
            );
            let text = fail_text(&r);
            assert!(!text.contains("secret-tok"), "输出不应含 token: {text}");
            assert!(!text.contains("Authorization"), "输出不应含 Authorization: {text}");
        }
    }

    #[test]
    fn probe_pass_known_ok_codes() {
        assert!(probe_pass(200));
        assert!(probe_pass(404));
        assert!(probe_pass(405));
    }

    #[test]
    fn probe_pass_known_fail_codes() {
        assert!(!probe_pass(401));
        assert!(!probe_pass(403));
    }

    // ---- host_of / is_loopback_host / loopback_fallback / probe_url / data_plane_hint ----

    #[test]
    fn host_of_extracts_host() {
        assert_eq!(host_of("http://<TAILNET_IP>:8900"), Some("<TAILNET_IP>"));
        assert_eq!(host_of("http://127.0.0.1:8900"), Some("127.0.0.1"));
        assert_eq!(host_of("https://localhost:8900"), Some("localhost"));
        assert_eq!(host_of("http://localhost"), Some("localhost"));
        assert_eq!(host_of("http://[::1]:8900"), Some("[::1]"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn is_loopback_host_variants() {
        for h in ["127.0.0.1", "localhost", "::1", "[::1]"] {
            assert!(is_loopback_host(h), "{h} 应判为回环");
        }
        for h in ["<TAILNET_IP>", "example.com", "gate.ponyjob.top"] {
            assert!(!is_loopback_host(h), "{h} 不应判为回环");
        }
    }

    #[test]
    fn loopback_fallback_non_loopback_returns_loopback() {
        assert_eq!(
            loopback_fallback("http://<TAILNET_IP>:8899"),
            Some("http://127.0.0.1:8899".to_string())
        );
        assert_eq!(
            loopback_fallback("https://example.com:8899"),
            Some("http://127.0.0.1:8899".to_string())
        );
    }

    #[test]
    fn loopback_fallback_loopback_is_none() {
        assert_eq!(loopback_fallback("http://127.0.0.1:8899"), None);
        assert_eq!(loopback_fallback("http://localhost:8899"), None);
    }

    #[test]
    fn probe_url_joins_and_trims_trailing_slash() {
        assert_eq!(
            probe_url("http://127.0.0.1:8899", "pony_abc", "anthropic"),
            "http://127.0.0.1:8899/pony_abc/anthropic/"
        );
        assert_eq!(
            probe_url("http://127.0.0.1:8899/", "pony_abc", "openai"),
            "http://127.0.0.1:8899/pony_abc/openai/"
        );
    }

    #[test]
    fn derive_from_base_keeps_behavior() {
        assert_eq!(
            derive_from_base("http://<TAILNET_IP>:8900"),
            Some("http://<TAILNET_IP>:8899".to_string())
        );
        assert_eq!(
            derive_from_base("http://127.0.0.1:8900"),
            Some("http://127.0.0.1:8899".to_string())
        );
        assert_eq!(derive_from_base("not a url"), None);
    }

    #[test]
    fn data_plane_hint_admin_token_401_is_explicit() {
        let h = data_plane_hint(401, "pony_admin_x", "pony_admin_x");
        assert!(h.contains("admin token"), "应明确提示 admin token: {h}");
    }

    #[test]
    fn data_plane_hint_non_admin_401_is_generic() {
        let h = data_plane_hint(401, "pony_data_x", "pony_admin_x");
        assert!(!h.contains("== admin"), "不应误判为 admin token: {h}");
        assert!(h.contains("吊销/过期"), "应提示通用原因: {h}");
    }

    #[test]
    fn data_plane_hint_403_stays_generic() {
        assert_eq!(data_plane_hint(403, "pony_admin_x", "pony_admin_x"), " (token rejected?)");
    }

    #[test]
    fn test_normalize_host_port() {
        assert_eq!(normalize_host_port("https://gate.ponyjob.top"), Some("gate.ponyjob.top:443".to_string()));
        assert_eq!(normalize_host_port("https://gate.ponyjob.top:8443"), Some("gate.ponyjob.top:8443".to_string()));
        assert_eq!(normalize_host_port("http://127.0.0.1"), Some("127.0.0.1:8899".to_string()));
        assert_eq!(normalize_host_port("http://127.0.0.1:9000"), Some("127.0.0.1:9000".to_string()));
        assert_eq!(normalize_host_port("192.168.1.100"), Some("192.168.1.100:8899".to_string()));
        assert_eq!(normalize_host_port("192.168.1.100:8080"), Some("192.168.1.100:8080".to_string()));
        assert_eq!(normalize_host_port(""), None);
    }
}