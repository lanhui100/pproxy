//! pproxy CLI 入口：解析 → 执行 → 退出码（M2 §2）。
//!
//! 本文件禁止业务逻辑：命令实现在 cmd/*，HTTP 在 client.rs，本地配置在 config.rs。

mod client;
mod cmd;
mod config;
mod export;
mod render;

use std::io::Write as _;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// 退出码契约（M2 §5）。
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_LOCAL_CONFIG: i32 = 2;
pub const EXIT_UNREACHABLE: i32 = 3;

#[derive(Parser)]
#[command(name = "pproxy", version, about = "Pony Proxy 管理 CLI")]
struct Cli {
    /// 覆盖 admin token（优先级最高：> PONY_ADMIN_TOKEN > config.toml）
    #[arg(long, global = true)]
    token: Option<String>,
    /// 覆盖管理面 base URL
    #[arg(long, global = true)]
    server: Option<String>,
    /// 覆盖数据面 base URL（export/doctor 用）
    #[arg(long, global = true)]
    data_plane: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 写入 ~/.pony/config.toml（或 --interactive 交互式引导）
    Init {
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        force: bool,
        /// 交互式初始化向导（推荐新用户使用）
        #[arg(long)]
        interactive: bool,
    },
    /// 部署上游服务（CF Worker / Vercel / Gate Worker）
    Deploy {
        /// 部署目标: cf-worker | vercel | gate | all
        target: String,
    },
    /// 服务状态（管理面 health + 本机 systemd + 环境代理）
    Status,
    /// 开启本机环境代理（设置 http_proxy / https_proxy 环境变量）
    On,
    /// 关闭本机环境代理（清除 http_proxy / https_proxy 环境变量）
    Off {
        /// 就地清除当前 shell 代理环境变量（无需 source）
        #[arg(long)]
        hard: bool,
    },
    /// 代理环境挂起/恢复/脚本生成（eval 模式，不依赖 source）
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    Start,
    Stop,
    Restart,
    Route {
        #[command(subcommand)]
        cmd: RouteCmd,
    },
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// 用量报表
    Usage {
        #[arg(long, default_value_t = 24)]
        hours: u64,
        #[arg(long)]
        route: Option<String>,
        #[arg(long)]
        token_id: Option<i64>,
    },
    /// 全路由体检
    Doctor {
        /// 数据面抽样探测用的明文 token（缺省跳过该环节）
        #[arg(long)]
        probe_token: Option<String>,
        /// CONNECT 隧道探针目标 host:port（缺省 oauth2.googleapis.com:443）
        #[arg(long)]
        tunnel_host: Option<String>,
    },
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

/// 代理环境挂起/恢复子命令（eval 模式）。
#[derive(Subcommand)]
enum EnvCmd {
    /// 保存当前代理环境变量快照，输出清除代码（eval 使用）
    Suspend,
    /// 从快照恢复代理环境变量，输出恢复代码（eval 使用）
    Resume,
    /// 生成兄弟项目代理隔离应急脚本（不依赖 Rust CLI）
    GenerateScript {
        /// 输出路径（默认 ~/.pony/mitigate.sh）
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
enum RouteCmd {
    List,
    Add {
        name: String,
        target_host: String,
        /// override 上游（worker|vercel|已配置上游名）；缺省自动选择
        #[arg(long)]
        upstream: Option<String>,
    },
    Rm {
        name: String,
    },
    Test {
        name: Option<String>,
        /// 测全部 enabled 路由（与 <NAME> 二选一）
        #[arg(long)]
        all: bool,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    Create {
        name: String,
        #[arg(long)]
        expires_days: Option<u64>,
    },
    List,
    Revoke {
        id: i64,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    Export {
        service: String,
        /// 路由名（缺省=service 名）
        #[arg(long)]
        route: Option<String>,
        /// 显式嵌入明文 token（用户从创建时保存处取）
        #[arg(long)]
        token: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            let mut err = std::io::stderr();
            let _ = writeln!(err, "error: {e}");
            // 本地配置缺失/非法 → 退出码 2（M2 §5）；其余 → 1
            let code = if e.is_local_config() {
                EXIT_LOCAL_CONFIG
            } else {
                EXIT_FAILURE
            };
            ExitCode::from(code as u8)
        }
    }
}

/// run 层错误：携带退出码类别（本地配置 → 2，其余 → 1）。
enum RunError {
    LocalConfig(config::ConfigError),
    Msg(String),
}

impl RunError {
    fn is_local_config(&self) -> bool {
        matches!(self, Self::LocalConfig(_))
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalConfig(e) => write!(f, "{e}"),
            Self::Msg(m) => write!(f, "{m}"),
        }
    }
}

impl From<config::ConfigError> for RunError {
    fn from(e: config::ConfigError) -> Self {
        Self::LocalConfig(e)
    }
}

impl From<String> for RunError {
    fn from(m: String) -> Self {
        Self::Msg(m)
    }
}

fn run(cli: Cli) -> Result<i32, RunError> {
    // init 优先：交互式或非交互式
    if let Command::Init { server, token, force, interactive } = &cli.command {
        if *interactive || server.is_none() {
            return cmd::init_interactive::run_interactive(*force).map_err(RunError::Msg);
        }
        let server = server.clone().unwrap_or_default();
        return init(&server, token.as_deref(), *force);
    }

    // deploy 也需要配置（但 deploy 本身会读取配置中的 token 等字段）
    if let Command::Deploy { target } = &cli.command {
        let cfg = config::load()?;
        let t = cmd::deploy::Target::from_str(target)
            .ok_or_else(|| RunError::Msg(format!("无效部署目标: {target} — 可选: cf-worker, vercel, gate, all")))?;
        return cmd::deploy::run(t, &cfg).map_err(RunError::Msg);
    }

    // start/stop/restart 纯本机 systemd，不读管理 API 配置
    let action = match &cli.command {
        Command::Start => Some("start"),
        Command::Stop => Some("stop"),
        Command::Restart => Some("restart"),
        _ => None,
    };
    if let Some(action) = action {
        return cmd::service::systemd_action(action).map_err(RunError::Msg);
    }

    // on/off/env 环境代理开关，不需要管理 API 配置
    match &cli.command {
        Command::On => {
            return cmd::proxy_env::toggle(true).map_err(RunError::Msg);
        }
        Command::Off { hard: true } => {
            return cmd::proxy_env::toggle_hard(false).map_err(RunError::Msg);
        }
        Command::Off { hard: false } => {
            return cmd::proxy_env::toggle(false).map_err(RunError::Msg);
        }
        Command::Env { cmd } => {
            return match cmd {
                EnvCmd::Suspend => cmd::proxy_env::env_suspend().map_err(RunError::Msg),
                EnvCmd::Resume => cmd::proxy_env::env_resume().map_err(RunError::Msg),
                EnvCmd::GenerateScript { output } => {
                    let out_path = output.as_deref().map(std::path::Path::new);
                    cmd::proxy_env::env_generate_script(out_path).map_err(RunError::Msg)
                }
            };
        }
        _ => {}
    }

    // 组装配置与 client
    let cfg = config::load()?;
    let server = cli.server.clone().unwrap_or_else(|| cfg.server.clone());
    let admin_token = cli
        .token
        .clone()
        .or_else(|| std::env::var("PPROXY_ADMIN_TOKEN").ok())
        .or_else(|| std::env::var("PONY_ADMIN_TOKEN").ok())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| cfg.admin_token.clone());
    let http = client::AdminClient::new(&server, &admin_token).map_err(|e| e.to_string())?;
    let data_plane_base: Option<String> = cli
        .data_plane
        .clone()
        .or_else(|| cfg.data_plane.clone().filter(|s| !s.is_empty()))
        .or_else(|| config::derive_data_plane(&cfg).ok());

    // cmd/* 层错误统一为 Msg 类别（退出码 1）；本地配置错误只可能出自 load()
    let dispatched: Result<i32, String> = match &cli.command {
        Command::Status => cmd::service::status(&http),
        Command::Route { cmd } => match cmd {
            RouteCmd::List => cmd::route::list(&http),
            RouteCmd::Add { name, target_host, upstream } => {
                cmd::route::add(&http, name, target_host, upstream.as_deref())
            }
            RouteCmd::Rm { name } => cmd::route::rm(&http, name),
            RouteCmd::Test { name, all } => match (name.as_deref(), *all) {
                (Some(n), false) => cmd::route::test(&http, n, false),
                (None, true) => cmd::route::test(&http, "-", true),
                _ => Err("route test: <NAME> 与 --all 必须二选一".into()),
            },
            RouteCmd::Enable { name } => cmd::route::set_enabled(&http, name, true),
            RouteCmd::Disable { name } => cmd::route::set_enabled(&http, name, false),
        },
        Command::Token { cmd } => match cmd {
            TokenCmd::Create { name, expires_days } => cmd::token::create(
                &http,
                name,
                *expires_days,
                data_plane_base.as_deref().unwrap_or("http://127.0.0.1:8899"),
            ),
            TokenCmd::List => cmd::token::list(&http),
            TokenCmd::Revoke { id } => cmd::token::revoke(&http, *id),
        },
        Command::Usage { hours, route, token_id } => {
            cmd::usage::report(&http, *hours, route.as_deref(), *token_id)
        }
        Command::Doctor { probe_token, tunnel_host } => cmd::doctor::run(
            &http,
            probe_token.as_deref(),
            data_plane_base,
            tunnel_host.as_deref().unwrap_or("oauth2.googleapis.com:443"),
        ),
        Command::Config {
            cmd: ConfigCmd::Export { service, route, token },
        } => cmd::export_cmd::run(&cfg, service, route.as_deref(), token.as_deref()),
        Command::Init { .. } | Command::Deploy { .. } | Command::Start | Command::Stop | Command::Restart | Command::On | Command::Off { .. } | Command::Env { .. } => {
            unreachable!("handled above")
        }
    };
    dispatched.map_err(RunError::Msg)
}

fn init(server: &str, token: Option<&str>, force: bool) -> Result<i32, RunError> {
    let path = config::config_path()?;
    if path.exists() && !force {
        return Err(RunError::Msg(format!(
            "config already exists: {} (use --force to overwrite)",
            path.display()
        )));
    }
    let cfg = config::PonyConfig {
        server: server.to_string(),
        admin_token: token.unwrap_or("").to_string(),
        data_plane: None,
        cf_token: None,
        cf_account_tag: None,
        vercel_token: None,
        tunnel_token: None,
        proxy_secret: None,
    };
    let written = config::save(&cfg)?;
    println!("config written: {}", written.display());
    if token.is_none() {
        let _ = writeln!(
            std::io::stderr(),
            "note: admin_token 未写入 — 后续可用 PPROXY_ADMIN_TOKEN 环境变量或 'pproxy init --token <t>' 补充"
        );
    }
    Ok(EXIT_OK)
}
