//! 网关管理通道：撤销黑名单的热推送（口令 + 传输路径）。
//!
//! 生产网关只监听 `127.0.0.1:3101`（`HOST=127.0.0.1`），管理机无法直连，因此通道分两个维度：
//! - **口令**：环境变量 `GATE_ADMIN_TOKEN` → `~/.pony/gate_admin.conf` 的 `token=`
//! - **路径**：`ssh=<host>`（经既有运维 SSH 通道在网关本机 curl）→ 否则 `url=`/`GATE_ADMIN` 直连
//!
//! 口令只经 stdin 传给远端 `curl -H`，不出现在任何进程命令行里（避免 `ps` / `/proc` 窥屏）。
//!
//! 本模块只做「把某标识符塞进网关黑名单」一件事，不判断该撤销谁——语义留给调用方
//! （`user rm` / `user disable` / `user revoke`），便于单测。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// 远端执行体：第一行读口令、其余读 JSON body，最后回显 HTTP 状态码。
const SSH_REMOTE_CMD: &str = r#"read -r tok; body=$(cat); curl -sS --max-time 5 -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:3101/api/user/revoke -H "Authorization: Bearer $tok" -H 'Content-Type: application/json' -d "$body""#;

/// 默认直连地址（网关与 CLI 同机时的既有约定）。
const DEFAULT_ADMIN_URL: &str = "http://127.0.0.1:3101";

/// 管理通道配置（`~/.pony/gate_admin.conf` 的 key=value 口径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminConf {
    pub token: Option<String>,
    pub ssh: Option<String>,
    pub url: String,
}

impl Default for AdminConf {
    fn default() -> Self {
        Self {
            token: None,
            ssh: None,
            url: DEFAULT_ADMIN_URL.to_string(),
        }
    }
}

/// 解析 `gate_admin.conf`：`#` 注释与空行忽略、值两端引号剥除、未知键忽略、空值视同未配置。
pub fn parse_conf(text: &str) -> AdminConf {
    let mut conf = AdminConf::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "token" => conf.token = Some(value.to_string()),
            "ssh" => conf.ssh = Some(value.to_string()),
            "url" => conf.url = value.to_string(),
            _ => {}
        }
    }
    conf
}

/// 配置文件路径：`${HOME}/.pony/gate_admin.conf`。
pub fn conf_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
    Ok(std::path::Path::new(&home)
        .join(".pony")
        .join("gate_admin.conf"))
}

/// 读取配置：环境变量优先（`GATE_ADMIN_TOKEN` / `GATE_ADMIN_SSH` / `GATE_ADMIN`），文件兜底。
pub fn load_conf() -> AdminConf {
    let mut conf = conf_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| parse_conf(&text))
        .unwrap_or_default();

    if let Ok(token) = std::env::var("GATE_ADMIN_TOKEN") {
        if !token.trim().is_empty() {
            conf.token = Some(token.trim().to_string());
        }
    }
    if let Ok(ssh) = std::env::var("GATE_ADMIN_SSH") {
        if !ssh.trim().is_empty() {
            conf.ssh = Some(ssh.trim().to_string());
        }
    }
    if let Ok(url) = std::env::var("GATE_ADMIN") {
        if !url.trim().is_empty() {
            conf.url = url.trim().to_string();
        }
    }
    conf
}

/// 撤销标识符白名单：`jti`（`tok-<hex>`）与 `sub`（`usr_<name>`）都只用这些字符。
/// 拒绝其余字符是纵深防御——标识符会进入远端 shell 的 JSON body。
pub fn is_valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 一次推送的结果（供调用方打印与决定退出码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    pub ok: bool,
    /// 实际使用的通道描述，如 `ssh:racknerd` / `http://127.0.0.1:3101`
    pub channel: String,
    /// 失败原因或成功备注
    pub detail: String,
}

impl PushReport {
    fn ok(channel: String, detail: &str) -> Self {
        Self {
            ok: true,
            channel,
            detail: detail.to_string(),
        }
    }

