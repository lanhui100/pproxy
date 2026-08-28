#![allow(dead_code)] // 所有 pub fn 均从 main.rs 路由调用，编译器跨模块分析保守

//! `pproxy on / off / status / env` — 本机环境代理开关与挂起/恢复。
//!
//! 通过设置/清除 `http_proxy` / `https_proxy` / `no_proxy` 环境变量
//! 控制当前 shell 或持久化配置的代理转发。
//!
//! 持久化模式：写入 `~/.pony/proxy.env`，shell 加载后生效。
//! 临时挂起模式：保存快照到 `~/.pony/env-saved.json`，输出 eval 代码供用户就地执行。
//!
//! ## 安全设计
//! - no_proxy 默认覆盖私有 IP 段、K8s 集群地址、本地链路，避免代理拦截兄弟项目。
//! - `pproxy on` 自动探测 ~/.kube/config 中的 API server 地址并追加到 no_proxy。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::EXIT_OK;

/// 代理环境变量配置文件名（持久化 export 脚本）。
const PROXY_ENV_FILE: &str = "proxy.env";

/// 代理快照文件名（env suspend 保存）。
const ENV_SNAPSHOT_FILE: &str = "env-saved.json";

/// 默认的直连列表（no_proxy）。
/// 覆盖私有 IP 段、K8s 内部域名、CG-NAT 段，防止兄弟项目（如 tilt/kubectl）被代理拦截。
const DEFAULT_NO_PROXY: &str = "\
localhost,\
127.0.0.1,\
::1,\
.local,\
10.0.0.0/8,\
172.16.0.0/12,\
192.168.0.0/16,\
100.64.0.0/10,\
.svc,\
.svc.cluster.local,\
.internal";

/// 应急脚本的内嵌模板（供 `pproxy env generate-script` 输出）。
const MITIGATE_SCRIPT: &str = r#"#!/bin/bash
# 兄弟项目代理隔离应急工具
# 由 `pproxy env generate-script` 生成
#
# 用法: source $0 [on|off]
#   on  - 临时关闭代理环境变量（保存原值供恢复）
#   off - 恢复之前保存的代理环境变量

PPROXY_SAVED_FILE="${PPROXY_SAVED_FILE:-/tmp/.pproxy-saved-env}"

case "${1:-}" in
    on)
        cat > "$PPROXY_SAVED_FILE" <<SAVED_EOF
export http_proxy="${http_proxy:-}"
export https_proxy="${https_proxy:-}"
export no_proxy="${no_proxy:-}"
export HTTP_PROXY="${HTTP_PROXY:-}"
export HTTPS_PROXY="${HTTPS_PROXY:-}"
export NO_PROXY="${NO_PROXY:-}"
SAVED_EOF
        unset http_proxy https_proxy no_proxy
        unset HTTP_PROXY HTTPS_PROXY NO_PROXY
        echo "✓ 代理环境变量已临时清除（保存至 $PPROXY_SAVED_FILE）"
        echo "  之后执行: source $0 off"
        ;;
    off)
        if [ -f "$PPROXY_SAVED_FILE" ]; then
            source "$PPROXY_SAVED_FILE"
            rm -f "$PPROXY_SAVED_FILE"
            echo "✓ 代理环境变量已恢复"
        else
            echo "⚠ 没有找到保存的代理环境变量 ($PPROXY_SAVED_FILE)"
        fi
        ;;
    *)
        echo "用法: source $0 [on|off]"
        echo "  on  - 临时关闭代理并保存原值（供兄弟项目启动）"
        echo "  off - 恢复之前保存的代理环境变量"
        ;;
esac
"#;

// ─── 路径辅助 ────────────────────────────────────────────────

/// 获取代理持久化文件路径。
fn proxy_env_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(&home).join(".pony").join(PROXY_ENV_FILE))
}

/// 获取快照文件路径。
fn snapshot_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(&home).join(".pony").join(ENV_SNAPSHOT_FILE))
}

/// 获取应急脚本默认输出路径。
fn default_script_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(&home).join(".pony").join("mitigate.sh"))
}

// ─── K8s API 地址自动探测 ────────────────────────────────────

