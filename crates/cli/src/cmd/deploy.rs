//! `pproxy deploy` — 自动化部署上游服务（CF Worker / Vercel 函数 / Gate Worker）。
//!
//! 设计原则：用户只需在 init 时填入 token，部署工作尽量自动化。
//! 对于需手动操作的步骤（如 wrangler login/Vercel project link），提供清晰指引。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::PonyConfig;
use crate::{EXIT_FAILURE, EXIT_OK};

/// 可部署的目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    CfWorker,
    Vercel,
    Gate,
    All,
}

impl Target {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "cf-worker" | "cf_worker" | "cf" => Some(Self::CfWorker),
            "vercel" => Some(Self::Vercel),
            "gate" | "gate-worker" | "gate_worker" => Some(Self::Gate),
            "all"  | "a" => Some(Self::All),
            _ => None,
        }
    }

    pub fn all_targets() -> &'static [Target] {
        &[Target::CfWorker, Target::Vercel, Target::Gate]
    }

    fn label(self) -> &'static str {
        match self {
            Target::CfWorker => "CF Worker (edge.example.com)",
            Target::Vercel => "Vercel 函数 (vedge.example.com)",
            Target::Gate => "Gate Worker (gate.example.com)",
            Target::All => "全部",
        }
    }
}

/// 运行部署入口。
pub fn run(target: Target, cfg: &PonyConfig) -> Result<i32, String> {
    let deploy_root = resolve_deploy_root()?;

    match target {
        Target::All => {
            let mut ok = true;
            for t in Target::all_targets() {
                println!("\n╔══════════════════════════════════════════════════╗");
                println!("║  部署 {}  ", t.label());
                println!("╚══════════════════════════════════════════════════╝");
                match deploy_one(*t, cfg, &deploy_root) {
                    Ok(0) => println!("✓ {} 部署成功\n", t.label()),
                    Ok(code) => { ok = false; eprintln!("✗ {} 返回退出码 {code}\n", t.label()); }
                    Err(e) => { ok = false; eprintln!("✗ {}: {e}\n", t.label()); }
                }
            }
            Ok(if ok { EXIT_OK } else { EXIT_FAILURE })
        }
        t => {
            println!("\n╔══════════════════════════════════════════════════╗");
            println!("║  部署 {}  ", t.label());
            println!("╚══════════════════════════════════════════════════╝");
            deploy_one(t, cfg, &deploy_root)
        }
    }
}

fn deploy_one(target: Target, cfg: &PonyConfig, deploy_root: &Path) -> Result<i32, String> {
    match target {
        Target::CfWorker => deploy_cf_worker(cfg, deploy_root),
        Target::Vercel => deploy_vercel(cfg, deploy_root),
        Target::Gate => deploy_gate(cfg, deploy_root),
        Target::All => unreachable!(),
    }
}

fn npx_cmd() -> &'static str {
    if cfg!(windows) {
        "npx.cmd"
    } else {
        "npx"
    }
}

// ---- Deploy helpers ----

/// 查找 deploy/ 目录。优先环境变量 PPROXY_DEPLOY_DIR，否则从当前目录或上级目录查找。
fn resolve_deploy_root() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("PPROXY_DEPLOY_DIR") {
        let p = PathBuf::from(dir);
        if p.join("cf-worker").join("wrangler.toml").exists() {
            return Ok(p);
        }
    }

    // 从当前工作目录查找
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("deploy");
        if candidate.join("cf-worker").join("wrangler.toml").exists() {
            return Ok(candidate);
        }
        // 上一层（项目根目录）
        if let Some(parent) = cwd.parent() {
            let candidate = parent.join("deploy");
            if candidate.join("cf-worker").join("wrangler.toml").exists() {
                return Ok(candidate);
            }
        }
    }

    // 从 config.json 位置推断
    if Path::new("config.json").exists() {
        let candidate = PathBuf::from("deploy");
        if candidate.join("cf-worker").join("wrangler.toml").exists() {
            return Ok(candidate);
        }
    }

    Err("无法定位 deploy/ 目录。请在项目根目录运行此命令，或设置 PPROXY_DEPLOY_DIR 环境变量。".into())
}

