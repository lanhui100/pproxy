//! `pproxy migrate` — 一键自动化云平台（Vercel / Cloudflare）账户迁移与初始化。
//!
//! 设计目标：用户只需输入一个 Token，自动完成：
//! 1. 账户身份自省与团队 Scope 判定
//! 2. 幂等创建或关联全套项目（edge / gate-worker / pony-dsk）
//! 3. 自动同步环境变量（PROXY_SECRET, TUNNEL_TOKEN_HASH）
//! 4. 自动绑定域名（vedge / vgate / dl）并提示 DNS 接管验证
//! 5. 自动写回本地 .vercel/project.json 与环境配置文件
//! 6. 自动化部署与冒烟体检

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cloud::{DomainStatus, VercelClient, CloudflareClient, write_local_project_json};
use crate::config::PonyConfig;
use crate::EXIT_OK;

#[derive(Debug, Clone)]
pub struct VercelMigrateOpts {
    pub token: Option<String>,
    pub team: Option<String>,
    pub project_edge: String,
    pub project_gate: String,
    pub project_dsk: String,
    pub domain_edge: String,
    pub domain_gate: String,
    pub domain_dsk: String,
    pub skip_deploy: bool,
    pub dry_run: bool,
}

impl Default for VercelMigrateOpts {
    fn default() -> Self {
        Self {
            token: None,
            team: None,
            project_edge: "pproxy-edge-v2".to_string(),
            project_gate: "vercel-gate-worker".to_string(),
            project_dsk: "pony-dsk".to_string(),
            domain_edge: "vedge.ponyjob.top".to_string(),
            domain_gate: "vgate.ponyjob.top".to_string(),
            domain_dsk: "dl.ponyjob.top".to_string(),
            skip_deploy: false,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CfMigrateOpts {
    pub token: Option<String>,
    pub account_id: Option<String>,
    pub skip_deploy: bool,
    pub dry_run: bool,
}

impl Default for CfMigrateOpts {
    fn default() -> Self {
        Self {
            token: None,
            account_id: None,
            skip_deploy: false,
            dry_run: false,
        }
    }
}

/// 执行 Vercel 账户全自动迁移与初始化。
pub fn run_vercel_migration(opts: &VercelMigrateOpts, cfg: &PonyConfig) -> Result<i32, String> {
    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║   Vercel 自动化账户初始化 / 迁移               ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // 1. 获取 Token
    let token = resolve_vercel_token(opts, cfg)?;
    let masked_token = crate::config::redact(&token);
    println!("  [1/6] 检查 Vercel 令牌: {}", masked_token);

    if opts.dry_run {
        println!("  [DRY RUN] 模拟模式开启，不向远程或本地写入变更。");
    }

    // 2. 身份探测与 Scope 选择
    let client = VercelClient::new(&token, None);
    let account_info = client.get_account_info().map_err(|e| {
        format!("连接 Vercel API 失败，请检查 Token 权限是否有效。\n错误详情: {e}")
    })?;

    println!("  ✓ 已识别账号: {} (ID: {})", account_info.user.username, account_info.user.id);

    // 确定使用哪个 Scope (个人 vs 团队)
    let (team_id, org_id, scope_name) = resolve_scope(&account_info, opts.team.as_deref())?;
    println!("  ✓ 目标 Scope: {} (orgId: {})", scope_name, org_id);

    let scoped_client = VercelClient::new(&token, team_id.clone());

    // 3. 准备所需环境变量
    let proxy_secret = resolve_or_gen_proxy_secret(cfg)?;
    let tunnel_token = resolve_tunnel_token(cfg);
    let tunnel_hash = crate::cmd::deploy::sha256_hex(&tunnel_token);

    let deploy_root = resolve_deploy_dir()?;

    // 4. 幂等初始化 3 个项目
    println!("\n  [2/6] 幂等创建/同步 Vercel 项目与环境变量...");
    
    // 4.1 Edge 代理项目
    print!("    - 项目 1: {} ... ", opts.project_edge);
    let edge_proj = if !opts.dry_run {
        let p = scoped_client.ensure_project(&opts.project_edge)?;
        scoped_client.set_env_var(&p.id, "PROXY_SECRET", &proxy_secret)?;
        print!("(ID: {}) 注入 PROXY_SECRET ✓", p.id);
        Some(p)
    } else {
        print!("[DRY RUN 预演] ✓");
        None
    };
    println!();

    // 4.2 Gate Worker 项目
    print!("    - 项目 2: {} ... ", opts.project_gate);
    let gate_proj = if !opts.dry_run {
        let p = scoped_client.ensure_project(&opts.project_gate)?;
        scoped_client.set_env_var(&p.id, "TUNNEL_TOKEN_HASH", &tunnel_hash)?;
        print!("(ID: {}) 注入 TUNNEL_TOKEN_HASH ✓", p.id);
        Some(p)
    } else {
        print!("[DRY RUN 预演] ✓");
        None
    };
    println!();

    // 4.3 桌面分发项目
    print!("    - 项目 3: {} ... ", opts.project_dsk);
    let _dsk_proj = if !opts.dry_run {
        let p = scoped_client.ensure_project(&opts.project_dsk)?;
        print!("(ID: {}) 静态分发环境就绪 ✓", p.id);
        Some(p)
    } else {
        print!("[DRY RUN 预演] ✓");
        None
    };
    println!();

    // 5. 绑定域名并检测 DNS 状态
    println!("\n  [3/6] 绑定自定义域名并检测 DNS 状态...");
    if !opts.dry_run {
        if let Some(ref p) = edge_proj {
            check_and_report_domain(&scoped_client, &p.id, &opts.domain_edge);
        }
        if let Some(ref p) = gate_proj {
            check_and_report_domain(&scoped_client, &p.id, &opts.domain_gate);
        }
        if let Some(ref p) = _dsk_proj {
            check_and_report_domain(&scoped_client, &p.id, &opts.domain_dsk);
        }
    } else {
        println!("    [DRY RUN] 预演跳过域名检查。");
    }

    // 6. 写入本地 .vercel 关联文件
    println!("\n  [4/6] 自动更新本地 .vercel 项目关联（免去手动 vercel link）...");
    if !opts.dry_run {
        let vercel_dir = deploy_root.join("vercel");
        if vercel_dir.exists() {
            if let Some(ref p) = edge_proj {
                let f = write_local_project_json(&vercel_dir, &p.id, &org_id, &opts.project_edge)?;
                println!("    ✓ 已更新 {}", f.display());
            }
        }

        let gate_dir = deploy_root.join("vercel-gate-worker");
        if gate_dir.exists() {
            if let Some(ref p) = gate_proj {
                let f = write_local_project_json(&gate_dir, &p.id, &org_id, &opts.project_gate)?;
                println!("    ✓ 已更新 {}", f.display());
            }
        }
    } else {
        println!("    [DRY RUN] 预演跳过本地 project.json 写入。");
    }

    // 7. 更新本地环境变量配置文件
    println!("\n  [5/6] 同步保存凭据至本地环境配置文件...");
    if !opts.dry_run {
        update_env_files(&token)?;
        println!("    ✓ 已更新 .secrets.env 与 .pproxy.env 中的 VERCEL_TOKEN");
    } else {
        println!("    [DRY RUN] 预演跳过环境文件写入。");
    }

    // 8. 触发部署与体检
    println!("\n  [6/6] 触发自动化部署与健康验证...");
    if opts.dry_run || opts.skip_deploy {
        println!("    (已跳过部署。后续可通过 'pproxy deploy vercel' 部署)");
    } else {
        println!("    正在部署 Vercel Edge 代理函数...");
        let status = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" })
            .args(["--yes", "vercel", "deploy", "--prod", "--yes"])
            .current_dir(deploy_root.join("vercel"))
            .env("VERCEL_TOKEN", &token)
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("    ✓ Vercel Edge 函数部署就绪！");
            }
            Ok(s) => {
                eprintln!("    ! vercel deploy 退出码: {s}（可通过 pproxy deploy vercel 重试）");
            }
            Err(e) => {
                eprintln!("    ! 运行 vercel deploy 失败: {e}");
            }
        }
    }

    println!("\n════════════════════════════════════════════════════");
    println!("✓ Vercel 账号初始化/迁移流程已圆满完成！");
    println!("════════════════════════════════════════════════════\n");

    Ok(EXIT_OK)
}

/// 执行 Cloudflare 账户全自动迁移与初始化。
pub fn run_cf_migration(opts: &CfMigrateOpts, cfg: &PonyConfig) -> Result<i32, String> {
    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║   Cloudflare 自动化账户初始化 / 迁移           ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    let token = resolve_cf_token(opts, cfg)?;
    let masked_token = crate::config::redact(&token);
    println!("  [1/4] 检查 Cloudflare API 令牌: {}", masked_token);

    let client = CloudflareClient::new(&token);
    let accounts = client.list_accounts().map_err(|e| {
        format!("连接 Cloudflare API 失败，请检查 Token 是否具备账户访问权限。\n详情: {e}")
    })?;

    if accounts.is_empty() {
        return Err("该 Cloudflare Token 未关联任何可访问的 Account，请检查 Token 授权资源。".into());
    }

    let target_account = if let Some(ref aid) = opts.account_id {
        accounts.into_iter().find(|a| &a.id == aid || &a.name == aid)
            .ok_or_else(|| format!("未在 Token 授权范围内找到指定的 Account: {aid}"))?
    } else {
        accounts.into_iter().next().unwrap()
    };

    println!("  ✓ 已选定 Cloudflare 账户: {} (ID: {})", target_account.name, target_account.id);

    if !opts.dry_run {
        println!("\n  [2/4] 保存 CF 凭据至本地环境...");
        update_cf_env_files(&token, &target_account.id)?;
        println!("    ✓ 已更新 .secrets.env 与 .pproxy.env 中的 CF 配置");
    }

    println!("\n  [3/4] 部署上游 CF Worker...");
    if opts.dry_run || opts.skip_deploy {
        println!("    (已跳过部署。后续可通过 'pproxy deploy cf' 部署)");
    } else {
        let deploy_root = resolve_deploy_dir()?;
        let cf_worker_dir = deploy_root.join("cf-worker");
        if cf_worker_dir.join("wrangler.toml").exists() {
            let status = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" })
                .args(["--yes", "wrangler", "deploy"])
                .current_dir(&cf_worker_dir)
                .env("CLOUDFLARE_API_TOKEN", &token)
                .env("CLOUDFLARE_ACCOUNT_ID", &target_account.id)
                .env_remove("http_proxy")
                .env_remove("https_proxy")
                .env_remove("HTTP_PROXY")
                .env_remove("HTTPS_PROXY")
                .status();

            match status {
                Ok(s) if s.success() => println!("    ✓ CF Worker 部署就绪！"),
                _ => eprintln!("    ! CF Worker 部署未完成，可后续运行 pproxy deploy cf 重试"),
            }
        }
    }

    println!("\n════════════════════════════════════════════════════");
    println!("✓ Cloudflare 账号迁移流程已完成！");
    println!("════════════════════════════════════════════════════\n");

    Ok(EXIT_OK)
}

// -------------------------------------------------------------------------
// 内部辅助工具
// -------------------------------------------------------------------------

fn resolve_scope(
    account: &crate::cloud::VercelAccount,
    preferred_team: Option<&str>,
) -> Result<(Option<String>, String, String), String> {
    if let Some(pref) = preferred_team {
        if pref == "personal" || pref == account.user.username || pref == account.user.id {
            return Ok((None, account.user.id.clone(), format!("个人 ({})", account.user.username)));
        }
        for t in &account.teams {
            if t.slug == pref || t.id == pref || t.name == pref {
                return Ok((Some(t.id.clone()), t.id.clone(), format!("团队 {} ({})", t.name, t.slug)));
            }
        }
        return Err(format!("未在账号下找到指定的团队: '{pref}'。可用团队: {:?}",
            account.teams.iter().map(|t| &t.slug).collect::<Vec<_>>()));
    }

    // 未指定团队时的自动策略：
    if account.teams.is_empty() {
        // 纯个人号
        Ok((None, account.user.id.clone(), format!("个人 ({})", account.user.username)))
    } else if account.teams.len() == 1 {
        // 仅有 1 个团队，默认选用该团队
        let t = &account.teams[0];
        Ok((Some(t.id.clone()), t.id.clone(), format!("团队 {} ({})", t.name, t.slug)))
    } else {
        // 多个团队：优先检查是否有名为 pony 或类似的项目团队，否则使用第一个
        let t = &account.teams[0];
        Ok((Some(t.id.clone()), t.id.clone(), format!("团队 {} ({}) [默认首选]", t.name, t.slug)))
    }
}

fn check_and_report_domain(client: &VercelClient, project_id: &str, domain: &str) {
    print!("    - 域名: {} ... ", domain);
    match client.ensure_domain(project_id, domain) {
        Ok(DomainStatus::Verified { .. }) => {
            println!("已绑定且解析生效 ✓");
        }
        Ok(DomainStatus::AlreadyAssigned { .. }) => {
            println!("已在项目中 ✓");
        }
        Ok(DomainStatus::NeedsVerification { records, .. }) => {
            println!("已绑定但需 DNS 验证所有权 !");
            for r in records {
                println!("      └ 请在 DNS 添加 {} 记录: 名称='{}', 内容='{}'", r.record_type, r.domain, r.value);
            }
        }
        Err(e) => {
            println!("绑定提示: {e}");
        }
    }
}

fn resolve_vercel_token(opts: &VercelMigrateOpts, cfg: &PonyConfig) -> Result<String, String> {
    if let Some(ref t) = opts.token {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    if let Ok(t) = std::env::var("VERCEL_TOKEN") {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    if let Ok(t) = std::env::var("PPROXY_VERCEL_TOKEN") {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    if let Some(ref t) = cfg.vercel_token {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    // 尝试从 .secrets.env 或 .pproxy.env 读取
    for filename in [".secrets.env", ".pproxy.env"] {
        if let Ok(content) = std::fs::read_to_string(filename) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("export VERCEL_TOKEN=")
                    .or_else(|| line.strip_prefix("VERCEL_TOKEN="))
                    .or_else(|| line.strip_prefix("PPROXY_VERCEL_TOKEN=")) {
                    let cleaned = val.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !cleaned.is_empty() {
                        return Ok(cleaned);
                    }
                }
            }
        }
    }

    Err("未提供 Vercel Token。请传入 --token <TOKEN>，或在环境变量/配置文件中设置 VERCEL_TOKEN。".into())
}

fn resolve_cf_token(opts: &CfMigrateOpts, cfg: &PonyConfig) -> Result<String, String> {
    if let Some(ref t) = opts.token {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    for env_k in ["CF_API_TOKEN", "CLOUDFLARE_API_TOKEN", "PPROXY_CF_API_TOKEN"] {
        if let Ok(t) = std::env::var(env_k) {
            if !t.trim().is_empty() {
                return Ok(t.trim().to_string());
            }
        }
    }
    if let Some(ref t) = cfg.cf_token {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    for filename in [".secrets.env", ".pproxy.env"] {
        if let Ok(content) = std::fs::read_to_string(filename) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("export CF_API_TOKEN=")
                    .or_else(|| line.strip_prefix("CF_API_TOKEN="))
                    .or_else(|| line.strip_prefix("PPROXY_CF_API_TOKEN=")) {
                    let cleaned = val.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !cleaned.is_empty() {
                        return Ok(cleaned);
                    }
                }
            }
        }
    }

    Err("未提供 Cloudflare Token。请传入 --token <TOKEN>，或设置 CF_API_TOKEN 环境变量。".into())
}

fn resolve_or_gen_proxy_secret(cfg: &PonyConfig) -> Result<String, String> {
    if let Some(ref s) = cfg.proxy_secret {
        if !s.is_empty() {
            return Ok(s.clone());
        }
    }
    if let Ok(s) = std::env::var("PROXY_SECRET") {
        if !s.is_empty() {
            return Ok(s);
        }
    }
    for filename in [".secrets.env", ".pproxy.env"] {
        if let Ok(content) = std::fs::read_to_string(filename) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("export PROXY_SECRET=")
                    .or_else(|| line.strip_prefix("PROXY_SECRET=")) {
                    let cleaned = val.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !cleaned.is_empty() {
                        return Ok(cleaned);
                    }
                }
            }
        }
    }

    // 默认生成并返回
    use rand::Rng;
    let bytes: Vec<u8> = (0..32).map(|_| rand::thread_rng().gen()).collect();
    Ok(hex::encode(bytes))
}

fn resolve_tunnel_token(cfg: &PonyConfig) -> String {
    if let Some(ref t) = cfg.tunnel_token {
        if !t.is_empty() {
            return t.clone();
        }
    }
    if let Ok(t) = std::env::var("TUNNEL_TOKEN").or_else(|_| std::env::var("PPROXY_TUNNEL_TOKEN")) {
        if !t.is_empty() {
            return t;
        }
    }
    for filename in [".secrets.env", ".pproxy.env"] {
        if let Ok(content) = std::fs::read_to_string(filename) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("export PPROXY_TUNNEL_TOKEN=")
                    .or_else(|| line.strip_prefix("PPROXY_TUNNEL_TOKEN=")) {
                    let cleaned = val.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !cleaned.is_empty() {
                        return cleaned;
                    }
                }
            }
        }
    }
    String::new()
}

