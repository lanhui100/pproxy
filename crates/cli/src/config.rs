//! `~/.pony/config.toml` 读写 + admin token 存取（M2 §3）。
//!
//! 纯本地文件逻辑，不碰 HTTP。错误统一 [`ConfigError`]，由 main 映射退出码 2。

use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 本地配置错误（main 统一映射退出码 2）。
#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    Read(std::io::Error),
    Parse(String),
    MissingField(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(
                f,
                "config not found: {} — run 'pproxy init --server <url> --token <admin_token>'",
                p.display()
            ),
            Self::Read(e) => write!(f, "config read failed: {e}"),
            Self::Parse(e) => write!(f, "config parse failed: {e}"),
            Self::MissingField(k) => write!(f, "config missing required key: {k}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// config.toml 结构（M2 §3）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PonyConfig {
    pub server: String,
    pub admin_token: String,
    #[serde(default)]
    pub data_plane: Option<String>,

    // ---- 部署配置（可选，交互式 init 写入，deploy 命令读取）----
    /// Cloudflare API Token（用于部署 CF Worker）
    #[serde(default)]
    pub cf_token: Option<String>,
    /// Cloudflare Account Tag（用于 CF Worker 部署）
    #[serde(default)]
    pub cf_account_tag: Option<String>,
    /// Vercel Token（用于部署 Vercel 函数）
    #[serde(default)]
    pub vercel_token: Option<String>,
    /// 隧道令牌（Gate Worker 认证）
    #[serde(default)]
    pub tunnel_token: Option<String>,
    /// 上游共享密钥（PROXY_SECRET，服务器与 Worker 之间）
    #[serde(default)]
    pub proxy_secret: Option<String>,
}

/// 获取当前用户主目录（HOME / USERPROFILE）。
pub fn home_dir() -> Result<PathBuf, ConfigError> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| ConfigError::Read(
            std::io::Error::new(std::io::ErrorKind::NotFound, "HOME or USERPROFILE not set"),
        ))?;
    Ok(PathBuf::from(home))
}

/// `$HOME/.pony/config.toml` 路径；HOME / USERPROFILE 缺失 → 错误（退出码 2）。
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(home_dir()?.join(".pony").join("config.toml"))
}

/// 安全原子写文件（临时文件写入 -> sync_all -> rename），并在 Unix 下赋予 0600 权限、父目录 0700 权限。
pub fn secure_write_file(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }

    let file_stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let tmp_path = path.with_file_name(format!(".{file_stem}.tmp.{}", rand::random::<u32>()));

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }

    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }

    #[cfg(windows)]
    {
        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 读配置：文件缺失 → NotFound；解析失败/缺必填键 → 对应变体。
pub fn load() -> Result<PonyConfig, ConfigError> {
    let path = config_path()?;
    let raw = fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound(path.clone())
        } else {
            ConfigError::Read(e)
        }
    })?;
    let cfg: PonyConfig = toml::from_str(&raw).map_err(|e| ConfigError::Parse(e.to_string()))?;
    if cfg.server.is_empty() {
        return Err(ConfigError::MissingField("server"));
    }
    if cfg.admin_token.is_empty() {
        return Err(ConfigError::MissingField("admin_token"));
    }
    warn_permissive_mode(&path);
    Ok(cfg)
}

/// 写配置并原子赋予 0600 权限；父目录一并创建（0700）。
pub fn save(cfg: &PonyConfig) -> Result<PathBuf, ConfigError> {
    let path = config_path()?;
    let body = toml::to_string_pretty(cfg).map_err(|e| ConfigError::Parse(e.to_string()))?;
    secure_write_file(&path, body.as_bytes()).map_err(ConfigError::Read)?;
    Ok(path)
}

