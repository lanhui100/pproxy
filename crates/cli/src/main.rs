//! pproxy CLI 入口：解析 → 执行 → 退出码（M2 §2）。
//!
//! 本文件禁止业务逻辑：命令实现在 cmd/*，HTTP 在 client.rs，本地配置在 config.rs。

mod client;
pub mod cloud;
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
#[command(name = "pproxy", version, about = "Pony Proxy 智能多模代理 CLI")]
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
    /// 独立启动嵌入式网关服务（前台运行，同时开启数据代理面与管理控制面）
    Serve {
        /// 自定义数据代理监听地址（默认 127.0.0.1:8899）
        #[arg(long)]
        listen: Option<String>,
        /// 自定义管理面监听地址（默认 127.0.0.1:8900，--lan 模式为 0.0.0.0:8900）
        #[arg(long)]
        admin_listen: Option<String>,
        /// 开启局域网共享模式（绑定 0.0.0.0，同局域网手机/设备可直接使用）
        #[arg(long, short = 'g', alias = "share")]
        lan: bool,
        /// 自定义数据代理监听端口（默认 8899）
        #[arg(long, short = 'p')]
        port: Option<u16>,
    },
    /// 用户管理（Basic Auth 用户名/密码 或 签发多租户令牌）
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// 跨端安全配置同步（ChaCha20-Poly1305 加密）
    Sync {
        #[command(subcommand)]
        cmd: SyncCmd,
    },
    /// 分布式自包含集群管理（一键入网、对等互联、大盘监控）
    Cluster {
        #[command(subcommand)]
        cmd: ClusterCmd,
    },
    /// 独立运行本地高可用分发守护进程（Local HA Forwarder，仅供 serve 内部派生，勿手动前台运行）
    HaForwarder {
        /// 对外固定监听入口（默认 127.0.0.1:8899）
        #[arg(long)]
        listen: String,
        /// 本地主引擎内部端口（如 127.0.0.1:18899）
        #[arg(long)]
        local: String,
        /// 集群备灾候选节点（逗号分隔 SocketAddr）
        #[arg(long)]
        peers: String,
    },
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
    On {
        /// 直接输出 shell export 语句（用于 eval "$(pproxy on --eval)"）
        #[arg(long)]
        eval: bool,
    },
    /// 关闭本机环境代理（清除 http_proxy / https_proxy 环境变量）
    Off {
        /// 直接输出 shell unset 语句（用于 eval "$(pproxy off --eval)"）
        #[arg(long)]
        eval: bool,
        /// 就地清除当前 shell 代理环境变量（无需 source）
        #[arg(long)]
        hard: bool,
    },
    /// 代理环境挂起/恢复/脚本生成（eval 模式，不依赖 source）
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    /// 启动后台网关守护进程（Linux systemd 服务）
    Start,
    /// 停止运行中的网关服务（终止本地 serve 进程或 systemd 服务）
    Stop,
    /// 重启后台网关守护进程（Linux systemd 服务）
    Restart,
    /// 路由转发规则管理（查看/添加/删除上游路由，控制 AI 模型服务分流）
    Route {
        #[command(subcommand)]
        cmd: RouteCmd,
    },
    /// 访问凭证管理（生成/查看/吊销供客户端调用代理的数据面 Token）
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// 用量报表统计（统计分析近期各路由与令牌的请求次数与流量明细）
    Usage {
        /// 统计回溯时间窗口（小时，默认 24 小时）
        #[arg(long, default_value_t = 24)]
        hours: u64,
        /// 仅按指定路由名称过滤
        #[arg(long)]
        route: Option<String>,
        /// 仅按指定令牌 ID 过滤
        #[arg(long)]
        token_id: Option<i64>,
    },
    /// 网关健康体检（全路由上游探测与出海 WebSocket 隧道打通测试）
    Doctor {
        /// 数据面抽样探测用的明文 token（缺省跳过该环节）
        #[arg(long)]
        probe_token: Option<String>,
        /// CONNECT 隧道探针目标 host:port（缺省 oauth2.googleapis.com:443）
        #[arg(long)]
        tunnel_host: Option<String>,
    },
    /// 配置与客户端集成（导出 Clash/Cursor/Surge 配置、设置出海隧道等）
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// 检查并升级 pproxy CLI 至最新版本 (支持 update 别名)
    #[command(alias = "update")]
    Upgrade {
        /// 仅检查最新版本，不执行升级
        #[arg(long)]
        check: bool,
        /// 强制重新下载并覆盖当前版本
        #[arg(long, short)]
        force: bool,
        /// 升级或降级至指定版本 (如 v0.3.26)
        #[arg(long)]
        version: Option<String>,
        /// 自定义下载镜像基址
        #[arg(long)]
        mirror: Option<String>,
    },
    /// 手机与客户端 Clash Meta 配置生成与扫码导入（一键生成/订阅URL/终端二维码）
    Clash {
        /// 覆盖访问令牌（缺省自动使用或创建）
        #[arg(long)]
        token: Option<String>,
        /// 覆盖局域网 IP（缺省自动探测本机局域网 IP）
        #[arg(long)]
        lan_ip: Option<String>,
        /// 覆盖代理端口（默认 8899）
        #[arg(long, short = 'p')]
        port: Option<u16>,
        /// 仅打印订阅 URL 链接
        #[arg(long)]
        url_only: bool,
    },
    /// 云服务一键初始化与账户迁移（支持 Vercel 和 Cloudflare）
    Migrate {
        #[command(subcommand)]
        cmd: MigrateCmd,
    },
    /// 运行轻量原生出海 Gate 节点服务 (WS↔TCP 隧道桥与 HTTP/SSE 代理)
    GateServer {
        /// 监听端口 (默认 3101)
        #[arg(long, default_value_t = 3101)]
        port: u16,
        /// 监听地址 (默认 0.0.0.0)
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        /// 隧道令牌 sha256 哈希值 (留空则从 TUNNEL_TOKEN_HASH 环境变量读取)
        #[arg(long)]
        token_hash: Option<String>,
        /// 反向代理鉴权密钥 (留空则从 PROXY_SECRET 环境变量读取)
        #[arg(long)]
        proxy_secret: Option<String>,
    },
}

