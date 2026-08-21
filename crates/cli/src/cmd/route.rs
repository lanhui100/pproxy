//! `pony route ...`（M2 §4.3）。

use std::io::Write as _;

use futures::future::join_all;

use crate::client::{AdminClient, ApiError, RouteTestResult};
use crate::cmd::service::tokio_block;
use crate::render::{fmt_ts, Table};
use crate::{EXIT_FAILURE, EXIT_OK};

fn print_test_line(name: &str, r: &RouteTestResult) {
    if r.ok {
        println!(
            "{name}: ok status={} latency_ms={}",
            r.status.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
            r.latency_ms.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
        );
    } else {
        println!(
            "{name}: FAIL {}",
            r.error.clone().unwrap_or_else(|| "unknown error".into())
        );
    }
}

/// API 错误 → stderr + 退出码（通用约定，M2 §4）。
pub(crate) fn report_err(e: ApiError) -> i32 {
    let code = e.exit_code();
    let mut err = std::io::stderr();
    let _ = writeln!(err, "error: {e}");
    code
}

fn route_row(r: &crate::client::RouteInfo) -> Vec<String> {
    vec![
        if r.enabled { r.name.clone() } else { format!("{} [disabled]", r.name) },
        r.target_host.clone(),
        r.effective_upstream.clone().unwrap_or_else(|| "?".into()),
        r.override_upstream.clone().unwrap_or_else(|| "-".into()),
        fmt_ts(r.created_at),
    ]
}

pub(crate) fn list(http: &AdminClient) -> Result<i32, String> {
    tokio_block(async {
        match http.list_routes().await {
            Ok(rows) => {
                let mut t = Table::new(&["name", "target_host", "upstream", "override", "created_at"]);
                for r in &rows {
                    t.push(route_row(r));
                }
                print!("{}", t.render());
                Ok(EXIT_OK)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}

pub(crate) fn add(
    http: &AdminClient,
    name: &str,
    target_host: &str,
    upstream: Option<&str>,
) -> Result<i32, String> {
    tokio_block(async {
        match http.create_route(name, target_host, upstream).await {
            Ok((rname, eff)) => {
                match upstream {
                    Some(_) => println!("created {rname} (override upstream: {eff})"),
                    None => println!("created {rname} (auto-selected upstream: {eff})"),
                }
                Ok(EXIT_OK)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}

pub(crate) fn rm(http: &AdminClient, name: &str) -> Result<i32, String> {
    tokio_block(async {
        match http.delete_route(name).await {
            Ok(true) => {
                println!("deleted {name}");
                Ok(EXIT_OK)
            }
            Ok(false) => {
                let _ = writeln!(std::io::stderr(), "error: route not found: {name}");
                Ok(EXIT_FAILURE)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}

pub(crate) async fn test_all_inner(http: &AdminClient) -> (bool, Vec<(String, RouteTestResult)>) {
    let routes = match http.list_routes().await {
        Ok(rows) => rows.into_iter().filter(|r| r.enabled).collect::<Vec<_>>(),
        Err(e) => {
            let _ = report_err(e);
            return (false, vec![]);
        }
    };
    let tests = join_all(
        routes
            .iter()
            .map(|r| async move { (r.name.clone(), http.test_route(&r.name).await) }),
    )
    .await;
    let mut all_ok = true;
    let mut out = Vec::new();
    for (name, res) in tests {
        match res {
            Ok(r) => {
                all_ok &= r.ok;
                out.push((name, r));
            }
            Err(e) => {
                all_ok = false;
                out.push((
                    name,
                    RouteTestResult { ok: false, status: None, latency_ms: None, error: Some(e.to_string()) },
                ));
            }
        }
    }
    (all_ok, out)
}

pub(crate) fn test(http: &AdminClient, name: &str, all: bool) -> Result<i32, String> {
    tokio_block(async {
        if all {
            let (all_ok, results) = test_all_inner(http).await;
            for (name, r) in &results {
                print_test_line(name, r);
            }
            Ok(if all_ok { EXIT_OK } else { EXIT_FAILURE })
        } else {
            // 单路由：404 先于网络调用由服务端保证（test 端点前置存在性判断）
            let client = http.with_timeout(30);
            match client.test_route(name).await {
                Ok(r) => {
                    print_test_line(name, &r);
                    Ok(if r.ok { EXIT_OK } else { EXIT_FAILURE })
                }
                Err(ApiError::Status(404, m)) => {
                    let _ = writeln!(std::io::stderr(), "error: route not found: {name} ({m})");
                    Ok(EXIT_FAILURE)
                }
                Err(e) => Ok(report_err(e)),
            }
        }
    })
}

pub(crate) fn set_enabled(http: &AdminClient, name: &str, enabled: bool) -> Result<i32, String> {
    tokio_block(async {
        match http.set_route_enabled(name, enabled).await {
            Ok(()) => {
                println!("{name} {}", if enabled { "enabled" } else { "disabled" });
                Ok(EXIT_OK)
            }
            Err(ApiError::Status(404, _)) => {
                let _ = writeln!(std::io::stderr(), "error: route not found: {name}");
                Ok(EXIT_FAILURE)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}
