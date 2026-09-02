//! `pproxy upgrade` (或 `pproxy update`) — CLI 自动检测与原地自升级命令。
//!
//! 支持版本比对 (Semver)、三平台架构推导、双通道极速下载 (CDN 镜像 + GitHub Release) 与安全自替换 (Self-Replace)。

use std::path::PathBuf;
use std::time::Duration;
use serde::Deserialize;

use crate::EXIT_OK;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_REPO: &str = "lanhui100/pproxy";
pub const DEFAULT_GATEWAY_DIST: &str = "https://access.ponyjob.top/dsk";
pub const DEFAULT_FALLBACK_DIST: &str = "https://dl.ponyjob.top";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GitHubReleaseAsset>,
}

/// 清洗版本字符串，去除 cli-v, desktop-v, v 等前缀
pub fn clean_version_str(tag_or_ver: &str) -> &str {
    let s = tag_or_ver.trim();
    if let Some(rest) = s.strip_prefix("cli-v") {
        rest
    } else if let Some(rest) = s.strip_prefix("desktop-v") {
        rest
    } else if let Some(rest) = s.strip_prefix('v') {
        rest
    } else {
        s
    }
}

/// 解析语义化版本为 (major, minor, patch)
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let clean = clean_version_str(s);
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    let patch_str = parts[2].split('-').next().unwrap_or(parts[2]);
    let patch = patch_str.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// 判断 target 版本是否严格高于 current 版本
pub fn is_newer_version(current: &str, target: &str) -> bool {
    match (parse_semver(current), parse_semver(target)) {
        (Some(cur), Some(tgt)) => tgt > cur,
        _ => clean_version_str(target) != clean_version_str(current),
    }
}

/// 根据编译与运行环境推导当前系统的 Release 二进制产物名称
pub fn detect_target_binary() -> Result<&'static str, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("linux", "x86_64") => Ok("pproxy-linux-amd64"),
        ("linux", "aarch64") => Ok("pproxy-linux-arm64"),
        ("macos", "x86_64") => Ok("pproxy-macos-amd64"),
        ("macos", "aarch64") => Ok("pproxy-macos-arm64"),
        ("windows", "x86_64") => Ok("pproxy-windows-x64.exe"),
        _ => Err(format!("当前系统/架构不受直接二进制升级支持: {os} ({arch})")),
    }
}

/// 获取远端最新 Release 信息（带双通道探测）
pub fn fetch_latest_release(client: &reqwest::blocking::Client) -> Result<GitHubRelease, String> {
    // 1. 优先从官方网关 /dsk/latest.json 获取最新版本清单（免 GitHub 登录与私有权限限制，速度快且稳定）
    let dist_bases = [
        std::env::var("PONY_DIST_URL").ok(),
        Some(DEFAULT_GATEWAY_DIST.to_string()),
        Some(DEFAULT_FALLBACK_DIST.to_string()),
    ];

    #[derive(Deserialize)]
    struct LatestManifest {
        version: String,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        pub_date: Option<String>,
    }

    for dist_opt in dist_bases.into_iter().flatten() {
        let manifest_url = format!("{}/latest.json", dist_opt.trim_end_matches('/'));
        if let Ok(resp) = client
            .get(&manifest_url)
            .header("User-Agent", format!("pproxy-cli/{CURRENT_VERSION}"))
            .timeout(Duration::from_secs(6))
            .send()
        {
            if resp.status().is_success() {
                if let Ok(manifest) = resp.json::<LatestManifest>() {
                    let tag = format!("desktop-v{}", manifest.version);
                    return Ok(GitHubRelease {
                        tag_name: tag,
                        name: Some(format!("Pony Proxy v{}", manifest.version)),
                        body: manifest.notes,
                        published_at: manifest.pub_date,
                        assets: vec![],
                    });
                }
            }
        }
    }

    // 2. 备用通道：尝试 GitHub Releases 列表 API（支持读取 GITHUB_TOKEN / PONY_GITHUB_TOKEN）
    let api_url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases?per_page=1");
    let mut req = client
        .get(&api_url)
        .header("User-Agent", format!("pproxy-cli/{CURRENT_VERSION}"))
        .header("Accept", "application/vnd.github.v3+json")
        .timeout(Duration::from_secs(8));

    if let Ok(token) = std::env::var("PONY_GITHUB_TOKEN").or_else(|_| std::env::var("GITHUB_TOKEN")) {
        if !token.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token.trim()));
        }
    }

    match req.send() {
        Ok(r) if r.status().is_success() => {
            let list: Vec<GitHubRelease> = r.json().map_err(|e| format!("解析 GitHub Release 列表失败: {e}"))?;
            list.into_iter().next().ok_or_else(|| "GitHub Release 列表为空".to_string())
        }
        _ => Err("检测更新失败: 无法连接分发网关及 GitHub 备用源".to_string()),
    }
}