/// 探测本机已知 K8s 集群的 API server 地址，追加到 no_proxy。
/// 解析 `~/.kube/config` 或 `$KUBECONFIG` 中的 `server:` 字段。
fn detect_k8s_cluster_entries() -> Vec<String> {
    let kubeconfig_path = std::env::var("KUBECONFIG")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".kube").join("config"))
        });
    let kubeconfig_path = match kubeconfig_path {
        Some(p) if p.exists() => p,
        _ => return vec![],
    };
    let content = match fs::read_to_string(&kubeconfig_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut entries: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("server:") {
            let url = rest.trim().trim_matches('"').trim_matches('\'');
            if let Some(host) = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
            {
                let host = host.split(':').next().unwrap_or(host);
                if !host.is_empty() {
                    entries.push(host.to_string());
                }
            }
        }
    }
    entries
}

// ─── 快照结构 ────────────────────────────────────────────────

/// 代理环境变量快照，JSON 序列化保存。
#[derive(Debug, Serialize, Deserialize)]
struct SavedProxyEnv {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    no_proxy: Option<String>,
    http_proxy_upper: Option<String>,
    https_proxy_upper: Option<String>,
    no_proxy_upper: Option<String>,
}

impl SavedProxyEnv {
    fn from_current() -> Self {
        Self {
            http_proxy: std::env::var("http_proxy").ok().filter(|s| !s.is_empty()),
            https_proxy: std::env::var("https_proxy").ok().filter(|s| !s.is_empty()),
            no_proxy: std::env::var("no_proxy").ok().filter(|s| !s.is_empty()),
            http_proxy_upper: std::env::var("HTTP_PROXY").ok().filter(|s| !s.is_empty()),
            https_proxy_upper: std::env::var("HTTPS_PROXY").ok().filter(|s| !s.is_empty()),
            no_proxy_upper: std::env::var("NO_PROXY").ok().filter(|s| !s.is_empty()),
        }
    }
}

// ─── 公共 API ────────────────────────────────────────────────

/// 检测当前代理状态。
pub fn status() -> Result<i32, String> {
    let env_path = proxy_env_path()?;

    let persistent = if env_path.exists() {
        match fs::read_to_string(&env_path) {
            Ok(content) => content.contains("export http_proxy="),
            Err(_) => false,
        }
    } else {
        false
    };

    let current_http = std::env::var("http_proxy").ok().filter(|s| !s.is_empty());
    let current_https = std::env::var("https_proxy").ok().filter(|s| !s.is_empty());
    let current_no = std::env::var("no_proxy").ok().filter(|s| !s.is_empty());

    println!("┌─ 环境代理状态 ────────────────────────────────");
    if persistent {
        println!("│ 持久化配置: 已启用");
    } else {
        println!("│ 持久化配置: 未启用");
    }
    match (&current_http, &current_https) {
        (Some(h), Some(s)) => {
            println!("│ 当前 shell:  已启用");
            println!("│   http_proxy:  {}", h);
            println!("│   https_proxy: {}", s);
            if let Some(no) = current_no {
                println!("│   no_proxy:    {}", no);
            }
        }
        (Some(h), None) => {
            println!("│ 当前 shell:  部分启用");
            println!("│   http_proxy:  {}", h);
            println!("│   https_proxy: (未设置)");
        }
        (None, Some(s)) => {
            println!("│ 当前 shell:  部分启用");
            println!("│   http_proxy:  (未设置)");
            println!("│   https_proxy: {}", s);
        }
        (None, None) => {
            if persistent {
                println!("│ 当前 shell:   未加载（需要 source {}）", env_path.display());
            } else {
                println!("│ 当前 shell:   未启用");
            }
        }
    }
    println!("└─────────────────────────────────────────────────");

    // K8s 冲突诊断
    if current_http.is_some() || current_https.is_some() {
        let k8s_entries = detect_k8s_cluster_entries();
        if !k8s_entries.is_empty() {
            let no_proxy_current = std::env::var("no_proxy")
                .or_else(|_| std::env::var("NO_PROXY"))
                .unwrap_or_default();
            let missing: Vec<&str> = k8s_entries
                .iter()
                .filter(|ip| !no_proxy_current.contains(ip.as_str()))
                .map(|s| s.as_str())
                .collect();
            if !missing.is_empty() {
                println!("│");
                println!("│ ⚠ 以下 K8s API 地址未在 no_proxy 中，可能影响 kubectl 等工具：");
                for ip in &missing {
                    println!("│   - {ip}");
                }
                println!("│   建议: 执行 pproxy on 重新生成配置（含自动探测），或手动添加");
            }
        }
    }

    println!();
    println!("开启代理:  pproxy on");
    println!("关闭代理:  pproxy off");
    println!("临时挂起:  eval \"$(pproxy env suspend)\"");
    println!("恢复代理:  eval \"$(pproxy env resume)\"");
    println!("生成脚本:  pproxy env generate-script");
    println!();
    println!("提示: 开启后请在当前 shell 执行:");
    println!("  source {}", env_path.display());

    Ok(EXIT_OK)
}