#[derive(Subcommand)]
enum MigrateCmd {
    /// 一键迁移或初始化 Vercel 账户与关联项目
    Vercel {
        /// Vercel Personal Access Token（以 vcp_ 开头）
        #[arg(long)]
        token: Option<String>,
        /// 指定团队 Scope（团队 slug 或团队 ID）
        #[arg(long)]
        team: Option<String>,
        /// Edge 代理项目名（默认 pproxy-edge-v2）
        #[arg(long, default_value = "pproxy-edge-v2")]
        project_edge: String,
        /// Gate Worker 项目名（默认 vercel-gate-worker）
        #[arg(long, default_value = "vercel-gate-worker")]
        project_gate: String,
        /// 桌面分发项目名（默认 pony-dsk）
        #[arg(long, default_value = "pony-dsk")]
        project_dsk: String,
        /// 跳过自动部署
        #[arg(long)]
        skip_deploy: bool,
        /// 预演模式（不向云端或本地写入变更）
        #[arg(long)]
        dry_run: bool,
    },
    /// 一键迁移或初始化 Cloudflare 账户
    Cf {
        /// Cloudflare API Token
        #[arg(long)]
        token: Option<String>,
        /// Cloudflare Account ID（默认自动探测）
        #[arg(long)]
        account_id: Option<String>,
        /// 跳过自动部署
        #[arg(long)]
        skip_deploy: bool,
        /// 预演模式（不向云端或本地写入变更）
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum UserCmd {
    /// 添加 Basic Auth 用户或签发商业化多租户令牌
    Add {
        username: String,
        /// 自定义密码（若不提供将自动生成高强度密码）
        #[arg(long, short = 'p')]
        password: Option<String>,
        /// 有效天数（默认 30 天）
        #[arg(long, short = 'd')]
        expires_days: Option<u32>,
        /// 商业化自包含令牌：周期总配额（如 50G, 100M）
        #[arg(long, short = 'q')]
        quota: Option<String>,
        /// 商业化自包含令牌：最大并发连接数（默认 3）
        #[arg(long, short = 'c', default_value = "3")]
        max_conns: usize,
    },
    /// 生成/初始化集群签名私钥与公钥
    Keygen,
    /// 列出所有用户
    List,
    /// 删除指定用户
    Rm { username: String },
    /// 禁用指定用户
    Disable { username: String },
    /// 启用指定用户
    Enable { username: String },
    /// 修改指定用户密码
    Passwd {
        username: String,
        #[arg(long, short = 'p')]
        password: String,
    },
    /// 废止/撤销指定多租户令牌或用户（加入全网黑名单）
    Revoke {
        /// 待撤销的令牌字符串 (usr_live_...) 或 用户名 (username)
        target: String,
    },
}

#[derive(Subcommand)]
enum SyncCmd {
    /// 导出跨端加密配置口令（有效期 10 分钟）
    Export {
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// 导入跨端加密配置口令
    Import {
        payload: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
}

#[derive(Subcommand)]
enum ClusterCmd {
    /// 生成节点加入令牌（One-Time Join Token）
    TokenCreate {
        /// 种子节点内网地址（如 100.95.193.103:8899）
        #[arg(long, short = 's')]
        seed: Option<String>,
        /// 令牌有效分钟数（默认 10 分钟，遵循安全审查限制）
        #[arg(long, short = 'm', default_value = "10")]
        valid_minutes: u64,
    },
    /// 将当前机器加入现有分布式备灾集群
    Join {
        /// 节点加入令牌
        #[arg(long, short = 't')]
        token: String,
        /// 种子节点地址（若令牌中未包含）
        #[arg(long, short = 'p')]
        peer: Option<String>,
        /// 入网后全自动拉取后台局域网双模服务 (Zero-Touch Bootstrap)
        #[arg(long, default_value = "false")]
        auto_start: bool,
    },
    /// 查看当前节点及全集群状态大盘
    Status,
    /// 触发全集群零停机滚动升级 (Zero-Downtime Rolling Upgrade)
    Upgrade {
        /// 本地新二进制路径（P2P 流式推送，推荐）
        #[arg(long, short = 'l')]
        local: Option<String>,
        /// MinIO 备份对象存储 URL
        #[arg(long, short = 'm')]
        minio: Option<String>,
        /// Cloudflare R2 备份 URL
        #[arg(long, short = 'r')]
        r2: Option<String>,
        /// 开发者 Ed25519 签名文件路径（.sig 强制校验防篡改）
        #[arg(long, short = 's')]
        sig: Option<String>,
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
    /// 列出所有已配置的路由规则及当前状态与上游
    List,
    /// 添加一条新的转发路由规则
    Add {
        /// 路由标识名称（如 openai、claude、gemini 等）
        name: String,
        /// 目标服务域名（如 api.openai.com、api.anthropic.com）
        target_host: String,
        /// override 上游（worker|vercel|已配置上游名）；缺省自动选择
        #[arg(long)]
        upstream: Option<String>,
    },
    /// 删除指定的路由规则
    Rm {
        /// 要删除的路由名称
        name: String,
    },
    /// 测试路由的连通性与可用性
    Test {
        /// 要测试的路由名称
        name: Option<String>,
        /// 测全部 enabled 路由（与 <NAME> 二选一）
        #[arg(long)]
        all: bool,
    },
    /// 启用指定的路由规则
    Enable {
        /// 待启用的路由名称
        name: String,
    },
    /// 禁用指定的路由规则（请求将不再转发）
    Disable {
        /// 待禁用的路由名称
        name: String,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// 创建新的访问令牌（用于客户端请求代理时的身份鉴权）
    Create {
        /// 令牌名称或备注（如 cursor、phone-clash、alice-dev）
        name: String,
        /// 令牌有效天数（缺省为永久有效）
        #[arg(long)]
        expires_days: Option<u64>,
    },
    /// 列出所有已生成的访问令牌及其使用状态
    List,
    /// 吊销指定的访问令牌（立即切断该令牌的代理权限）
    Revoke {
        /// 要吊销的令牌 ID（数字）
        id: i64,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// 导出第三方客户端配置片段（支持 subconverter / clash / surge / cursor 等格式）
    Export {
        /// 目标客户端类型: cursor | openai | claude | clash | surge | subconverter | env
        service: String,
        /// 关联路由名称（缺省自动导出全部可用路由）
        #[arg(long)]
        route: Option<String>,
        /// 数据面明文 token（缺省自动使用第一个有效 token）
        #[arg(long)]
        token: Option<String>,
        /// 生成终端二维码并保存本地配置文件（针对 clash）
        #[arg(long)]
        qr: bool,
    },
    /// 设置出海隧道 Gate URL 与 Token（持久化至 config.toml 与 .pproxy.env）
    SetTunnel {
        /// Gate 端点 URL（支持逗号分隔多个，如 wss://vgate.example.com/api/ws,wss://gate.example.com/ws）
        #[arg(long)]
        gate_url: Option<String>,
        /// 隧道认证 Token（如 gate_xxx）
        #[arg(long)]
        token: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => match code {
            EXIT_OK => ExitCode::SUCCESS,
            _ => ExitCode::from(code as u8),
        },
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "error: {e}");
            let code = if e.is_local_config() {
                EXIT_LOCAL_CONFIG
            } else {
                EXIT_FAILURE
            };
            ExitCode::from(code as u8)
        }
    }
}

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
    // 0. upgrade 自升级命令（无需本地服务端配置）
    if let Command::Upgrade { check, force, version, mirror } = &cli.command {
        return cmd::upgrade::run(*check, *force, version.as_deref(), mirror.as_deref())
            .map_err(RunError::Msg);
    }

    // 1. serve 独立起服
    if let Command::Serve { listen, admin_listen, lan, port } = &cli.command {
        return cmd::serve::run(listen.as_deref(), admin_listen.as_deref(), *lan, *port).map_err(RunError::Msg);
    }

    // 1.0 ha-forwarder 独立高可用守护进程（由 serve 内部派生）
    if let Command::HaForwarder { listen, local, peers } = &cli.command {
        let listen_sock: std::net::SocketAddr = listen.parse().map_err(|e: std::net::AddrParseError| RunError::Msg(e.to_string()))?;
        let local_sock: std::net::SocketAddr = local.parse().map_err(|e: std::net::AddrParseError| RunError::Msg(e.to_string()))?;
        let peer_addrs: Vec<std::net::SocketAddr> = peers
            .split([',', ';'])
            .filter_map(|p| p.trim().parse().ok())
            .collect();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| RunError::Msg(e.to_string()))?;

        let start_result = rt.block_on(async move {
            // ha-forwarder 独立进程：初始化 tracing 以便 failover 日志可观测（stderr）
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .try_init()
                .ok();

            let cluster_key = std::env::var("PPROXY_CLUSTER_KEY").unwrap_or_default();
            let node_id = std::env::var("PPROXY_CLUSTER_NODE_ID")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "node-local".into());
            let ha = std::sync::Arc::new(
                pproxy_core::LocalHaForwarder::new(listen_sock, local_sock, peer_addrs)
                    .with_cluster_identity(cluster_key, node_id),
            );
            ha.start().await.map_err(|e| e.to_string())?;
            // 常驻运行（start 内部已 spawn 转发循环，此处保持主协程存活）
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        });

        start_result.map_err(RunError::Msg)?;
    }

    // 1.1 clash 手机配置生成与扫码导入
    if let Command::Clash { token, lan_ip, port, url_only } = &cli.command {
        let cfg = config::load().unwrap_or_default();
        return cmd::clash::run(&cfg, token.as_deref(), lan_ip.as_deref(), *port, *url_only)
            .map_err(RunError::Msg);
    }

    // 2. user 用户管理
    if let Command::User { cmd } = &cli.command {
        return match cmd {
            UserCmd::Add { username, password, expires_days, quota, max_conns } => {
                cmd::user::add(username, password.as_deref(), *expires_days, quota.as_deref(), *max_conns).map_err(RunError::Msg)
            }
            UserCmd::Keygen => cmd::user::keygen().map_err(RunError::Msg),
            UserCmd::List => cmd::user::list().map_err(RunError::Msg),
            UserCmd::Rm { username } => cmd::user::rm(username).map_err(RunError::Msg),
            UserCmd::Disable { username } => cmd::user::disable(username).map_err(RunError::Msg),
            UserCmd::Enable { username } => cmd::user::enable(username).map_err(RunError::Msg),
            UserCmd::Passwd { username, password } => {
                cmd::user::passwd(username, password).map_err(RunError::Msg)
            }
            UserCmd::Revoke { target } => {
                cmd::user::revoke(target).map_err(RunError::Msg)
            }
        };
    }

    // 2.1 cluster 分布式集群管理
    if let Command::Cluster { cmd } = &cli.command {
        return match cmd {
            ClusterCmd::TokenCreate { seed, valid_minutes } => {
                cmd::cluster::token_create(seed.as_deref(), *valid_minutes).map_err(RunError::Msg)
            }
            ClusterCmd::Join { token, peer, auto_start } => {
                cmd::cluster::join(token, peer.as_deref(), *auto_start).map_err(RunError::Msg)
            }
            ClusterCmd::Status => {
                cmd::cluster::status().map_err(RunError::Msg)
            }
            ClusterCmd::Upgrade { local, minio, r2, sig } => {
                cmd::cluster::upgrade(local.as_deref(), minio.as_deref(), r2.as_deref(), sig.as_deref()).map_err(RunError::Msg)
            }
        };
    }

    // 3. sync 跨端同步
    if let Command::Sync { cmd } = &cli.command {
        return match cmd {
            SyncCmd::Export { passphrase } => {
                cmd::sync::export(passphrase.as_deref()).map_err(RunError::Msg)
            }
            SyncCmd::Import { payload, passphrase } => {
                cmd::sync::import(payload, passphrase.as_deref()).map_err(RunError::Msg)
            }
        };
    }

    // 4. init 优先：交互式或非交互式
    if let Command::Init { server, token, force, interactive } = &cli.command {
        if *interactive || server.is_none() {
            return cmd::init_interactive::run_interactive(*force).map_err(RunError::Msg);
        }
        let server = server.clone().unwrap_or_default();
        return init(&server, token.as_deref(), *force);
    }

    // 4.1 migrate 一键云服务初始化/迁移
    if let Command::Migrate { cmd } = &cli.command {
        let cfg = config::load().unwrap_or_default();
        return match cmd {
            MigrateCmd::Vercel { token, team, project_edge, project_gate, project_dsk, skip_deploy, dry_run } => {
                let opts = cmd::migrate::VercelMigrateOpts {
                    token: token.clone(),
                    team: team.clone(),
                    project_edge: project_edge.clone(),
                    project_gate: project_gate.clone(),
                    project_dsk: project_dsk.clone(),
                    skip_deploy: *skip_deploy,
                    dry_run: *dry_run,
                    ..Default::default()
                };
                cmd::migrate::run_vercel_migration(&opts, &cfg).map_err(RunError::Msg)
            }
            MigrateCmd::Cf { token, account_id, skip_deploy, dry_run } => {
                let opts = cmd::migrate::CfMigrateOpts {
                    token: token.clone(),
                    account_id: account_id.clone(),
                    skip_deploy: *skip_deploy,
                    dry_run: *dry_run,
                };
                cmd::migrate::run_cf_migration(&opts, &cfg).map_err(RunError::Msg)
            }
        };
    }

    // 5. deploy 也需要配置
    if let Command::Deploy { target } = &cli.command {
        let cfg = config::load()?;
        let t = cmd::deploy::Target::from_str(target)
            .ok_or_else(|| RunError::Msg(format!("无效部署目标: {target} — 可选: cf-worker, vercel, gate, all")))?;
        return cmd::deploy::run(t, &cfg).map_err(RunError::Msg);
    }

    // 6. start/stop/restart 服务生命周期控制
    match &cli.command {
        Command::Start => return cmd::service::start().map_err(RunError::Msg),
        Command::Stop => return cmd::service::stop().map_err(RunError::Msg),
        Command::Restart => return cmd::service::restart().map_err(RunError::Msg),
        Command::GateServer { port, host, token_hash, proxy_secret } => {
            let addr: std::net::SocketAddr = format!("{}:{}", host, port)
                .parse()
                .map_err(|e| RunError::Msg(format!("Invalid socket address: {e}")))?;
            let hash = token_hash
                .clone()
                .or_else(|| std::env::var("TUNNEL_TOKEN_HASH").ok())
                .unwrap_or_default();
            let secret = proxy_secret
                .clone()
                .or_else(|| std::env::var("PROXY_SECRET").ok())
                .unwrap_or_default();
            
            println!("🚀 Starting pproxy native Gate Server on {addr}...");
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| RunError::Msg(format!("Runtime build error: {e}")))?
                .block_on(async {
                    pproxy_gate_server::run_server(addr, hash, secret)
                        .await
                        .map_err(|e| RunError::Msg(format!("Gate server error: {e}")))
                })?;
            return Ok(0);
        }
        _ => {}
    }

    // 7. on/off/env 环境代理开关
    match &cli.command {
        Command::On { eval } => {
            return cmd::proxy_env::on(*eval).map_err(RunError::Msg);
        }
        Command::Off { eval, hard } => {
            return cmd::proxy_env::off(*eval, *hard).map_err(RunError::Msg);
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

    // 8. 组装配置与 client
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
            cmd: ConfigCmd::Export { service, route, token, qr },
        } => {
            if service.eq_ignore_ascii_case("clash") && *qr {
                cmd::clash::run(&cfg, token.as_deref(), None, None, false)
            } else {
                cmd::export_cmd::run(&cfg, service, route.as_deref(), token.as_deref())
            }
        }
        Command::Config {
            cmd: ConfigCmd::SetTunnel { gate_url, token },
        } => {
            use std::io::Write;
            let gate_url = match gate_url {
                Some(u) => u.clone(),
                None => {
                    print!("请输入 Gate 端点 URL (默认: wss://vgate.example.com/api/ws,wss://gate.example.com/ws): ");
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    let _ = std::io::stdin().read_line(&mut input);
                    let input = input.trim();
                    if input.is_empty() {
                        "wss://vgate.example.com/api/ws,wss://gate.example.com/ws".to_string()
                    } else {
                        input.to_string()
                    }
                }
            };
            let token = match token {
                Some(t) => t.clone(),
                None => {
                    print!("请输入 隧道认证 Token (gate_xxx): ");
                    let _ = std::io::stdout().flush();
                    let mut input = String::new();
                    let _ = std::io::stdin().read_line(&mut input);
                    input.trim().to_string()
                }
            };
            if token.is_empty() {
                return Err(RunError::Msg("错误: 隧道 Token 不能为空".to_string()));
            }
            config::save_tunnel_config(&gate_url, &token).map_err(RunError::Msg)?;
            println!("\x1b[1;32m✓ 出海隧道配置已成功持久化至 ~/.pony/config.toml 与 ~/.pony/.pproxy.env\x1b[0m");
            println!("  Gate 端点: {}", gate_url);
            println!("  隧道令牌:  {}", config::redact(&token));
            println!();
            println!("\x1b[1;36m💡 请运行 pproxy on 开启代理，所有终端流量将通过出海隧道畅通访问！\x1b[0m");
            return Ok(EXIT_OK);
        }
        Command::Serve { .. }
        | Command::HaForwarder { .. }
        | Command::Clash { .. }
        | Command::User { .. }
        | Command::Sync { .. }
        | Command::Cluster { .. }
        | Command::Init { .. }
        | Command::Deploy { .. }
        | Command::Start
        | Command::Stop
        | Command::Restart
        | Command::On { .. }
        | Command::Off { .. }
        | Command::Env { .. }
        | Command::Upgrade { .. }
        | Command::GateServer { .. }
        | Command::Migrate { .. } => {
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
        tunnel_gate_url: None,
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