/// 执行跨平台可执行文件就地安全原子替换
pub fn replace_current_executable(new_bytes: &[u8]) -> Result<PathBuf, String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("无法定位当前运行的可执行文件路径: {e}"))?;
    
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| "无法获取当前程序所在目录".to_string())?;

    let tmp_path = exe_dir.join(format!(".pproxy-upgrade-{}.tmp", std::process::id()));
    
    // 写入新二进制到同一目录的临时文件（确保处于同一文件系统分区，rename 可原子完成）
    std::fs::write(&tmp_path, new_bytes)
        .map_err(|e| format!("写入临时更新文件失败 (请检查目录写入权限): {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp_path)
            .map_err(|e| format!("读取临时文件属性失败: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        if let Err(e) = std::fs::set_permissions(&tmp_path, perms) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("赋予执行权限失败: {e}"));
        }

        if let Err(e) = std::fs::rename(&tmp_path, &current_exe) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("替换可执行文件失败 (如位于系统目录请尝试 sudo pproxy upgrade): {e}"));
        }
    }

    #[cfg(windows)]
    {
        let old_path = exe_dir.join(format!(".pproxy-upgrade-{}.old", std::process::id()));
        // Windows 运行中的 exe 可以被 rename，但不能被直接 write / overwrite
        if let Err(e) = std::fs::rename(&current_exe, &old_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("重命名旧程序失败 (可能需要管理员权限): {e}"));
        }

        if let Err(e) = std::fs::rename(&tmp_path, &current_exe) {
            // 发生错误尝试还原
            let _ = std::fs::rename(&old_path, &current_exe);
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("移动新程序至目标位置失败，已尝试恢复旧版本: {e}"));
        }

        // 尝试删除 old 文件（删除失败不影响升级结果）
        let _ = std::fs::remove_file(&old_path);
    }

    Ok(current_exe)
}

/// 执行下载并返回二进制内容
fn download_binary(
    client: &reqwest::blocking::Client,
    binary_name: &str,
    target_tag: &str,
    custom_mirror: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut candidate_urls = Vec::new();

    if let Some(mirror) = custom_mirror {
        let base = mirror.trim_end_matches('/');
        candidate_urls.push(format!("{base}/{binary_name}"));
    } else {
        if let Ok(dist) = std::env::var("PONY_DIST_URL") {
            let base = dist.trim_end_matches('/');
            candidate_urls.push(format!("{base}/{binary_name}"));
        }
        // 1. 优先官方网关分发源
        candidate_urls.push(format!("{DEFAULT_GATEWAY_DIST}/{binary_name}"));
        // 2. 备用分发源
        candidate_urls.push(format!("{DEFAULT_FALLBACK_DIST}/{binary_name}"));
        // 3. 备用 GitHub Release 链接
        candidate_urls.push(format!(
            "https://github.com/{GITHUB_REPO}/releases/download/{target_tag}/{binary_name}"
        ));
        // 4. 备用 GitHub latest 链接
        candidate_urls.push(format!(
            "https://github.com/{GITHUB_REPO}/releases/latest/download/{binary_name}"
        ));
    }

    let auth_token = std::env::var("PONY_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut last_err = String::new();
    for url in &candidate_urls {
        print!("  尝试从源下载: \x1b[34m{url}\x1b[0m ... ");
        use std::io::Write as _;
        let _ = std::io::stdout().flush();

        let mut req = client
            .get(url)
            .header("User-Agent", format!("pproxy-cli/{CURRENT_VERSION}"))
            .timeout(Duration::from_secs(60));

        if url.contains("github.com") {
            if let Some(token) = &auth_token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
        }

        let resp = req.send();

        match resp {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().map_err(|e| format!("读取下载数据失败: {e}"))?;
                if bytes.len() < 512 * 1024 {
                    // 文件过小，可能是错误页面
                    println!("\x1b[31m异常 (产物过小)\x1b[0m");
                    last_err = format!("下载产物大小异常 ({} bytes)", bytes.len());
                    continue;
                }
                println!("\x1b[32m成功 ({:.2} MB)\x1b[0m", bytes.len() as f64 / (1024.0 * 1024.0));
                return Ok(bytes.to_vec());
            }
            Ok(r) => {
                println!("\x1b[33mHTTP {}\x1b[0m", r.status());
                last_err = format!("HTTP {}", r.status());
            }
            Err(e) => {
                println!("\x1b[31m连接失败\x1b[0m");
                last_err = e.to_string();
            }
        }
    }

    Err(format!("所有候选下载源均失败: {last_err}"))
}