/// 开启/关闭持久化代理（保留向后兼容）。
pub fn toggle(enable: bool) -> Result<i32, String> {
    if enable {
        enable_proxy()
    } else {
        disable_proxy()
    }
}

/// 开启持久化代理 + 硬清除（--hard 模式：输出 eval 线段供就地执行）。
pub fn toggle_hard(enable: bool) -> Result<i32, String> {
    if enable {
        enable_proxy()
    } else {
        disable_proxy_hard()
    }
}

// ─── 开启（持久化） ──────────────────────────────────────────

/// 开启：计算代理地址 + 探测 K8s 集群地址，写入 ~/.pony/proxy.env。
fn enable_proxy() -> Result<i32, String> {
    let data_plane = match config::load() {
        Ok(cfg) => config::derive_data_plane(&cfg)
            .unwrap_or_else(|_| "http://127.0.0.1:8899".to_string()),
        Err(_) => "http://127.0.0.1:8899".to_string(),
    };

    let k8s_entries = detect_k8s_cluster_entries();
    let extra_no_proxy = if k8s_entries.is_empty() {
        String::new()
    } else {
        format!(",{}", k8s_entries.join(","))
    };
    let no_proxy = format!("{}{}", DEFAULT_NO_PROXY, extra_no_proxy);

    let path = proxy_env_path()?;
    let path_display = path.display().to_string();
    let content = format!(
        r#"# Pony Proxy — 环境代理配置
# 由 `pproxy on` 生成，`pproxy off` 清除
# 加载方式: source {}
export http_proxy="{data_plane}"
export https_proxy="{data_plane}"
export no_proxy="{no_proxy}"
"#,
        path_display
    );

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(&path, &content).map_err(|e| format!("写入文件失败: {e}"))?;

    println!("✓ 环境代理已启用");
    println!();
    println!("代理地址:  {data_plane}");
    println!("直连列表:  {no_proxy}");
    if !k8s_entries.is_empty() {
        println!("  (自动探测 K8s API: {})", k8s_entries.join(", "));
    }
    println!();
    println!("请在当前 shell 中执行以下命令加载代理配置：");
    println!("  source {}", path_display);
    println!();
    println!("或将其添加到 ~/.bashrc / ~/.zshrc 以永久生效：");
    println!("  echo 'source {}' >> ~/.bashrc", path_display);

    Ok(EXIT_OK)
}

// ─── 关闭（持久化）：写入 unset 脚本 ──────────────────────────

/// 关闭：写入 unset 脚本到 ~/.pony/proxy.env（而非删除文件），
/// 这样用户可以 source 来清除环境变量。
fn disable_proxy() -> Result<i32, String> {
    let path = proxy_env_path()?;
    let path_display = path.display().to_string();
    let content = format!(
        r#"# Pony Proxy — 代理关闭配置
# 由 `pproxy off` 生成
# 加载方式: source {}
unset http_proxy
unset https_proxy
unset no_proxy
unset HTTP_PROXY
unset HTTPS_PROXY
unset NO_PROXY
"#,
        path_display
    );

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(&path, &content).map_err(|e| format!("写入文件失败: {e}"))?;

    println!("✓ 代理关闭配置已写入");
    println!();
    println!("请在当前 shell 中执行以下命令清除代理环境变量：");
    println!("  source {}", path_display);
    println!();
    println!("如需就地清除（无需 source），请执行：");
    println!("  eval \"$(pproxy off --hard)\"");

    Ok(EXIT_OK)
}

/// 关闭 --hard 模式：写入 unset 文件 + 直接清除当前进程环境 + 输出 eval 代码。
fn disable_proxy_hard() -> Result<i32, String> {
    let _ = disable_proxy();

    let vars = [
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ];
    for var in &vars {
        std::env::remove_var(var);
    }

    println!();
    println!("或者直接 eval 以下代码清除当前 shell 环境：");
    println!("  eval \"$(pproxy env suspend)\"  # 保存并清除");
    println!("  eval \"$(pproxy env resume)\"   # 恢复");

    Ok(EXIT_OK)
}

// ─── env 子命令组：suspend / resume / generate-script ─────────

