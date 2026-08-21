//! `pony status / start / stop / restart`（M2 §4.2）。

use std::process::Command;

use crate::client::{AdminClient, ApiError};
use crate::render::Table;
use crate::{EXIT_FAILURE, EXIT_OK, EXIT_UNREACHABLE};

/// status：管理面 health 渲染 + 本机 systemd 状态行。
pub fn status(http: &AdminClient) -> Result<i32, String> {
    let runtime = tokio_block(async {
        match http.health().await {
            Ok(v) => Ok(v),
            Err(e @ ApiError::Connection(_)) => Err((e.to_string(), EXIT_UNREACHABLE)),
            Err(e) => Err((e.to_string(), e.exit_code())),
        }
    });
    let health = match runtime {
        Ok(v) => v,
        Err((msg, code)) => {
            eprintln!("error: {msg}");
            return Ok(code);
        }
    };

    println!(
        "admin:  {}",
        http.base_url()
    );
    println!("status: {}", health.get("status").and_then(|v| v.as_str()).unwrap_or("?"));
    println!("db:     {}", health.get("db").and_then(|v| v.as_str()).unwrap_or("?"));
    println!(
        "tokens_active: {}",
        health.get("tokens_active").and_then(|v| v.as_u64()).unwrap_or(0)
    );

    if let Some(routes) = health.get("routes").and_then(|v| v.as_object()) {
        let mut t = Table::new(&["route", "enabled", "upstream"]);
        for (name, info) in routes {
            t.push(vec![
                name.clone(),
                info.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false).to_string(),
                info.get("upstream").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
            ]);
        }
        print!("{}", t.render());
    }

    // systemd 行：非 Linux 或无 systemctl 则跳过
    #[cfg(target_os = "linux")]
    {
        if which_systemctl().is_some() {
            match Command::new("systemctl").args(["is-active", "pproxy"]).output() {
                Ok(out) => {
                    let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    println!("systemd (pproxy): {state}");
                }
                Err(_) => {}
            }
        }
    }
    Ok(EXIT_OK)
}

fn which_systemctl() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("systemctl"))
            .find(|p| p.is_file())
    })
}

/// start/stop/restart：sudo systemctl 封装，透传 exit code（M2 §4.2）。
pub fn systemd_action(action: &str) -> Result<i32, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        eprintln!("error: service start/stop/restart 仅支持本机 Linux (systemd)");
        return Ok(EXIT_FAILURE);
    }
    #[cfg(target_os = "linux")]
    {
        if which_systemctl().is_none() {
            eprintln!("error: systemctl not found — service 开关仅支持本机 systemd");
            return Ok(EXIT_FAILURE);
        }
        // 直接 Command（不经 shell，无注入面）；sudo 提权交互由终端处理
        let status = Command::new("sudo")
            .args(["systemctl", action, "pproxy"])
            .status()
            .map_err(|e| format!("failed to spawn sudo systemctl: {e}"))?;
        Ok(status.code().unwrap_or(EXIT_FAILURE))
    }
}

/// 单线程 block_on（CLI 无并发需求，不建完整 runtime）。
pub(crate) fn tokio_block<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio_block_impl(fut)
}

#[cfg(not(test))]
pub(crate) fn tokio_block_impl<T>(fut: impl std::future::Future<Output = T>) -> T {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(fut)
}

#[cfg(test)]
pub(crate) fn tokio_block_impl<T>(fut: impl std::future::Future<Output = T>) -> T {
    // 测试中已有 tokio runtime（#[tokio::test]），直接 block_on 会 panic；
    // 测试路径直接内联执行 future（当前测试不覆盖此函数）
    futures_lite_block(fut)
}

#[cfg(test)]
fn futures_lite_block<T>(fut: impl std::future::Future<Output = T>) -> T {
    // 极简 executor：本 crate 测试只用于类型检查，不做真实异步驱动
    let _ = fut;
    unreachable!("async tests use #[tokio::test] in client.rs directly")
}