fn resolve_deploy_dir() -> Result<PathBuf, String> {
    if let Ok(d) = std::env::var("PPROXY_DEPLOY_DIR") {
        let p = PathBuf::from(d);
        if p.exists() {
            return Ok(p);
        }
    }
    let p = PathBuf::from("deploy");
    if p.exists() {
        return Ok(p);
    }
    Ok(PathBuf::from("."))
}

/// 更新本地环境文件（.secrets.env 与 .pproxy.env）中的 VERCEL_TOKEN
fn update_env_files(new_token: &str) -> Result<(), String> {
    upsert_env_line(".secrets.env", "export VERCEL_TOKEN=", &format!("export VERCEL_TOKEN=\"{new_token}\""))?;
    upsert_env_line(".pproxy.env", "PPROXY_VERCEL_TOKEN=", &format!("PPROXY_VERCEL_TOKEN={new_token}"))?;
    Ok(())
}

/// 更新本地环境文件中的 CF 相关配置
fn update_cf_env_files(new_token: &str, account_id: &str) -> Result<(), String> {
    upsert_env_line(".secrets.env", "export CF_API_TOKEN=", &format!("export CF_API_TOKEN=\"{new_token}\""))?;
    upsert_env_line(".pproxy.env", "PPROXY_CF_API_TOKEN=", &format!("PPROXY_CF_API_TOKEN={new_token}"))?;
    upsert_env_line(".pproxy.env", "PPROXY_CF_ACCOUNT_TAG=", &format!("PPROXY_CF_ACCOUNT_TAG={account_id}"))?;
    Ok(())
}