/// `pproxy env suspend`：保存当前代理环境变量到快照文件，
/// 输出 shell eval 代码供用户就地清除。
pub fn env_suspend() -> Result<i32, String> {
    let saved = SavedProxyEnv::from_current();
    let path = snapshot_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&saved).map_err(|e| format!("序列化失败: {e}"))?,
    )
    .map_err(|e| format!("写入快照失败: {e}"))?;

    println!("# pproxy env suspend — 代理快照已保存到 {}", path.display());
    println!("unset http_proxy https_proxy no_proxy");
    println!("unset HTTP_PROXY HTTPS_PROXY NO_PROXY");
    println!("echo \"✓ 代理已临时清除（运行 pproxy env resume 恢复）\"");

    Ok(EXIT_OK)
}

/// `pproxy env resume`：从快照文件恢复代理环境变量，输出 eval 代码。
pub fn env_resume() -> Result<i32, String> {
    let path = snapshot_path()?;
    let saved: SavedProxyEnv = serde_json::from_str(
        &fs::read_to_string(&path)
            .map_err(|_| format!("代理快照未找到: {} — 请先执行 pproxy env suspend", path.display()))?,
    )
    .map_err(|e| format!("解析快照失败: {e}"))?;

    if let Some(v) = &saved.http_proxy {
        println!("export http_proxy=\"{v}\"");
    }
    if let Some(v) = &saved.https_proxy {
        println!("export https_proxy=\"{v}\"");
    }
    if let Some(v) = &saved.no_proxy {
        println!("export no_proxy=\"{v}\"");
    }
    if let Some(v) = &saved.http_proxy_upper {
        println!("export HTTP_PROXY=\"{v}\"");
    }
    if let Some(v) = &saved.https_proxy_upper {
        println!("export HTTPS_PROXY=\"{v}\"");
    }
    if let Some(v) = &saved.no_proxy_upper {
        println!("export NO_PROXY=\"{v}\"");
    }
    let _ = fs::remove_file(&path);
    println!("echo \"✓ 代理已恢复（快照已清除）\"");

    Ok(EXIT_OK)
}

