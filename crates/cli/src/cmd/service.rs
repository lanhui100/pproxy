//! `pproxy status / start / stop / restart`（M2 §4.2）。

#[cfg(target_os = "linux")]
use std::process::Command;

use crate::client::{AdminClient, ApiError};
use crate::cmd::proxy_env;
use crate::render::Table;
use crate::{EXIT_FAILURE, EXIT_OK, EXIT_UNREACHABLE};

/// status：管理面 health 渲染 + 本机 systemd 状态行 + 环境代理状态。
pub fn status(http: &AdminClient) -> Result<i32, String> {
    // 服务端状态
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
            let is_root = unsafe { libc::geteuid() == 0 };
            let cmd_args = if is_root {
                vec!["is-active", "pproxy"]
            } else {
                vec!["--user", "is-active", "pproxy"]
            };
            match Command::new("systemctl").args(&cmd_args).output() {
                Ok(out) => {
                    let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let mode = if is_root { "system" } else { "user" };
                    println!("systemd (pproxy [{mode}]): {state}");
                }
                Err(_) => {}
            }
        }
    }

    // 环境代理状态
    println!();
    let _ = proxy_env::status();

    Ok(EXIT_OK)
}

#[cfg(target_os = "linux")]
fn which_systemctl() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("systemctl"))
            .find(|p| p.is_file())
    })
}

/// start/stop/restart：自动适配 root (system) vs 非 root (user) systemd 服务。
pub fn systemd_action(action: &str) -> Result<i32, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        eprintln!("error: service start/stop/restart 仅支持本机 Linux (systemd)");
        Ok(EXIT_FAILURE)
    }
    #[cfg(target_os = "linux")]
    {
        if which_systemctl().is_none() {
            eprintln!("error: systemctl not found — service 开关仅支持本机 systemd");
            return Ok(EXIT_FAILURE);
        }

        let is_root = unsafe { libc::geteuid() == 0 };
        let status = if is_root {
            Command::new("systemctl")
                .args([action, "pproxy"])
                .status()
                .map_err(|e| format!("failed to spawn systemctl: {e}"))?
        } else {
            Command::new("systemctl")
                .args(["--user", action, "pproxy"])
                .status()
                .map_err(|e| format!("failed to spawn systemctl --user: {e}"))?
        };

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
    drop(fut);
    unreachable!("async tests use #[tokio::test] in client.rs directly")
}