/// 检查是否有活跃的后台 systemd 代理服务
fn check_running_service_notice() {
    #[cfg(unix)]
    {
        let is_active = std::process::Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "pproxy-server"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if is_active {
            println!();
            println!("\x1b[1;36m💡 提示: 检测到后台用户守护进程 pproxy-server 正在运行。\x1b[0m");
            println!("  建议运行: \x1b[32mpproxy restart\x1b[0m (或 systemctl --user restart pproxy-server) 以生效新版本。");
        }
    }
}

/// upgrade 命令入口
pub fn run(
    check_only: bool,
    force: bool,
    target_version: Option<&str>,
    mirror: Option<&str>,
) -> Result<i32, String> {
    let binary_name = detect_target_binary()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败: {e}"))?;

    println!("\x1b[1;34m=== Pony Proxy CLI 升级管理 ===\x1b[0m");
    println!("  当前版本: \x1b[1m{}\x1b[0m", CURRENT_VERSION);
    println!("  系统架构: \x1b[1m{}\x1b[0m ({})", std::env::consts::OS, std::env::consts::ARCH);
    println!("  匹配产物: \x1b[36m{}\x1b[0m", binary_name);
    println!();

    let (latest_ver_clean, target_tag) = if let Some(ver) = target_version {
        let clean = clean_version_str(ver).to_string();
        let tag = if ver.starts_with('v') || ver.starts_with("cli-v") {
            ver.to_string()
        } else {
            format!("v{clean}")
        };
        (clean, tag)
    } else {
        print!("正在检查远端最新版本... ");
        use std::io::Write as _;
        let _ = std::io::stdout().flush();

        let release = fetch_latest_release(&client)?;
        let clean = clean_version_str(&release.tag_name).to_string();
        println!("\x1b[32m{}\x1b[0m (Tag: {})", clean, release.tag_name);

        if let Some(body) = release.body.as_deref().filter(|b| !b.is_empty()) {
            println!();
            println!("\x1b[1m[更新日志]\x1b[0m");
            for line in body.lines().take(10) {
                println!("  {line}");
            }
            if body.lines().count() > 10 {
                println!("  ...");
            }
            println!();
        }

        (clean, release.tag_name)
    };

    let has_update = is_newer_version(CURRENT_VERSION, &latest_ver_clean);

    if check_only {
        if has_update {
            println!("\x1b[1;32m✓ 发现新版本可用: {} -> {}\x1b[0m", CURRENT_VERSION, latest_ver_clean);
            println!("  运行 \x1b[1mpproxy upgrade\x1b[0m 即可执行一键自动更新。");
        } else {
            println!("\x1b[32m✓ 当前已是最新版本 ({})\x1b[0m", CURRENT_VERSION);
        }
        return Ok(EXIT_OK);
    }

    if !has_update && !force && target_version.is_none() {
        println!("\x1b[32m✓ 当前已是最新版本 ({})，无需升级。\x1b[0m", CURRENT_VERSION);
        println!("  提示: 可使用 \x1b[1mpproxy upgrade --force\x1b[0m 强制重新下载覆盖。");
        return Ok(EXIT_OK);
    }

    println!("\x1b[1m开始下载并更新...\x1b[0m");
    let bytes = download_binary(&client, binary_name, &target_tag, mirror)?;

    print!("正在替换可执行文件... ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    let exe_path = replace_current_executable(&bytes)?;
    println!("\x1b[32m完成\x1b[0m");

    println!();
    println!("\x1b[1;32m════════════════════════════════════════════════════════════════\x1b[0m");
    println!("\x1b[1;32m🎉 升级成功！当前版本: v{}\x1b[0m", latest_ver_clean);
    println!("  路径: {}", exe_path.display());
    println!("\x1b[1;32m════════════════════════════════════════════════════════════════\x1b[0m");

    check_running_service_notice();

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_version_str() {
        assert_eq!(clean_version_str("v0.3.26"), "0.3.26");
        assert_eq!(clean_version_str("cli-v0.3.26"), "0.3.26");
        assert_eq!(clean_version_str("desktop-v0.3.26"), "0.3.26");
        assert_eq!(clean_version_str("0.3.26"), "0.3.26");
        assert_eq!(clean_version_str("  v1.2.3  "), "1.2.3");
    }

    #[test]
    fn test_semver_parse_and_compare() {
        assert_eq!(parse_semver("0.3.25"), Some((0, 3, 25)));
        assert_eq!(parse_semver("v0.3.26"), Some((0, 3, 26)));
        assert_eq!(parse_semver("cli-v1.0.0-rc1"), Some((1, 0, 0)));
        assert_eq!(parse_semver("invalid"), None);

        assert!(is_newer_version("0.3.25", "0.3.26"));
        assert!(is_newer_version("0.3.25", "0.4.0"));
        assert!(is_newer_version("0.3.25", "1.0.0"));
        assert!(!is_newer_version("0.3.26", "0.3.26"));
        assert!(!is_newer_version("0.3.26", "0.3.25"));
        assert!(!is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_detect_target_binary() {
        let res = detect_target_binary();
        assert!(res.is_ok(), "Target binary should be recognized on build machine: {:?}", res);
        let name = res.unwrap();
        assert!(name.starts_with("pproxy-"));
    }

    #[test]
    fn test_github_release_json_parse() {
        let json_data = r#"{
            "tag_name": "cli-v0.3.26",
            "name": "Pony Proxy Release",
            "body": "Bugfixes and performance improvements",
            "assets": [
                {
                    "name": "pproxy-linux-amd64",
                    "browser_download_url": "https://github.com/lanhui100/pproxy/releases/download/cli-v0.3.26/pproxy-linux-amd64",
                    "size": 1234567
                }
            ]
        }"#;

        let parsed: Result<GitHubRelease, _> = serde_json::from_str(json_data);
        assert!(parsed.is_ok());
        let release = parsed.unwrap();
        assert_eq!(release.tag_name, "cli-v0.3.26");
        assert_eq!(clean_version_str(&release.tag_name), "0.3.26");
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].name, "pproxy-linux-amd64");
    }

    #[test]
    fn test_replace_executable_simulation() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let old_exe = tmp_dir.path().join("mock_pproxy");
        std::fs::write(&old_exe, b"old_binary_data").unwrap();

        let new_bytes = b"new_binary_data_v2";
        let tmp_write = tmp_dir.path().join("mock_pproxy.tmp");
        std::fs::write(&tmp_write, new_bytes).unwrap();
        
        #[cfg(unix)]
        {
            std::fs::rename(&tmp_write, &old_exe).unwrap();
        }
        #[cfg(windows)]
        {
            let backup = tmp_dir.path().join("mock_pproxy.old");
            std::fs::rename(&old_exe, &backup).unwrap();
            std::fs::rename(&tmp_write, &old_exe).unwrap();
            let _ = std::fs::remove_file(&backup);
        }

        let updated_data = std::fs::read(&old_exe).unwrap();
        assert_eq!(updated_data, new_bytes);
    }

    #[test]
    fn test_gateway_manifest_parse() {
        let json_data = r#"{
            "version": "0.3.27",
            "notes": "Release v0.3.27 notes",
            "pub_date": "2026-09-02T08:56:21Z",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://access.ponyjob.top/dsk/Pony.Proxy_0.3.27_x64-setup.exe"
                }
            }
        }"#;

        #[derive(Deserialize)]
        struct Manifest {
            version: String,
            notes: Option<String>,
            pub_date: Option<String>,
        }

        let parsed: Result<Manifest, _> = serde_json::from_str(json_data);
        assert!(parsed.is_ok());
        let m = parsed.unwrap();
        assert_eq!(m.version, "0.3.27");
        assert_eq!(m.notes.as_deref(), Some("Release v0.3.27 notes"));
        assert_eq!(m.pub_date.as_deref(), Some("2026-09-02T08:56:21Z"));
    }
}
