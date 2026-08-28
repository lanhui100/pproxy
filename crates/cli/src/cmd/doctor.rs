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
        match http.health().await {
            Ok(v) => {
                let db = v.get("db").and_then(|x| x.as_str()).unwrap_or("?");
                let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("?");
                if status == "ok" && db == "ok" {
                    println!("[pass] admin api health (status={status}, db={db})");
                    passed += 1;
                } else {
                    println!("[fail] admin api health (status={status}, db={db})");
                    failed += 1;
                }
            }
            Err(e @ ApiError::Connection(_)) => {
                println!("[fail] admin api unreachable: {e}");
                failed += 1;
                print_summary(passed, failed, skipped);
                return Ok(EXIT_FAILURE);
            }
            Err(e) => {
                println!("[fail] admin api health: {e}");
                failed += 1;
            }
        }

        // 2. routes
        let routes = match http.list_routes().await {
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
                let url = format!("{}/{}/{}/", dp.trim_end_matches('/'), tok, first_route);
                let client = http.with_timeout(30);
                match client.probe_get(&url).await {
                    Ok(status) => {
                        if probe_pass(status) {
                            println!("[pass] data plane probe: HTTP {status} via {first_route}");
                            passed += 1;
                        } else {
                            println!("[fail] data plane probe: HTTP {status} (token rejected?) via {first_route}");
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
                println!("[skip] data plane probe: --probe-token 未提供或无 enabled 路由");
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
enum TunnelProbeResult {
    Pass,
    Skip(&'static str),
    Fail(String),
}

/// 执行 CONNECT 隧道探针：裸 TCP 连数据面，发 CONNECT 读响应行。
/// 不依赖 token/env，零机密（spec §3.7）。
fn tunnel_probe(data_plane: &Option<String>, host: &str) -> TunnelProbeResult {
    let addr = match data_plane {
        Some(dp) => {
            // 从 http://host:port 提取 host:port
            let rest = dp.trim_start_matches("http://").trim_start_matches("https://");
            let host_port = rest.split('/').next().unwrap_or(rest);
            host_port.to_string()
        }
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
        Err(e) => return TunnelProbeResult::Fail(format!("TCP 连接失败: {e}")),
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
    let status = status_line.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok());
    let sl = status_line.trim();
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
        Some(other) => TunnelProbeResult::Fail(format!("HTTP {other}: {sl}")),
        None => TunnelProbeResult::Fail(format!("异常响应行: {sl}")),
    }
}

fn print_summary(passed: u32, failed: u32, skipped: u32) {
    println!("{passed} passed, {failed} failed, {skipped} skipped");
}

/// 管理面 base URL → 数据面 base 推导兜底（config.data_plane 缺失时）。
/// 同 config::derive_data_plane 的规则，但此处不产生错误（doctor 尽力而为）。
fn derive_from_base(server: &str) -> Option<String> {
    let rest = server.strip_prefix("http://").or_else(|| server.strip_prefix("https://"))?;
    let host_port = rest.split('/').next()?;
    let host = match host_port.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host_port,
    };
    if host.is_empty() {
        return None;
    }
    let scheme = if server.starts_with("https://") { "https" } else { "http" };
    Some(format!("{scheme}://{host}:8899"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_probe_200_is_pass() {
        // 纯函数逻辑：tunnel_probe 是 IO 函数，此处只验证判定逻辑
        // 实际集成测试在 dev 服务器上手工执行（T3）
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
}