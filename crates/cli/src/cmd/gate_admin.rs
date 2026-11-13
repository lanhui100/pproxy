//! 网关管理通道：管理端点调用（口令 + 传输路径）。
//!
//! 生产网关只监听 `127.0.0.1:3101`（`HOST=127.0.0.1`），管理机无法直连，因此通道分两个维度：
//! - **口令**：环境变量 `GATE_ADMIN_TOKEN` → `~/.pony/gate_admin.conf` 的 `token=`
//! - **路径**：`ssh=<host>`（经既有运维 SSH 通道在网关本机 curl）→ 否则 `url=`/`GATE_ADMIN` 直连
//!
//! 口令只经 stdin 传给远端 `curl -H`，不出现在任何进程命令行里（避免 `ps` / `/proc` 窥屏）。
//!
//! 本模块只做「按路径调用管理端点并回传状态码 + 响应体」一件事，不解释语义——
//! 撤销（`user rm`/`disable`/`revoke`）与用量查询/清零的语义留给调用方，便于单测。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// 远端执行体：第一行读口令、第二行读路径、其余读 JSON body。
/// 输出 = 响应体 + 末行 HTTP 状态码（`-w '\n%{http_code}'`），便于同时取两者。
const SSH_REMOTE_CMD: &str = r#"read -r tok; read -r path; body=$(cat); curl -sS --max-time 5 -w '\n%{http_code}' -X POST "http://127.0.0.1:3101$path" -H "Authorization: Bearer $tok" -H 'Content-Type: application/json' --data-binary "$body""#;

/// 默认直连地址（网关与 CLI 同机时的既有约定）。
const DEFAULT_ADMIN_URL: &str = "http://127.0.0.1:3101";

/// 一次管理端点调用的响应（状态码 + 响应体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminReply {
    pub status: u16,
    pub body: String,
}

impl AdminReply {
    /// 解析 `curl -w '\n%{http_code}'` 的输出：末行状态码，其余为响应体。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim_end_matches(['\n', '\r']);
        let (body, code) = match trimmed.rsplit_once('\n') {
            Some((body, code)) => (body, code),
            None => ("", trimmed),
        };
        let status = code
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("网关响应状态码不可解析: {}", code.trim()))?;
        Ok(Self {
            status,
            body: body.to_string(),
        })
    }
}

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

/// 令牌形态白名单：`usr_live_` + base64url（含 `-` `_`）+ `.` 段分隔。
/// 令牌会进入远端 shell 的 JSON body，因此仍做字符集收敛（纵深防御）。
pub fn is_valid_token_shape(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 4096
        && t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 为已签发令牌申请订阅短码（`POST /api/admin/shortlink`）。
///
/// 短码让订阅 URL 从 ~390 字节压到 ~42 字节，终端二维码因此从版本 13（71 列 × 35 行）
/// 降到版本 3（37 列 × 19 行）。管理通道未配置/不可达时返回 `Err`，
/// 调用方回退到长令牌 URL —— 功能不变，只是二维码更大。
pub fn request_short_code(token: &str) -> Result<String, String> {
    if !is_valid_token_shape(token) {
        return Err("令牌形态非法，拒绝申请短码".to_string());
    }
    let body = serde_json::json!({ "token": token }).to_string();
    let reply = admin_post("/api/admin/shortlink", &body)?;
    if reply.status != 200 {
        return Err(format!(
            "网关返回 HTTP {}: {}",
            reply.status,
            reply.body.trim()
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(&reply.body).map_err(|e| format!("短码响应不可解析: {e}"))?;
    value
        .get("code")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "网关未返回短码".to_string())
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

/// 当前生效的通道描述（`ssh:<host>` 或直连 URL），供失败回执定位。
pub fn channel_label() -> String {
    let conf = load_conf();
    match conf.ssh {
        Some(host) => format!("ssh:{host}"),
        None => conf.url,
    }
}

/// 调用网关管理端点（POST，JSON body），返回状态码与响应体。
///
/// 口令缺失即失败（不静默降级）：管理动作未生效必须是可观测的。
pub fn admin_post(path: &str, body: &str) -> Result<AdminReply, String> {
    let conf = load_conf();
    let Some(token) = conf.token.as_deref() else {
        return Err("未配置管理口令（GATE_ADMIN_TOKEN 或 ~/.pony/gate_admin.conf 的 token=）".to_string());
    };
    match conf.ssh.as_deref() {
        Some(host) => post_via_ssh(host, token, path, body),
        None => post_via_http(&conf.url, token, path, body),
    }
}

/// 把标识符推送进网关内存黑名单（`POST /api/user/revoke`）。
///
/// 未配置口令时返回 `ok=false`（调用方据此提示「仅本地落盘，需人工收口」），
/// 而非静默成功——撤销失败必须是可观测的。
pub fn push_revoke(identifier: &str) -> PushReport {
    if !is_valid_identifier(identifier) {
        return PushReport::failed("none".to_string(), format!("非法撤销标识符: {identifier}"));
    }

    let body = serde_json::json!({ "identifier": identifier }).to_string();
    match admin_post("/api/user/revoke", &body) {
        Ok(reply) if reply.status == 200 => {
            PushReport::ok(channel_label(), "网关内存黑名单已更新")
        }
        Ok(reply) => PushReport::failed(channel_label(), format!("网关返回 HTTP {}", reply.status)),
        Err(e) => PushReport::failed(gate_failure_channel(), e),
    }
}

/// 失败回执的通道标签：口令缺失时沿用既有 `none` 口径，其余给真实通道。
fn gate_failure_channel() -> String {
    if load_conf().token.is_some() {
        channel_label()
    } else {
        "none".to_string()
    }
}

/// 直连调用（网关与 CLI 同机）。
fn post_via_http(url: &str, token: &str, path: &str, body: &str) -> Result<AdminReply, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败: {e}"))?;
    let resp = client
        .post(format!("{url}{path}"))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| format!("连接网关失败: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    Ok(AdminReply { status, body: text })
}

/// 经 SSH 在网关本机调用：口令、路径与 body 走 stdin，不进命令行。
fn post_via_ssh(host: &str, token: &str, path: &str, body: &str) -> Result<AdminReply, String> {
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
            .write_all(format!("{token}\n{path}\n{body}").as_bytes())
            .map_err(|e| format!("写入 ssh stdin 失败: {e}"))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh 执行失败: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "ssh 无输出（{}）",
            err.trim().lines().next().unwrap_or("未知错误")
        ));
    }
    AdminReply::parse(&stdout)
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

    #[test]
    fn admin_reply_splits_body_and_status() {
        let reply = AdminReply::parse("{\"ok\":true}\n200\n").unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, "{\"ok\":true}");

        // 多行响应体：只把末行当状态码
        let reply = AdminReply::parse("line1\nline2\n401").unwrap();
        assert_eq!(reply.status, 401);
        assert_eq!(reply.body, "line1\nline2");

        // 空响应体（撤销端点只回状态码时同样成立）
        let reply = AdminReply::parse("200\n").unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, "");

        assert!(AdminReply::parse("oops").is_err());
    }
}
