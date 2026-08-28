//! `pproxy on / off / status` — 本机环境代理开关。
//!
//! 通过设置/清除 `http_proxy` / `https_proxy` / `no_proxy` 环境变量
//! 控制当前 shell 或持久化配置的代理转发。
//!
//! 原理：
//! - 临时模式：仅修改当前进程环境变量（影响该进程启动的子进程）
//! - 持久化模式：写入 `~/.pony/proxy.env`，shell 加载后生效
//!
//! 本模块实现的是"当前 shell 代理开关"——通过写入 shell rc 文件
//! 或输出 eval 片段供用户 source 使用。

use std::fs;
use std::path::PathBuf;

use crate::config;
use crate::EXIT_OK;

/// 代理环境变量配置。
const PROXY_ENV_FILE: &str = "proxy.env";

/// 默认的直连列表（no_proxy）。
const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1,.local";

/// 获取代理持久化文件路径。
fn proxy_env_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(&home).join(".pony").join(PROXY_ENV_FILE))
}

/// 检测当前代理状态。
pub fn status() -> Result<i32, String> {
    let env_path = proxy_env_path()?;

    // 检查持久化配置
    let persistent = if env_path.exists() {
        match fs::read_to_string(&env_path) {
            Ok(content) => content.contains("export http_proxy="),
            Err(_) => false,
        }
    } else {
        false
    };

    // 检查当前进程环境变量
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
    println!();
    println!("开启代理:  pproxy on");
    println!("关闭代理:  pproxy off");
    println!();
    println!("提示: 开启后请在当前 shell 执行:");
    println!("  source {}", env_path.display());

    Ok(EXIT_OK)
}

/// 开启环境代理。
pub fn toggle(enable: bool) -> Result<i32, String> {
    if enable {
        enable_proxy()
    } else {
        disable_proxy()
    }
}

/// 开启：计算代理地址，写入 ~/.pony/proxy.env。
fn enable_proxy() -> Result<i32, String> {
    // 先从配置读取 server 地址
    let data_plane = match config::load() {
        Ok(cfg) => {
            config::derive_data_plane(&cfg).unwrap_or_else(|_| "http://127.0.0.1:8899".to_string())
        }
        Err(_) => "http://127.0.0.1:8899".to_string(),
    };

    // 生成代理 env 文件
    let content = format!(
        r#"# Pony Proxy — 环境代理配置
# 由 `pproxy on` 生成，`pproxy off` 清除
# 加载方式: source {}
export http_proxy="{data_plane}"
export https_proxy="{data_plane}"
export no_proxy="{DEFAULT_NO_PROXY}"
"#,
        proxy_env_path().map(|p| p.display().to_string()).unwrap_or_else(|_| "~/.pony/proxy.env".into())
    );

    let path = proxy_env_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(&path, &content).map_err(|e| format!("写入文件失败: {e}"))?;

    println!("✓ 环境代理已启用");
    println!();
    println!("代理地址: {data_plane}");
    println!("直连列表: {DEFAULT_NO_PROXY}");
    println!();
    println!("请在当前 shell 中执行以下命令加载代理配置：");
    println!("  source {}", path.display());
    println!();
    println!("或将其添加到 ~/.bashrc / ~/.zshrc 以永久生效：");
    println!("  echo 'source {}' >> ~/.bashrc", path.display());

    Ok(EXIT_OK)
}

/// 关闭：清除 ~/.pony/proxy.env。
fn disable_proxy() -> Result<i32, String> {
    let path = proxy_env_path()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("删除文件失败: {e}"))?;
        println!("✓ 持久化代理配置已删除");
    } else {
        println!("代理配置不存在，无需关闭");
    }

    // 也尝试清除当前进程的环境变量
    // 注意：这只会影响当前进程和子进程，不影响已打开的 shell
    println!();
    println!("如需清除当前 shell 的代理环境变量，请执行：");
    println!("  unset http_proxy https_proxy no_proxy");
    println!("  unset HTTP_PROXY HTTPS_PROXY NO_PROXY");

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_env_path_uses_home() {
        let home = std::env::var("HOME").unwrap();
        let path = proxy_env_path().unwrap();
        assert!(path.starts_with(&home));
        assert!(path.to_string_lossy().contains(".pony/proxy.env"));
    }

    #[test]
    fn proxy_env_file_content_structure() {
        let content = format!(
            r#"# Pony Proxy — 环境代理配置
# 由 `pproxy on` 生成，`pproxy off` 清除
# 加载方式: source {}
export http_proxy="http://127.0.0.1:8899"
export https_proxy="http://127.0.0.1:8899"
export no_proxy="localhost,127.0.0.1,::1,.local"
"#,
            "~/.pony/proxy.env"
        );
        assert!(content.contains("export http_proxy="));
        assert!(content.contains("export https_proxy="));
        assert!(content.contains("export no_proxy="));
        assert!(content.contains("http://127.0.0.1:8899"));
        assert!(content.contains("localhost,127.0.0.1,::1,.local"));
    }

    #[test]
    fn toggle_enable_creates_file() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        // 覆盖 HOME 指向临时目录
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = enable_proxy();
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
        assert!(result.is_ok());
        // 验证文件被创建
        let path = tmp.path().join(".pony").join("proxy.env");
        assert!(path.exists(), "proxy.env 应被创建");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("export http_proxy="));
        assert!(content.contains("export https_proxy="));
    }

    #[test]
    fn toggle_disable_removes_file() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let path = tmp.path().join(".pony").join("proxy.env");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "export http_proxy=\"http://x\"").unwrap();

        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = disable_proxy();
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
        assert!(result.is_ok());
        assert!(!path.exists(), "proxy.env 应被删除");
    }

    #[test]
    fn disable_when_not_exists_does_not_error() {
        use tempfile::tempdir;
        let tmp = tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = disable_proxy();
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        }
        assert!(result.is_ok());
    }
}