    fn failed(channel: String, detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            channel,
            detail: detail.into(),
        }
    }
}

/// 把标识符推送进网关内存黑名单（`POST /api/user/revoke`）。
///
/// 未配置口令时返回 `ok=false`（调用方据此提示「仅本地落盘，需人工收口」），
/// 而非静默成功——撤销失败必须是可观测的。
pub fn push_revoke(identifier: &str) -> PushReport {
    let conf = load_conf();
    let Some(token) = conf.token.as_deref() else {
        return PushReport::failed(
            "none".to_string(),
            "未配置管理口令（GATE_ADMIN_TOKEN 或 ~/.pony/gate_admin.conf 的 token=）",
        );
    };
    if !is_valid_identifier(identifier) {
        return PushReport::failed("none".to_string(), format!("非法撤销标识符: {identifier}"));
    }

    let body = serde_json::json!({ "identifier": identifier }).to_string();
    match conf.ssh.as_deref() {
        Some(host) => {
            let channel = format!("ssh:{host}");
            match push_via_ssh(host, token, &body) {
                Ok(code) if code == "200" => PushReport::ok(channel, "网关内存黑名单已更新"),
                Ok(code) => PushReport::failed(channel, format!("网关返回 HTTP {code}")),
                Err(e) => PushReport::failed(channel, e),
            }
        }
        None => {
            let channel = conf.url.clone();
            match push_via_http(&conf.url, token, &body) {
                Ok(status) if status.is_success() => PushReport::ok(channel, "网关内存黑名单已更新"),
                Ok(status) => PushReport::failed(channel, format!("网关返回 HTTP {status}")),
                Err(e) => PushReport::failed(channel, e),
            }
        }
    }
}

/// 直连推送（网关与 CLI 同机）。
fn push_via_http(url: &str, token: &str, body: &str) -> Result<reqwest::StatusCode, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败: {e}"))?;
    client
        .post(format!("{url}/api/user/revoke"))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .map(|r| r.status())
        .map_err(|e| format!("连接网关失败: {e}"))
}

/// 经 SSH 在网关本机推送：口令与 body 走 stdin，不进命令行。
fn push_via_ssh(host: &str, token: &str, body: &str) -> Result<String, String> {
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", host])
        .arg(SSH_REMOTE_CMD)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;

    {
        let stdin = child.stdin.as_mut().ok_or("ssh stdin 不可用")?;
        stdin
            .write_all(format!("{token}\n{body}").as_bytes())
            .map_err(|e| format!("写入 ssh stdin 失败: {e}"))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh 执行失败: {e}"))?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if code.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ssh 无输出（{}）", err.trim().lines().next().unwrap_or("未知错误")));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_conf_reads_keys_and_ignores_noise() {
        let conf = parse_conf(
            "# 网关管理通道\n\
             token=abc123\n\
             ssh = racknerd \n\
             url=\"http://10.0.0.1:3101\"\n\
             unknown=xxx\n\
             empty=\n",
        );
        assert_eq!(conf.token.as_deref(), Some("abc123"));
        assert_eq!(conf.ssh.as_deref(), Some("racknerd"));
        assert_eq!(conf.url, "http://10.0.0.1:3101");
    }

    #[test]
    fn parse_conf_defaults_to_localhost() {
        let conf = parse_conf("");
        assert_eq!(conf, AdminConf::default());
        assert_eq!(conf.url, DEFAULT_ADMIN_URL);
        assert!(conf.token.is_none() && conf.ssh.is_none());
    }

    #[test]
    fn identifier_whitelist_blocks_shell_metacharacters() {
        assert!(is_valid_identifier("tok-0123456789abcdef"));
        assert!(is_valid_identifier("usr_alice"));
        assert!(is_valid_identifier("usr_a.b"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("usr_alice'; rm -rf /"));
        assert!(!is_valid_identifier("usr_alice\nbody"));
        assert!(!is_valid_identifier(&"a".repeat(129)));
    }
}