/// `pproxy env generate-script`：生成应急 shell 脚本到指定路径。
pub fn env_generate_script(output: Option<&Path>) -> Result<i32, String> {
    let out_path = match output {
        Some(p) => p.to_path_buf(),
        None => default_script_path()?,
    };

    if let Some(dir) = out_path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(&out_path, MITIGATE_SCRIPT).map_err(|e| format!("写入脚本失败: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(0o755));
    }

    println!("✓ 应急脚本已生成: {}", out_path.display());
    println!();
    println!("用法:");
    println!("  source {} on   # 临时关闭代理（供兄弟项目启动）", out_path.display());
    println!("  source {} off  # 恢复代理", out_path.display());
    println!();
    println!("或将以下别名加入 ~/.bashrc / ~/.zshrc：");
    println!("  alias pproxy-mitigate-on='source {} on'", out_path.display());
    println!("  alias pproxy-mitigate-off='source {} off'", out_path.display());

    Ok(EXIT_OK)
}

// ─── 测试 ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 序列化修改 HOME 的测试，避免并行竞争。
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 在 HOME_LOCK 保护下设置 HOME 并执行闭包，完成后恢复原值。
    fn with_home<F: FnOnce() -> R, R>(tmp_dir: &Path, f: F) -> R {
        let _lock = HOME_LOCK.lock().unwrap();
        let orig = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp_dir);
        let result = f();
        match orig {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    #[test]
    fn proxy_env_path_uses_home() {
        let home = std::env::var("HOME").unwrap();
        let path = proxy_env_path().unwrap();
        assert!(path.starts_with(&home));
        assert!(path.to_string_lossy().contains(".pony/proxy.env"));
    }

    #[test]
    fn default_no_proxy_contains_private_ranges() {
        assert!(DEFAULT_NO_PROXY.contains("10.0.0.0/8"));
        assert!(DEFAULT_NO_PROXY.contains("172.16.0.0/12"));
        assert!(DEFAULT_NO_PROXY.contains("192.168.0.0/16"));
        assert!(DEFAULT_NO_PROXY.contains("100.64.0.0/10"));
        assert!(DEFAULT_NO_PROXY.contains(".svc"));
        assert!(DEFAULT_NO_PROXY.contains(".local"));
    }

    #[test]
    fn proxy_env_file_content_structure() {
        let k8s_entries: Vec<String> = vec![];
        let extra_no_proxy = if k8s_entries.is_empty() {
            String::new()
        } else {
            format!(",{}", k8s_entries.join(","))
        };
        let no_proxy = format!("{}{}", DEFAULT_NO_PROXY, extra_no_proxy);
        let content = format!(
            r#"# Pony Proxy — 环境代理配置
# 由 `pproxy on` 生成，`pproxy off` 清除
# 加载方式: source {}
export http_proxy="http://127.0.0.1:8899"
export https_proxy="http://127.0.0.1:8899"
export no_proxy="{no_proxy}"
"#,
            "~/.pony/proxy.env"
        );
        assert!(content.contains("export http_proxy="));
        assert!(content.contains("export https_proxy="));
        assert!(content.contains("export no_proxy="));
        assert!(content.contains("http://127.0.0.1:8899"));
        assert!(content.contains("10.0.0.0/8"));
        assert!(content.contains(".svc.cluster.local"));
    }

    #[test]
    fn toggle_enable_creates_file() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        with_home(tmp.path(), || {
            let result = enable_proxy();
            assert!(result.is_ok());
            let path = tmp.path().join(".pony").join("proxy.env");
            assert!(path.exists(), "proxy.env 应被创建");
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(content.contains("export http_proxy="));
            assert!(content.contains("export https_proxy="));
        });
    }

    #[test]
    fn toggle_disable_writes_unset_instead_of_removing() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let enable_path = tmp.path().join(".pony").join("proxy.env");
        fs::create_dir_all(enable_path.parent().unwrap()).unwrap();
        fs::write(&enable_path, "export http_proxy=\"http://x\"").unwrap();

        with_home(tmp.path(), || {
            let result = disable_proxy();
            assert!(result.is_ok());
            assert!(enable_path.exists(), "proxy.env 应继续存在");
            let content = fs::read_to_string(&enable_path).unwrap();
            assert!(content.contains("unset http_proxy"));
            assert!(content.contains("unset https_proxy"));
        });
    }

    #[test]
    fn disable_when_not_exists_does_not_error() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        with_home(tmp.path(), || {
            let result = disable_proxy();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn saved_proxy_env_roundtrip() {
        let saved = SavedProxyEnv {
            http_proxy: Some("http://127.0.0.1:8899".into()),
            https_proxy: Some("http://127.0.0.1:8899".into()),
            no_proxy: Some("localhost,.local".into()),
            http_proxy_upper: None,
            https_proxy_upper: None,
            no_proxy_upper: None,
        };
        let json = serde_json::to_string_pretty(&saved).unwrap();
        let restored: SavedProxyEnv = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.http_proxy, saved.http_proxy);
        assert_eq!(restored.https_proxy, saved.https_proxy);
        assert_eq!(restored.no_proxy, saved.no_proxy);
        assert!(restored.http_proxy_upper.is_none());
    }

    #[test]
    fn mitigate_script_contains_on_off_logic() {
        assert!(MITIGATE_SCRIPT.contains("on)"));
        assert!(MITIGATE_SCRIPT.contains("off)"));
        assert!(MITIGATE_SCRIPT.contains("unset http_proxy"));
        assert!(MITIGATE_SCRIPT.contains("source \"$PPROXY_SAVED_FILE\""));
    }

    #[test]
    fn detect_k8s_parses_example_kubeconfig() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let kubeconfig = tmp.path().join("config");
        fs::write(
            &kubeconfig,
            r#"apiVersion: v1
clusters:
- cluster:
    server: https://175.24.73.251:6443
  name: k3s
- cluster:
    server: https://192.168.1.100:6443
  name: local
contexts:
- context:
    cluster: k3s
  name: k3s
"#,
        )
        .unwrap();
        let orig_kubeconfig = std::env::var("KUBECONFIG").ok();
        std::env::set_var("KUBECONFIG", kubeconfig.to_str().unwrap());
        let entries = detect_k8s_cluster_entries();
        if let Some(h) = orig_kubeconfig {
            std::env::set_var("KUBECONFIG", h);
        } else {
            std::env::remove_var("KUBECONFIG");
        }
        assert!(entries.contains(&"175.24.73.251".to_string()));
        assert!(entries.contains(&"192.168.1.100".to_string()));
    }

    #[test]
    fn default_script_path_is_under_pony_dir() {
        let path = default_script_path().unwrap();
        assert!(path.to_string_lossy().contains(".pony/mitigate.sh"));
    }
}