//! `pproxy status / start / stop / restart`（M2 §4.2）。

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
            let services = ["pproxy-server", "pproxy"];
            let mut matched = None;
            for svc in services {
                let cmd_args = if is_root {
                    vec!["is-active", svc]
                } else {
                    vec!["--user", "is-active", svc]
                };
                if let Ok(out) = Command::new("systemctl").args(&cmd_args).output() {
                    let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if state == "active" {
                        matched = Some((svc, state));
                        break;
                    } else if matched.is_none() {
                        matched = Some((svc, state));
                    }
                }
            }
            if let Some((svc, state)) = matched {
                let mode = if is_root { "system" } else { "user" };
                println!("systemd ({svc} [{mode}]): {state}");
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

/// 跨平台终止指定 PID 进程。
pub fn kill_process(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let res = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
        match res {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret == 0 {
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if !is_process_alive(pid) {
                    return true;
                }
            }
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            return true;
        }
        false
    }
}

/// 检查指定 PID 进程是否存活。
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            }
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

#[cfg(target_os = "windows")]
fn kill_orphan_pproxy_windows(current_pid: u32) -> usize {
    let out = match Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq pproxy.exe", "/FO", "CSV", "/NH"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0,
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let mut killed = 0;
    for line in s.lines() {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim().trim_matches('"')).collect();
        if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("pproxy.exe") {
            if let Ok(pid) = parts[1].parse::<u32>() {
                if pid != current_pid && is_process_alive(pid) {
                    if kill_process(pid) {
                        killed += 1;
                    }
                }
            }
        }
    }
    killed
}

/// 停止运行中的 pproxy 服务（跨平台：基于端口锁文件 + 进程树终止 + systemd）。
pub fn stop() -> Result<i32, String> {
    let mut stopped_count = 0;
    let current_pid = std::process::id();

    // 1. 扫描 ~/.pony/ 目录下的所有 pproxy_*.lock 锁文件
    let db_path = pproxy_core::store::default_db_path();
    let pony_dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));

    if let Ok(entries) = std::fs::read_dir(pony_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.starts_with("pproxy_") && file_name.ends_with(".lock") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let mut pid_opt = None;
                    let mut addr_opt = None;
                    for line in content.lines() {
                        if let Some(pid_str) = line.strip_prefix("pid=") {
                            pid_opt = pid_str.trim().parse::<u32>().ok();
                        } else if let Some(addr_str) = line.strip_prefix("addr=") {
                            addr_opt = Some(addr_str.trim().to_string());
                        }
                    }

                    if let Some(pid) = pid_opt {
                        if pid != current_pid && is_process_alive(pid) {
                            let addr_desc = addr_opt.as_deref().unwrap_or("未知地址");
                            println!("正在停止 pproxy 实例 (PID: {pid}, 监听: {addr_desc})...");
                            if kill_process(pid) {
                                println!("✓ 已成功终止 pproxy 进程 (PID: {pid})。");
                                stopped_count += 1;
                            } else {
                                eprintln!("⚠ 终止进程 (PID: {pid}) 失败，请检查操作权限。");
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    // 2. Linux 下停止 systemd 后台服务
    #[cfg(target_os = "linux")]
    {
        if which_systemctl().is_some() {
            let is_root = unsafe { libc::geteuid() == 0 };
            let cmd_args = if is_root {
                vec!["stop", "pproxy"]
            } else {
                vec!["--user", "stop", "pproxy"]
            };
            if let Ok(status) = Command::new("systemctl").args(&cmd_args).status() {
                if status.success() {
                    println!("✓ 已停止 systemd pproxy 服务。");
                    stopped_count += 1;
                }
            }
        }
    }

    // 3. Windows 下兜底终止可能无锁文件的残留 pproxy.exe 实例
    #[cfg(target_os = "windows")]
    {
        let killed = kill_orphan_pproxy_windows(current_pid);
        if killed > 0 {
            println!("✓ 已终止 {killed} 个残留的 pproxy 实例。");
            stopped_count += killed;
        }
    }

    if stopped_count > 0 {
        println!("✓ pproxy 网关服务已完全停止。");
    } else {
        println!("ℹ 未检测到正在运行的 pproxy 服务实例。");
    }

    Ok(EXIT_OK)
}

/// 启动后台服务（Linux systemd 模式；其他平台引导使用 serve）。
pub fn start() -> Result<i32, String> {
    #[cfg(target_os = "linux")]
    {
        if which_systemctl().is_none() {
            return Err("未找到 systemctl，当前环境不支持 systemd 服务管理".into());
        }
        println!("正在启动 systemd pproxy 服务...");
        let code = systemd_action("start")?;
        if code == EXIT_OK {
            println!("✓ pproxy 服务已成功启动。");
        }
        Ok(code)
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("提示: 请使用 'pproxy serve'（或添加 --lan）在前台或后台启动网关服务。");
        Ok(EXIT_OK)
    }
}

/// 重启后台服务。
pub fn restart() -> Result<i32, String> {
    let _ = stop()?;
    #[cfg(target_os = "linux")]
    {
        start()
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("提示: 服务已完全停止。请运行 'pproxy serve' 重新启动网关。");
        Ok(EXIT_OK)
    }
}

/// start/stop/restart：自动适配 root (system) vs 非 root (user) systemd 服务。
#[allow(dead_code)]
pub fn systemd_action(action: &str) -> Result<i32, String> {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("error: service start/stop/restart 依赖 Linux systemd 后台服务管理。");
        if action == "stop" {
            eprintln!("提示: 若您在前台通过 'pproxy serve' 运行网关，请在对应终端窗口按 Ctrl+C 停止服务。");
        }
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

/// 单线程 block_on（CLI 调度辅助函数）。
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
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(fut)
        }
    }
}