/// 获取 PROXY_SECRET（配置 > 环境变量 > .secrets.env）。
fn resolve_proxy_secret(cfg: &PonyConfig) -> Result<String, String> {
    if let Some(s) = &cfg.proxy_secret {
        if !s.is_empty() {
            return Ok(s.clone());
        }
    }
    if let Ok(s) = std::env::var("PROXY_SECRET") {
        if !s.is_empty() {
            return Ok(s);
        }
    }
    // 尝试从 .secrets.env 读取
    let candidates = [
        PathBuf::from(".secrets.env"),
        dirs_or_cwd().join(".secrets.env"),
    ];
    for p in &candidates {
        if p.exists() {
            if let Ok(content) = std::fs::read_to_string(p) {
                for line in content.lines() {
                    if line.starts_with("export PROXY_SECRET=") {
                        let val = trim_secret_value(line.trim_start_matches("export PROXY_SECRET="));
                        if !val.is_empty() {
                            return Ok(val);
                        }
                    }
                }
            }
        }
    }
    Err("PROXY_SECRET 未配置。请运行 'pproxy init --interactive' 填入，或设置 PROXY_SECRET 环境变量。".into())
}

fn dirs_or_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

fn trim_secret_value(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// 检查 wrangler 是否已登录。
fn check_wrangler_login() -> bool {
    Command::new(npx_cmd())
        .args(["wrangler", "whoami"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 通过 stdin pipe 设置 wrangler secret（避免交互式提示）。
fn set_wrangler_secret(name: &str, value: &str, work_dir: &Path) -> Result<(), String> {
    let mut child = Command::new(npx_cmd())
        .args(["wrangler", "secret", "put", name])
        .current_dir(work_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 wrangler: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(value.as_bytes()).map_err(|e| format!("写入 wrangler stdin: {e}"))?;
        drop(stdin);
    }

    let output = child.wait_with_output().map_err(|e| format!("wrangler secret put: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("  wrangler secret put {name} 返回非零: {stderr}");
    }
    Ok(())
}

/// 计算 SHA-256 十六进制（公开供测试验证）。
pub(crate) fn sha256_hex(input: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

// ---- CF Worker 部署 ----

fn deploy_cf_worker(cfg: &PonyConfig, deploy_root: &Path) -> Result<i32, String> {
    let work_dir = deploy_root.join("cf-worker");
    if !work_dir.join("wrangler.toml").exists() {
        return Err(format!("cf-worker 目录不存在: {}", work_dir.display()));
    }

    // 检查 wrangler 登录状态
    let logged_in = check_wrangler_login();
    if !logged_in {
        println!("  wrangler 未登录，尝试自动登录...");
        let login_status = Command::new(npx_cmd())
            .args(["wrangler", "login"])
            .status()
            .map_err(|e| format!("wrangler login 失败: {e}"))?;
        if !login_status.success() {
            return Err("wrangler 登录失败，请手动运行: npx wrangler login".into());
        }
    }

    // 设置 PROXY_SECRET
    let proxy_secret = resolve_proxy_secret(cfg)?;
    println!("  设置 PROXY_SECRET...");
    set_wrangler_secret("PROXY_SECRET", &proxy_secret, &work_dir)?;

    // 部署
    println!("  正在部署 CF Worker...");
    let status = Command::new(npx_cmd())
        .args(["wrangler", "deploy"])
        .current_dir(&work_dir)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("wrangler deploy 失败: {e}"))?;

    if !status.success() {
        eprintln!();
        eprintln!("┌─ 部署失败 — 手动部署步骤 ────────────────────");
        eprintln!("│ cd {}", work_dir.display());
        eprintln!("│ npx wrangler login");
        eprintln!("│ npx wrangler secret put PROXY_SECRET");
        eprintln!("│ npx wrangler deploy");
        eprintln!("└─────────────────────────────────────────────────");
        return Err("CF Worker 部署失败".into());
    }

    println!("✓ CF Worker 部署成功 (edge.example.com)");
    Ok(EXIT_OK)
}

// ---- Vercel 部署 ----

fn deploy_vercel(cfg: &PonyConfig, deploy_root: &Path) -> Result<i32, String> {
    let work_dir = deploy_root.join("vercel");
    if !work_dir.join("vercel.json").exists() {
        return Err(format!("vercel 目录不存在: {}", work_dir.display()));
    }

    let token = match &cfg.vercel_token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => match std::env::var("VERCEL_TOKEN").ok() {
            Some(t) if !t.is_empty() => t,
            _ => return Err(
                "Vercel Token 未配置。请运行 'pproxy init --interactive' 填入，\n\
                 或设置 VERCEL_TOKEN 环境变量。".into()
            ),
        },
    };

    let proxy_secret = resolve_proxy_secret(cfg)?;

    // 检查是否已 link
    let mut has_project = work_dir.join(".vercel").join("project.json").exists();
    if !has_project {
        println!("  Vercel 项目尚未关联，正在尝试通过 API 自动探测并创建关联...");
        let client = crate::cloud::VercelClient::new(&token, None);
        if let Ok(acc) = client.get_account_info() {
            let (team_id, org_id) = if let Some(t) = acc.teams.into_iter().next() {
                (Some(t.id.clone()), t.id)
            } else {
                (None, acc.user.id)
            };
            let scoped = crate::cloud::VercelClient::new(&token, team_id);
            if let Ok(p) = scoped.ensure_project("pproxy-edge-v2") {
                if crate::cloud::write_local_project_json(&work_dir, &p.id, &org_id, "pproxy-edge-v2").is_ok() {
                    println!("  ✓ 已自动创建并关联项目: pproxy-edge-v2");
                    has_project = true;
                }
            }
        }
    }

    if !has_project {
        println!("  正在尝试通过 CLI 进行 vercel link...");
        let link_status = Command::new(npx_cmd())
            .args(["vercel", "link", "--confirm"])
            .current_dir(&work_dir)
            .env("VERCEL_TOKEN", &token)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| format!("vercel link 失败: {e}"))?;

        if !link_status.success() {
            eprintln!();
            eprintln!("┌─ 手动关联步骤 ──────────────────────────────");
            eprintln!("│ cd {}", work_dir.display());
            eprintln!("│ vercel link");
            eprintln!("│ vercel pull");
            eprintln!("│ vercel env add PROXY_SECRET production");
            eprintln!("│ vercel deploy --prod");
            eprintln!("└─────────────────────────────────────────────────");
            return Err("Vercel 项目关联失败".into());
        }
    }

    // 通过 Vercel API 设置环境变量
    println!("  设置 PROXY_SECRET...");
    if let Err(e) = set_vercel_env(&token, &work_dir, "PROXY_SECRET", &proxy_secret) {
        eprintln!("  (非致命) 设置环境变量失败: {e}");
    }

    // 部署
    println!("  正在部署 Vercel 函数...");
    let status = Command::new(npx_cmd())
        .args(["vercel", "deploy", "--prod"])
        .current_dir(&work_dir)
        .env("VERCEL_TOKEN", &token)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("vercel deploy 失败: {e}"))?;

    if !status.success() {
        return Err("Vercel 部署失败".into());
    }

    println!("✓ Vercel 函数部署成功 (vedge.example.com)");
    Ok(EXIT_OK)
}

/// 通过 Vercel API 设置环境变量。
fn set_vercel_env(token: &str, work_dir: &Path, name: &str, value: &str) -> Result<(), String> {
    let proj_path = work_dir.join(".vercel").join("project.json");
    let project_id = if proj_path.exists() {
        let content = std::fs::read_to_string(&proj_path).map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        json.get("projectId").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        None
    };

    let project_id = match project_id {
        Some(id) => id,
        None => return Err("无法读取 .vercel/project.json".into()),
    };

    // 使用 Vercel API 设置环境变量
    let url = format!("https://api.vercel.com/v10/projects/{project_id}/env");
    let body = serde_json::json!({
        "type": "encrypted",
        "key": name,
        "value": value,
        "target": ["production"],
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .map_err(|e| format!("API 请求失败: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        // 200 也算成功（已存在时 upsert 返回 200）
        if status.as_u16() != 200 {
            eprintln!("  Vercel API 返回 {status}: {text}");
        }
    }
    Ok(())
}

// ---- Gate Worker 部署 ----

fn deploy_gate(cfg: &PonyConfig, deploy_root: &Path) -> Result<i32, String> {
    let work_dir = deploy_root.join("cf-gate-worker");
    if !work_dir.join("wrangler.toml").exists() {
        return Err(format!("cf-gate-worker 目录不存在: {}", work_dir.display()));
    }

    let tunnel_token = match &cfg.tunnel_token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => match std::env::var("GATE_TUNNEL_TOKEN").ok() {
            Some(t) if !t.is_empty() => t,
            _ => return Err(
                "Tunnel Token 未配置。请运行 'pproxy init --interactive' 填入，\n\
                 或设置 GATE_TUNNEL_TOKEN 环境变量。".into()
            ),
        },
    };

    // 检查 wrangler 登录
    let logged_in = check_wrangler_login();
    if !logged_in {
        println!("  wrangler 未登录，尝试自动登录...");
        let login_status = Command::new(npx_cmd())
            .args(["wrangler", "login"])
            .status()
            .map_err(|e| format!("wrangler login 失败: {e}"))?;
        if !login_status.success() {
            return Err("wrangler 登录失败，请手动运行: npx wrangler login".into());
        }
    }

    // 计算 SHA-256 哈希并设置 secret
    let hash = sha256_hex(&tunnel_token);
    println!("  设置 TUNNEL_TOKEN_HASH...");
    set_wrangler_secret("TUNNEL_TOKEN_HASH", &hash, &work_dir)?;

    // 部署
    println!("  正在部署 Gate Worker...");
    let status = Command::new(npx_cmd())
        .args(["wrangler", "deploy"])
        .current_dir(&work_dir)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("wrangler deploy 失败: {e}"))?;

    if !status.success() {
        eprintln!();
        eprintln!("┌─ 部署失败 — 手动部署步骤 ────────────────────");
        eprintln!("│ cd {}", work_dir.display());
        eprintln!("│ npx wrangler login");
        eprintln!("│ npx wrangler secret put TUNNEL_TOKEN_HASH");
        eprintln!("│ npx wrangler deploy");
        eprintln!("│");
        eprintln!("│ 部署完成后在 Cloudflare Dashboard 绑定自定义域：");
        eprintln!("│   gate.example.com → 此 Worker");
        eprintln!("└─────────────────────────────────────────────────");
        return Err("Gate Worker 部署失败".into());
    }

    println!("✓ Gate Worker 部署成功 (gate.example.com)");
    println!();
    println!("┌─ 后续配置 ──────────────────────────────────────");
    println!("│ 1. 在 Cloudflare Dashboard 绑定自定义域：");
    println!("│    gate.example.com → 此 Worker");
    println!("│ 2. 更新 .pproxy.env（如需）：");
    println!("│    PPROXY_TUNNEL_GATE_URL=wss://gate.example.com/ws");
    println!("│    PPROXY_TUNNEL_TOKEN={}", crate::config::redact(&tunnel_token));
    println!("└─────────────────────────────────────────────────");

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_from_str_valid() {
        assert_eq!(Target::from_str("cf-worker"), Some(Target::CfWorker));
        assert_eq!(Target::from_str("cf_worker"), Some(Target::CfWorker));
        assert_eq!(Target::from_str("cf"), Some(Target::CfWorker));
        assert_eq!(Target::from_str("vercel"), Some(Target::Vercel));
        assert_eq!(Target::from_str("gate"), Some(Target::Gate));
        assert_eq!(Target::from_str("gate-worker"), Some(Target::Gate));
        assert_eq!(Target::from_str("all"), Some(Target::All));
        assert_eq!(Target::from_str("a"), Some(Target::All));
    }

    #[test]
    fn target_from_str_invalid() {
        assert_eq!(Target::from_str(""), None);
        assert_eq!(Target::from_str("unknown"), None);
        assert_eq!(Target::from_str("cf_worker_extra"), None);
    }

    #[test]
    fn target_all_targets_contains_all_three() {
        let all = Target::all_targets();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&Target::CfWorker));
        assert!(all.contains(&Target::Vercel));
        assert!(all.contains(&Target::Gate));
    }

    #[test]
    fn target_label_not_empty() {
        for t in Target::all_targets() {
            assert!(!t.label().is_empty(), "label for {:?} should not be empty", t);
        }
    }

    #[test]
    fn sha256_hex_known_values() {
        // empty string
        let empty_hash = sha256_hex("");
        assert_eq!(empty_hash.len(), 64);
        // known test vector
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        // 开源安全整改：合成测试向量（勿放真实 token）
        assert_eq!(
            sha256_hex("gate_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "134581fb8dacdff0775c40219f89deabb59af765c14d3fe55d9e65502b2c8800"
        );
    }

    #[test]
    fn sha256_hex_different_inputs_different_hashes() {
        let h1 = sha256_hex("token_a");
        let h2 = sha256_hex("token_b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn trim_secret_value_strips_quotes_and_whitespace() {
        // trim_matches 从两端移除所有匹配字符，不要求配对
        assert_eq!(trim_secret_value("\"secret\""), "secret");
        assert_eq!(trim_secret_value("'secret'"), "secret");
        assert_eq!(trim_secret_value("  secret  "), "secret");
        // 只有一侧引号时也会被移除
        assert_eq!(trim_secret_value("\"secret"), "secret");
        // 无引号正常
        assert_eq!(trim_secret_value("secret"), "secret");
        assert_eq!(trim_secret_value(""), "");
    }

    #[test]
    fn resolve_proxy_secret_from_config() {
        let cfg = PonyConfig {
            server: "http://127.0.0.1:8900".into(),
            admin_token: "admin".into(),
            data_plane: None,
            cf_token: None,
            cf_account_tag: None,
            vercel_token: None,
            tunnel_token: None,
            tunnel_gate_url: None,
            proxy_secret: Some("my_secret".into()),
        };
        let result = resolve_proxy_secret(&cfg);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my_secret");
    }

    #[test]
    fn resolve_proxy_secret_missing_error() {
        let cfg = PonyConfig {
            server: "http://127.0.0.1:8900".into(),
            admin_token: "admin".into(),
            data_plane: None,
            cf_token: None,
            cf_account_tag: None,
            vercel_token: None,
            tunnel_token: None,
            tunnel_gate_url: None,
            proxy_secret: None,
        };
        // PROXY_SECRET 未配置，环境变量也不存在（测试环境），应返回错误
        let result = resolve_proxy_secret(&cfg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("PROXY_SECRET 未配置"));
    }

    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_deploy_root_from_env() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        // 用临时目录模拟 deploy/ 结构
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("cf-worker")).unwrap();
        std::fs::write(tmp.path().join("cf-worker").join("wrangler.toml"), "name = \"test\"").unwrap();
        std::env::set_var("PPROXY_DEPLOY_DIR", tmp.path());
        let result = resolve_deploy_root();
        std::env::remove_var("PPROXY_DEPLOY_DIR");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().canonicalize().unwrap(), tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn resolve_deploy_root_missing_in_env_dir() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        // 设置 PPROXY_DEPLOY_DIR 指向一个不存在 wrangler.toml 的目录
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("cf-worker")).unwrap();
        std::env::set_var("PPROXY_DEPLOY_DIR", tmp.path());
        let result = resolve_deploy_root();
        std::env::remove_var("PPROXY_DEPLOY_DIR");
        if let Err(e) = result {
            assert!(e.contains("无法定位"));
        }
    }
}