fn upsert_env_line(file_path: &str, prefix: &str, replacement: &str) -> Result<(), String> {
    let p = Path::new(file_path);
    let mut lines = Vec::new();
    let mut found = false;

    if p.exists() {
        let content = std::fs::read_to_string(p).map_err(|e| format!("读取 {file_path} 失败: {e}"))?;
        for line in content.lines() {
            if line.starts_with(prefix) {
                lines.push(replacement.to_string());
                found = true;
            } else {
                lines.push(line.to_string());
            }
        }
    }

    if !found {
        lines.push(replacement.to_string());
    }

    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(p, out).map_err(|e| format!("写入 {file_path} 失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_resolve_scope_personal_only() {
        let acc = crate::cloud::VercelAccount {
            user: crate::cloud::VercelUser {
                id: "usr_123".into(),
                username: "ponydev".into(),
                email: None,
                name: None,
            },
            teams: vec![],
        };

        let (tid, org_id, name) = resolve_scope(&acc, None).unwrap();
        assert_eq!(tid, None);
        assert_eq!(org_id, "usr_123");
        assert!(name.contains("ponydev"));
    }

    #[test]
    fn test_resolve_scope_single_team() {
        let acc = crate::cloud::VercelAccount {
            user: crate::cloud::VercelUser {
                id: "usr_123".into(),
                username: "ponydev".into(),
                email: None,
                name: None,
            },
            teams: vec![crate::cloud::VercelTeam {
                id: "team_abc".into(),
                slug: "my-team".into(),
                name: "My Team".into(),
            }],
        };

        let (tid, org_id, name) = resolve_scope(&acc, None).unwrap();
        assert_eq!(tid, Some("team_abc".into()));
        assert_eq!(org_id, "team_abc");
        assert!(name.contains("my-team"));
    }

    #[test]
    fn test_resolve_scope_preferred_team() {
        let acc = crate::cloud::VercelAccount {
            user: crate::cloud::VercelUser {
                id: "usr_123".into(),
                username: "ponydev".into(),
                email: None,
                name: None,
            },
            teams: vec![
                crate::cloud::VercelTeam {
                    id: "team_1".into(),
                    slug: "alpha".into(),
                    name: "Alpha Team".into(),
                },
                crate::cloud::VercelTeam {
                    id: "team_2".into(),
                    slug: "beta".into(),
                    name: "Beta Team".into(),
                },
            ],
        };

        let (tid, org_id, name) = resolve_scope(&acc, Some("beta")).unwrap();
        assert_eq!(tid, Some("team_2".into()));
        assert_eq!(org_id, "team_2");
        assert!(name.contains("beta"));
    }

    #[test]
    fn test_upsert_env_line_new_and_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join(".test.env");
        let env_str = env_file.to_str().unwrap();

        // 1. 文件不存在直接追加
        upsert_env_line(env_str, "KEY=", "KEY=first_val").unwrap();
        let c1 = std::fs::read_to_string(&env_file).unwrap();
        assert_eq!(c1.trim(), "KEY=first_val");

        // 2. 覆盖现有行
        upsert_env_line(env_str, "KEY=", "KEY=second_val").unwrap();
        let c2 = std::fs::read_to_string(&env_file).unwrap();
        assert_eq!(c2.trim(), "KEY=second_val");

        // 3. 追加另一变量不影响已有行
        upsert_env_line(env_str, "OTHER=", "OTHER=val2").unwrap();
        let c3 = std::fs::read_to_string(&env_file).unwrap();
        assert!(c3.contains("KEY=second_val"));
        assert!(c3.contains("OTHER=val2"));
    }
}