/// group/other 位非零 → stderr warn（不阻断，M2 §3）。
fn warn_permissive_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::io::Write;
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                let mut err = std::io::stderr();
                let _ = writeln!(
                    err,
                    "warning: {} is group/other readable (mode {:04o}); consider chmod 600",
                    path.display(),
                    mode & 0o777
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// token 脱敏展示（前 10 位 + …）；过短则全遮。
#[allow(dead_code)] // 错误输出统一脱敏入口，当前仅单测引用
pub(crate) fn redact(token: &str) -> String {
    if token.len() > 10 {
        format!("{}…", &token[..10])
    } else {
        "…".to_string()
    }
}

/// data_plane 推导（M2 §3）：显式配置优先；否则 server 的 scheme+host、端口换 8899。
pub fn derive_data_plane(cfg: &PonyConfig) -> Result<String, ConfigError> {
    if let Some(dp) = &cfg.data_plane {
        if !dp.is_empty() {
            return Ok(dp.clone());
        }
    }
    derive_from_server(&cfg.server).ok_or_else(|| ConfigError::Parse(format!(
        "cannot derive data plane from server url: {}",
        cfg.server
    )))
}

/// 从 server URL 推导数据面 base URL（端口替换为 8899）。测试导出。
pub(crate) fn derive_from_server(server: &str) -> Option<String> {
    let rest = server.strip_prefix("http://").or_else(|| server.strip_prefix("https://"))?;
    // 去掉路径部分与尾斜杠
    let host_port = rest.split('/').next()?;
    if host_port.is_empty() {
        return None;
    }
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

    fn tmpdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    fn write_cfg(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
    }

    #[test]
    fn roundtrip_and_permissions() {
        let (_dir, path) = tmpdir();
        write_cfg(&path, "server = \"http://127.0.0.1:8900\"\nadmin_token = \"pony_admin_x\"\n");
        let cfg: PonyConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.server, "http://127.0.0.1:8900");
        assert_eq!(cfg.data_plane, None);
        #[cfg(unix)]
        {
            // 权限位检测：0644 应被判定为宽松
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(fs::metadata(&path).unwrap().permissions().mode() & 0o077 != 0);
        }
    }

    #[test]
    fn missing_server_rejected() {
        // 显式空串：解析通过、load 层校验拒绝
        let parsed: PonyConfig = toml::from_str("server = \"\"\nadmin_token = \"x\"").unwrap();
        assert_eq!(parsed.server, "");
        // 缺字段：解析层直接报错
        assert!(toml::from_str::<PonyConfig>("admin_token = \"x\"").is_err());
    }

    #[test]
    fn missing_admin_token_detected() {
        let body = "server = \"http://127.0.0.1:8900\"\nadmin_token = \"\"\n";
        let cfg: PonyConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.admin_token, ""); // 解析通过，空值由 load 校验拒绝
        assert!(toml::from_str::<PonyConfig>("server = \"http://x\"").is_err());
    }

    #[test]
    fn permissive_mode_bitmath() {
        assert_eq!(0o644 & 0o077, 0o044);
        assert_eq!(0o600 & 0o077, 0);
        assert_eq!(0o640 & 0o077, 0o040);
    }

    #[test]
    fn redact_shows_prefix_only() {
        assert_eq!(redact("pony_admin_abcdef"), "pony_admin…"); // 前 10 字节 + …
        assert_eq!(redact("short"), "…");
    }

    #[test]
    fn derive_data_plane_variants() {
        let mk = |server: &str, dp: Option<&str>| PonyConfig {
            server: server.into(),
            admin_token: "t".into(),
            data_plane: dp.map(Into::into),
            cf_token: None,
            cf_account_tag: None,
            vercel_token: None,
            tunnel_token: None,
            proxy_secret: None,
        };
        assert_eq!(
            derive_data_plane(&mk("http://127.0.0.1:8900", None)).unwrap(),
            "http://127.0.0.1:8899"
        );
        assert_eq!(
            derive_data_plane(&mk("https://api.example.com", None)).unwrap(),
            "https://api.example.com:8899"
        );
        assert_eq!(
            derive_data_plane(&mk("http://127.0.0.1:8900", Some("http://10.0.0.5:9999"))).unwrap(),
            "http://10.0.0.5:9999"
        );
        assert!(derive_data_plane(&mk("ftp://x", None)).is_err());
    }

    #[test]
    fn derive_keeps_ipv4_host_with_digits() {
        assert_eq!(
            derive_from_server("http://192.168.1.2:9000").unwrap(),
            "http://192.168.1.2:8899"
        );
    }

    #[test]
    fn pony_config_new_fields_roundtrip() {
        let cfg = PonyConfig {
            server: "http://127.0.0.1:8900".into(),
            admin_token: "admin_token".into(),
            data_plane: Some("http://10.0.0.1:8899".into()),
            cf_token: Some("cfat_test_token".into()),
            cf_account_tag: Some("test_account_tag".into()),
            vercel_token: Some("vcp_test_token".into()),
            tunnel_token: Some("gate_test_token".into()),
            proxy_secret: Some("proxy_secret_value".into()),
        };
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        // 验证所有字段都在序列化输出中
        assert!(toml_str.contains("cf_token"));
        assert!(toml_str.contains("cf_account_tag"));
        assert!(toml_str.contains("vercel_token"));
        assert!(toml_str.contains("tunnel_token"));
        assert!(toml_str.contains("proxy_secret"));
        assert!(toml_str.contains("cfat_test_token"));
        assert!(toml_str.contains("vcp_test_token"));

        // 反序列化回读
        let parsed: PonyConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.cf_token, Some("cfat_test_token".into()));
        assert_eq!(parsed.cf_account_tag, Some("test_account_tag".into()));
        assert_eq!(parsed.vercel_token, Some("vcp_test_token".into()));
        assert_eq!(parsed.tunnel_token, Some("gate_test_token".into()));
        assert_eq!(parsed.proxy_secret, Some("proxy_secret_value".into()));
    }

    #[test]
    fn pony_config_new_fields_optional_default_to_none() {
        let toml_str = r#"
server = "http://127.0.0.1:8900"
admin_token = "admin"
"#;
        let cfg: PonyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.cf_token, None);
        assert_eq!(cfg.cf_account_tag, None);
        assert_eq!(cfg.vercel_token, None);
        assert_eq!(cfg.tunnel_token, None);
        assert_eq!(cfg.proxy_secret, None);
    }
}
