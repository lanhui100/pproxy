//! `pony doctor`（M2 §4.6）：顺序执行、失败不中断、末尾汇总。

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
        match (probe_token, data_plane, enabled_names.first()) {
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

        print_summary(passed, failed, skipped);
        Ok(if failed > 0 { EXIT_FAILURE } else { EXIT_OK })
    })
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
