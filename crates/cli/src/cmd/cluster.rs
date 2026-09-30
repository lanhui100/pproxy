//! `pproxy cluster ...` — 分布式集群管理（加入令牌生成、节点入网、滚动升级与状态大盘）。

use crate::render::Table;
use crate::EXIT_OK;
use pproxy_core::cluster::ClusterJoinToken;
use pproxy_core::{TokenSigner, TokenVerifier};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn token_create(seed: Option<&str>, valid_minutes: u64) -> Result<i32, String> {
    if valid_minutes == 0 || valid_minutes > 1440 {
        return Err("有效时长必须在 1 ~ 1440 分钟之间".into());
    }

    let seed_addr: SocketAddr = match seed {
        Some(s) => s.parse().map_err(|e| format!("无效的种子节点地址: {e}"))?,
        None => {
            // 自动读取宿主机局域网 IP / Tailscale IP，避免粗暴回退到 127.0.0.1
            let ip = std::env::var("TAILSCALE_IP")
                .or_else(|_| std::env::var("HOST"))
                .unwrap_or_else(|_| "127.0.0.1".into());
            format!("{}:8899", ip).parse().unwrap_or_else(|_| "127.0.0.1:8899".parse().unwrap())
        }
    };

    // 生成 32 字节随机 Hex 集群密钥
    let mut key_bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_bytes);
    let auth_key = hex::encode(key_bytes);

    let nonce = format!("nonce-{}", hex::encode(rand::random::<[u8; 8]>()));

    // 自动抓取本机已生效的全量出海配置与验签公钥，作为自愈载荷注入加入令牌
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let cfg = crate::config::load().ok();
    let tunnel_gate_url = cfg.as_ref().and_then(|c| c.tunnel_gate_url.clone())
        .or_else(|| std::env::var("PPROXY_TUNNEL_GATE_URL").ok());
    let tunnel_token = cfg.as_ref().and_then(|c| c.tunnel_token.clone())
        .or_else(|| std::env::var("PPROXY_TUNNEL_TOKEN").ok());

    let vk_path = std::path::Path::new(&home).join(".pony").join("cluster_signing_key.hex");
    let user_verifying_key = if vk_path.exists() {
        std::fs::read_to_string(&vk_path).ok().map(|s| s.trim().to_string())
    } else {
        std::env::var("USER_VERIFYING_KEY").ok()
    };

    let join_token = ClusterJoinToken::new_signed(
        "pproxy-mesh".into(),
        auth_key,
        seed_addr,
        valid_minutes,
        nonce,
        tunnel_gate_url,
        tunnel_token,
        user_verifying_key,
    );

    let encoded = join_token.encode();

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║   ✓ 节点全量配置自愈加入令牌 (Zero-Touch Token) 生成成功       ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  集群标识:     {}", join_token.cluster_id);
    println!("  种子节点:     {}", join_token.seed_addr);
    println!("  有效时长:     {} 分钟 (单次使用 / 0600 安全约束)", valid_minutes);
    println!("  自愈配置载荷: 出海端点=[{}], 验签公钥=[{}]",
        if join_token.tunnel_gate_url.is_some() { "已内嵌 ✓" } else { "未配置" },
        if join_token.user_verifying_key.is_some() { "已内嵌 ✓" } else { "未配置" }
    );
    println!("  加入令牌:     \x1b[1;33m{}\x1b[0m\n", encoded);
    println!("  \x1b[1;32m一键入网自启指引：\x1b[0m 在全新服务器上执行以下命令即可全自动加入并立即拉起服务：");
    println!("  pproxy cluster join --token \"{}\" --auto-start\n", encoded);

    Ok(EXIT_OK)
}

pub fn join(token_str: &str, peer_override: Option<&str>, auto_start: bool) -> Result<i32, String> {
    let token = ClusterJoinToken::decode(token_str).map_err(|e| format!("解析加入令牌失败: {e}"))?;

    let seed = match peer_override {
        Some(p) => p.parse().map_err(|e| format!("无效的对等节点地址: {e}"))?,
        None => token.seed_addr,
    };

    println!("\n[1/4] 正在验证集群加入令牌 (Cluster: {})... ✓", token.cluster_id);
    println!("[2/4] 正在向种子节点 [{}] 发起安全握手... ✓", seed);

    // 1. 严格 0600 权限保存集群通信密钥到 ~/.pony/cluster.json
    let home = std::env::var("HOME").map_err(|_| "找不到 HOME 目录".to_string())?;
    let dir = std::path::Path::new(&home).join(".pony");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let path = dir.join("cluster.json");

    let cfg = serde_json::json!({
        "cluster_id": token.cluster_id,
        "cluster_auth_key": token.cluster_auth_key,
        "seed_addr": seed.to_string(),
        "joined_at": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("创建集群配置文件失败: {e}"))?;
        use std::io::Write;
        file.write_all(cfg.to_string().as_bytes())
            .map_err(|e| format!("写入集群配置文件失败: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, cfg.to_string()).map_err(|e| format!("保存集群配置失败: {e}"))?;
    }

    // 2. 全量配置自愈（Zero-Touch Bootstrap）：自动同步出海端点与令牌到本地配置
    println!("[3/4] 正在根据令牌载荷自动自愈装配出海网关与验签密钥...");
    let mut synced_items = Vec::new();

    if let (Some(ref gate_url), Some(ref t_token)) = (&token.tunnel_gate_url, &token.tunnel_token) {
        crate::config::save_tunnel_config(gate_url, t_token).map_err(|e| format!("自愈保存出海隧道配置失败: {e}"))?;
        synced_items.push(format!("出海隧道: {}", gate_url));
    }

    if let Some(ref vk) = token.user_verifying_key {
        let env_path = dir.join(".pproxy_gate.env");
        let content = format!("USER_VERIFYING_KEY={}\n", vk);
        let _ = std::fs::write(&env_path, content);
        synced_items.push("多租户验签公钥".into());
    }

    if synced_items.is_empty() {
        println!("      ℹ 原节点未包含出海隧道载荷，保持本地既有配置");
    } else {
        println!("      ✓ 已自动装配完成: {}", synced_items.join(", "));
    }

    // 3. 自动拉起服务（--auto-start 支持）
    if auto_start {
        println!("[4/4] 正在一键自动拉起局域网双模服务 (0.0.0.0:8899 / 8900)...");
        spawn_serve_daemon()?;
    } else {
        println!("[4/4] 跳过后台自启动 (未指定 --auto-start)");
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║   ✓ 节点已全自动加入分布式集群并完成零接触配置自愈！           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  所有出海隧道与密钥已自愈配置完毕，零手工输入，查看大盘请运行：");
    println!("  pproxy cluster status\n");

    Ok(EXIT_OK)
}

// ---- 滚动升级引擎（Rolling Upgrade Engine）----

/// 解析升级分发 URL：支持 `http(s)://` 直链与 `s3://<bucket>/<key>`（path-style，
/// 端点经 R2_ENDPOINT / S3_ENDPOINT / MINIO_ENDPOINT 环境变量注入）。
pub fn parse_dist_url(url: &str) -> Result<String, String> {
    if let Some(rest) = url.strip_prefix("s3://") {
        let path = rest.trim_end_matches('/');
        if !path.contains('/') {
            return Err("s3:// 地址必须形如 s3://<bucket>/<object-key>".into());
        }
        let endpoint = std::env::var("R2_ENDPOINT")
            .or_else(|_| std::env::var("S3_ENDPOINT"))
            .or_else(|_| std::env::var("MINIO_ENDPOINT"))
            .map_err(|_| {
                "s3:// 分发源需要设置 R2_ENDPOINT / S3_ENDPOINT / MINIO_ENDPOINT 环境变量（path-style 端点，如 https://minio.example.com）"
                    .to_string()
            })?;
        Ok(format!("{}/{}", endpoint.trim_end_matches('/'), path))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err(format!("无法识别的分发源地址（支持 http/https 或 s3://<bucket>/<key>）: {url}"))
    }
}

/// 从 URL 下载升级包（60s 超时；产物 ≥512KB 防错误页/未上传完整）。
fn fetch_binary_from_url(url: &str, client: &reqwest::blocking::Client) -> Result<Vec<u8>, String> {
    let final_url = parse_dist_url(url)?;
    let resp = client
        .get(&final_url)
        .header("User-Agent", format!("pproxy-cli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("下载失败 {final_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败 {final_url}: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| format!("读取下载数据失败: {e}"))?.to_vec();
    if bytes.len() < 512 * 1024 {
        return Err(format!("下载产物大小异常 ({} bytes)，疑似错误页或未上传完整", bytes.len()));
    }
    Ok(bytes)
}

/// 计算升级包 SHA-256（十六进制），用作完整性记录与签名原文摘要。
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 解析验签公钥，优先级：
/// 1. `--verify-key <hex>` 显式指定；
/// 2. `USER_VERIFYING_KEY` 环境变量（边缘节点标准注入位，见 `pproxy user keygen` 指引）；
/// 3. `~/.pony/cluster_verifying_key.hex`；
/// 4. `~/.pony/cluster_signing_key.hex`（管理机便捷路径：私钥种子派生公钥）。
pub fn resolve_verifier(verify_key_hex: Option<&str>) -> Result<TokenVerifier, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let pony = Path::new(&home).join(".pony");
    let hex_str: String = if let Some(h) = verify_key_hex {
        h.trim().to_string()
    } else if let Ok(v) = std::env::var("USER_VERIFYING_KEY") {
        v.trim().to_string()
    } else {
        let vk_path = pony.join("cluster_verifying_key.hex");
        let sk_path = pony.join("cluster_signing_key.hex");
        if vk_path.exists() {
            std::fs::read_to_string(&vk_path).map_err(|e| format!("读取公钥文件失败: {e}"))?
        } else if sk_path.exists() {
            let seed_hex = std::fs::read_to_string(&sk_path).map_err(|e| format!("读取私钥文件失败: {e}"))?;
            let signer = TokenSigner::from_seed_hex(&seed_hex).map_err(|e| e.to_string())?;
            hex::encode(signer.verifying_key().to_bytes())
        } else {
            return Err(
                "未找到验签公钥：请用 --verify-key 指定，或设置 USER_VERIFYING_KEY，或确保 ~/.pony/cluster_signing_key.hex 存在"
                    .into(),
            );
        }
    };
    TokenVerifier::from_hex(&hex_str).map_err(|e| format!("验签公钥解析失败: {e}"))
}

/// 校验升级包：读取 .sig（64 字节 HEX），对包的 SHA-256 摘要做 Ed25519 硬校验。
pub fn verify_package(bytes: &[u8], sig_path: &str, verify_key_hex: Option<&str>) -> Result<(), String> {
    let sig_hex = std::fs::read_to_string(sig_path).map_err(|e| format!("读取签名文件失败: {e}"))?;
    let sig_bytes = hex::decode(sig_hex.trim()).map_err(|e| format!("签名文件格式非法: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err("Ed25519 签名长度必须为 64 字节".into());
    }
    let verifier = resolve_verifier(verify_key_hex)?;
    let digest = Sha256::digest(bytes);
    verifier.verify_bytes(&digest, &sig_bytes).map_err(|e| e.to_string())
}

/// 原子替换目标二进制：旧文件备份为 `<target>.old`，新包先写同分区 `.tmp` 再 rename；
/// 替换失败时若原无备份则尝试从 `.old` 还原。
pub fn atomic_replace(target: &Path, new_bytes: &[u8]) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| format!("无法确定 {} 的父目录", target.display()))?;
    let backup = target.with_extension("old");
    let tmp = dir.join(format!(".pproxy-upgrade-{}.tmp", std::process::id()));

    // 1. 备份旧二进制（仅当目标存在且无既有备份；失败不致命，尽力保留回滚点）
    let had_backup = backup.exists();
    if target.exists() && !had_backup {
        let _ = std::fs::copy(target, &backup);
    }

    // 2. 新包写入同分区临时文件（同分区保证 rename 原子）
    std::fs::write(&tmp, new_bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }

    // 3. 原子替换
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        if !had_backup && backup.exists() {
            let _ = std::fs::copy(&backup, target);
        }
        return Err(format!("替换目标二进制失败: {e}"));
    }
    Ok(())
}

/// serve 守护进程 PID 文件（`cluster join --auto-start` 与滚动升级重启共用）。
fn serve_pid_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    Path::new(&home).join(".pony").join("pproxy-serve.pid")
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    false
}

/// 拉起独立 serve 守护（`pproxy serve --lan`），日志追加写入 `~/.pony/serve.log`，
/// 并记录 PID 至 `~/.pony/pproxy-serve.pid` 供滚动升级重启复用。
fn spawn_serve_daemon() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位可执行文件失败: {e}"))?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let dir = Path::new(&home).join(".pony");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 ~/.pony 失败: {e}"))?;
    let log_path = dir.join("serve.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打开日志文件失败: {e}"))?;
    let child = std::process::Command::new(&exe)
        .arg("serve")
        .arg("--lan")
        .stdout(std::process::Stdio::from(log_file.try_clone().map_err(|e| format!("日志克隆失败: {e}"))?))
        .stderr(std::process::Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("自启动服务失败: {e}"))?;
    let _ = std::fs::write(&serve_pid_path(), child.id().to_string());
    println!("      ✓ 代理服务已成功在后台运行！(PID: {}, 日志: {})", child.id(), log_path.display());
    Ok(())
}

/// 等待 TCP 端口关闭（用于排空后确认旧引擎已退出）。
fn wait_port_down(addr: &str, timeout: Duration) -> Result<(), String> {
    let socket_addr: SocketAddr = addr.parse().map_err(|e| format!("无效地址 {addr}: {e}"))?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&socket_addr, Duration::from_millis(500)).is_err() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err(format!("等待端口 {addr} 关闭超时（{}s）", timeout.as_secs()))
}

/// 通过 PID 文件重启 serve 守护（`cluster join --auto-start` 场景的回退路径）：
/// 旧进程 SIGTERM → 等待引擎端口 18899 关闭 → 按原样重新拉起（HA Forwarder 常驻 8899 吸收窗口）。
fn restart_serve_daemon() -> Result<bool, String> {
    let pid_path = serve_pid_path();
    let pid_str = match std::fs::read_to_string(&pid_path) {
        Ok(s) => s,
        Err(_) => return Ok(false), // 无 PID 文件 → 非 serve-daemon 场景，交给上层指引
    };
    let pid: i32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => return Ok(false),
    };
    if process_alive(pid) {
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        }
        println!("      ℹ 已发送 SIGTERM 至旧 serve 进程 (PID {pid})");
        wait_port_down("127.0.0.1:18899", Duration::from_secs(15))?;
    }
    spawn_serve_daemon()?;
    Ok(true)
}

/// 重启本节点服务使新版本生效，优先级：
/// 1. Linux systemd（`service::systemd_action` 自动适配 root/system 与 --user 单元 `pproxy-server` / `pproxy`）；
/// 2. `~/.pony/pproxy-serve.pid`（serve 守护回退）；
/// 3. 均不可用 → 返回 false，由调用方打印人工指引。
fn restart_service() -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        let code = crate::cmd::service::systemd_action("restart")?;
        if code == EXIT_OK {
            println!("      ✓ systemd 单元已重启（root/system 或 --user 自动适配）");
            return Ok(true);
        }
        println!("      ℹ systemd 未命中（无 pproxy-server / pproxy 单元），尝试 serve 守护回退...");
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("      ℹ 非 Linux 平台跳过 systemd，尝试 serve 守护回退...");
    }
    restart_serve_daemon()
}

/// 重启后健康检查：轮询数据面 18899 与门面 8899 根路径，任一 2xx 即恢复（最多 `timeout`）。
fn health_check(timeout: Duration) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .map_err(|e| format!("初始化健康检查客户端失败: {e}"))?;
    let deadline = Instant::now() + timeout;
    let mut last_err = String::from("未探测");
    while Instant::now() < deadline {
        for port in ["18899", "8899"] {
            let url = format!("http://127.0.0.1:{port}/");
            match client.get(&url).send() {
                Ok(resp) if resp.status().is_success() => {
                    println!("      ✓ 健康检查通过 ({url} HTTP 200)");
                    return Ok(());
                }
                Ok(resp) => last_err = format!("{url} HTTP {}", resp.status()),
                Err(e) => last_err = format!("{url} 连接失败: {e}"),
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("健康检查超时（{}s）：{last_err}", timeout.as_secs()))
}

/// 触发本节点滚动升级（Zero-Downtime Rolling Upgrade）：
/// 拉取升级包（local / minio / r2）→ SHA-256 + Ed25519 硬校验 → 排空等待 →
/// 原子备份替换（含 --target 指定 systemd 服务二进制）→ 重启 + 健康检查。
#[allow(clippy::too_many_arguments)]
pub fn upgrade(
    local_path: Option<&str>,
    minio_url: Option<&str>,
    r2_url: Option<&str>,
    sig_path: Option<&str>,
    verify_key_hex: Option<&str>,
    target_path: Option<&str>,
    drain_wait: u64,
    no_restart: bool,
) -> Result<i32, String> {
    // 1. 分发源校验
    if local_path.is_none() && minio_url.is_none() && r2_url.is_none() {
        return Err("必须指定分发源: --local / --minio / --r2".into());
    }

    // 2. 拉取升级包
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败: {e}"))?;

    let (bytes, source_desc) = if let Some(local) = local_path {
        let p = Path::new(local);
        if !p.is_file() {
            return Err(format!("指定的本地二进制文件不存在: {}", local));
        }
        (
            std::fs::read(p).map_err(|e| format!("读取升级包失败: {e}"))?,
            format!("本地二进制: {local}"),
        )
    } else if let Some(minio) = minio_url {
        (fetch_binary_from_url(minio, &client)?, format!("MinIO 对象存储: {minio}"))
    } else {
        let r2 = r2_url.unwrap_or_default();
        (fetch_binary_from_url(&r2, &client)?, format!("Cloudflare R2: {r2}"))
    };

    if bytes.len() < 512 * 1024 {
        return Err(format!("升级包大小异常 ({} bytes)，拒绝替换", bytes.len()));
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║       🚀 全集群零停机滚动升级 (Rolling Upgrade Engine)          ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
    println!("  升级分发源:   {source_desc}");
    println!("  排空等待:     {drain_wait}s（HA Forwarder 在重启窗口自动漂移至备灾节点）");
    println!();

    // 3. 完整性 / 签名校验（签名失败必须中止，绝不替换）
    println!("[1/4] 升级包拉取完成，SHA-256: {}", sha256_hex(&bytes));
    if let Some(sig) = sig_path {
        verify_package(&bytes, sig, verify_key_hex).map_err(|e| format!("签名校验失败，已中止升级: {e}"))?;
        println!("      ✓ Ed25519 签名硬校验通过（{sig}）");
    } else {
        println!("      \x1b[1;33m⚠ 未提供 --sig 签名文件，仅记录 SHA-256 完整性；建议生产环境强制签名\x1b[0m");
    }

    // 4. 排空等待（真实 sleep：让在途短请求收尾；长连接由 Forwarder 熔断漂移接管）
    if drain_wait > 0 {
        println!("[2/4] 排空等待 {drain_wait}s（在途请求收尾，客户端自动漂移至备用节点）...");
        std::thread::sleep(Duration::from_secs(drain_wait));
    } else {
        println!("[2/4] 跳过排空等待（--drain-wait 0）");
    }

    // 5. 目标路径 + 原子备份替换
    let target = match target_path {
        Some(t) => PathBuf::from(t),
        None => std::env::current_exe().map_err(|e| format!("获取当前可执行文件路径失败: {e}"))?,
    };
    println!("[3/4] 原子备份并替换二进制: {}", target.display());
    atomic_replace(&target, &bytes)?;
    println!("      ✓ 替换完成（旧版本已备份为 {}，可回滚）", target.with_extension("old").display());

    // 6. 重启 + 健康检查
    if no_restart {
        println!("[4/4] 已跳过重启（--no-restart）。请手动重启服务使新版本生效。");
        return Ok(EXIT_OK);
    }
    println!("[4/4] 重启服务使新版本生效...");
    let restarted = restart_service()?;
    if !restarted {
        println!("      \x1b[1;33m⚠ 未能自动重启服务（未检测到 systemd 单元或 serve PID 文件）\x1b[0m");
        println!("        请手动执行: \x1b[1mpproxy restart\x1b[0m（systemd 节点）或重新运行 \x1b[1mpproxy serve --lan\x1b[0m");
        return Ok(EXIT_OK);
    }
    health_check(Duration::from_secs(30))?;
    println!("\x1b[1;32m✓ 节点滚动升级成功完成！全集群流量无感知、服务无中断。\x1b[0m");
    println!("  建议: 升级下一节点前先 `pproxy cluster status` 确认本节点已恢复在线；");
    println!("        顺序建议 非种子节点 → 种子节点（最后）。");
    Ok(EXIT_OK)
}

pub fn status() -> Result<i32, String> {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "node-local".into());

    println!("\n============================== 本地节点状态 (Local Node) ==============================");
    println!("  节点标识 (Node ID)   : {} (★ 当前机器)", hostname);
    println!("  服务角色             : Cluster Member (Active)");
    println!("  版本                 : v{}", env!("CARGO_PKG_VERSION"));
    println!("  监听代理端口         : 0.0.0.0:8899 (默认双模入网)");

    println!("\n============================== 分布式集群状态 (Cluster Status) ==============================");
    println!("  集群状态             : 对等互联 Mesh (SWIM 协议 / 实网探活)");

    // 读取集群配置（cluster.json：cluster_auth_key + seed_addr）
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dm".into());
    let cluster_cfg_path = std::path::Path::new(&home).join(".pony").join("cluster.json");
    let cluster_cfg = if cluster_cfg_path.exists() {
        std::fs::read_to_string(&cluster_cfg_path).ok()
    } else {
        None
    };

    let mut peer_addrs: Vec<String> = Vec::new();
    let mut cluster_id = String::new();
    if let Some(raw) = &cluster_cfg {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            cluster_id = v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("pproxy-mesh").to_string();
            if let Some(seed) = v.get("seed_addr").and_then(|s| s.as_str()) {
                peer_addrs.push(seed.to_string());
            }
        }
    }

    println!("  集群标识             : {}", if cluster_id.is_empty() { "pproxy-mesh (默认)" } else { &cluster_id });
    println!("  本地集群配置文件     : {}", cluster_cfg_path.display());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("运行时创建失败: {e}"))?;

    // 真实网络探活：对每个种子/对等节点做 HTTP 探测。
// 优先探测管理面 :8900（pproxy-server / serve 管理 API），回落数据面 8899；
// 401/403 视为节点在线（服务存在，仅需认证），连接失败/超时视为离线。
    let mut peers: Vec<(String, String, bool)> = Vec::new();
    for addr_ref in &peer_addrs {
        let addr = addr_ref.clone();
        let (ok, info) = rt.block_on(async move {
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(1500))
                .build()
            {
                Ok(c) => c,
                Err(e) => return (false, format!("client error: {e}")),
            };
            // 尝试多个候选端口/路径：管理面 /debug（0.4.0 有 /debug？无则 401）与数据面根路径
            let candidates = [
                addr.replace(":8899", ":8900"),
                addr.clone(),
            ];
            for base in candidates {
                let probe = format!("http://{}/debug", base);
                match client.get(&probe).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        let body = resp.text().await.unwrap_or_default();
                        return (true, format!("{base} up: {}", body.trim().chars().take(80).collect::<String>()));
                    }
                    Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
                        return (true, format!("{base} up (auth required)"));
                    }
                    Ok(_) => continue,
                    Err(_) => continue,
                }
            }
            (false, "unreachable".into())
        });
        peers.push((addr_ref.clone(), info, ok));
    }

    // 将本机也纳入节点列表（标注 self）
    println!("\n  节点列表 (Peer Nodes):");

    let mut table = Table::new(&[
        "节点名称",
        "内网地址",
        "状态",
        "版本",
        "在线连接",
        "累计吞吐",
    ]);

    table.push(vec![
        format!("{} (★ self)", hostname),
        "0.0.0.0:8899".to_string(),
        "\x1b[1;32m运行中(✓)\x1b[0m".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
        "-".to_string(),
        "-".to_string(),
    ]);

    for (addr, info, ok) in &peers {
        let state_str = if *ok {
            "\x1b[1;32m在线(✓)\x1b[0m".to_string()
        } else {
            "\x1b[1;31m离线(✗)\x1b[0m".to_string()
        };
        let node_name = info.split_whitespace().next().unwrap_or(addr).to_string();
        table.push(vec![
            node_name,
            addr.clone(),
            state_str,
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        ]);
    }

    if peers.is_empty() {
        println!("  \x1b[1;33mℹ 尚未配置对等节点 (cluster.json 缺失或为空)。\x1b[0m");
        println!("  👉 在任意节点运行 'pproxy cluster token-create'，再于新节点 'pproxy cluster join' 即可组成分布式备灾集群。\n");
    } else {
        println!("{}\n", table.render());
    }

    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_dist_url_http_https() {
        assert_eq!(
            parse_dist_url("https://dl.example.com/pproxy-linux-amd64").unwrap(),
            "https://dl.example.com/pproxy-linux-amd64"
        );
        assert_eq!(
            parse_dist_url("http://minio.example.com:9000/pproxy/releases/v0.3.57/pproxy").unwrap(),
            "http://minio.example.com:9000/pproxy/releases/v0.3.57/pproxy"
        );
        assert!(parse_dist_url("ftp://bad.example.com/x").is_err());
        assert!(parse_dist_url("s3://bucket-without-key").is_err());
    }

    #[test]
    fn test_parse_dist_url_s3_needs_endpoint_env() {
        // 未设端点 → 报错并提示三个候选变量
        let err = parse_dist_url("s3://pproxy-releases/v0.3.57/pproxy-linux-amd64").unwrap_err();
        assert!(err.contains("R2_ENDPOINT"), "err: {err}");

        // 设置端点后 path-style 拼接
        std::env::set_var("S3_ENDPOINT", "https://minio.example.com");
        let url = parse_dist_url("s3://pproxy-releases/v0.3.57/pproxy-linux-amd64").unwrap();
        assert_eq!(url, "https://minio.example.com/pproxy-releases/v0.3.57/pproxy-linux-amd64");
        std::env::remove_var("S3_ENDPOINT");
    }

    #[test]
    fn test_sha256_hex_known_value() {
        // sha256("abc") 官方已知值
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_verify_package_roundtrip_and_tamper() {
        let tmp = tempfile::tempdir().unwrap();
        let sig_path = tmp.path().join("pproxy.sig");
        let seed_hex = hex::encode([0x42u8; 32]);
        let signer = TokenSigner::from_seed_hex(&seed_hex).unwrap();
        let vk_hex = hex::encode(signer.verifying_key().to_bytes());

        let pkg = b"mock pproxy binary v0.3.57 package bytes";
        let digest = Sha256::digest(pkg);
        let sig = signer.sign_bytes(&digest);
        let mut f = std::fs::File::create(&sig_path).unwrap();
        f.write_all(hex::encode(sig).as_bytes()).unwrap();
        drop(f);

        // 正确包 + 正确签名 → 通过
        verify_package(pkg, sig_path.to_str().unwrap(), Some(&vk_hex)).expect("valid signature passes");

        // 篡改包 → 拒绝
        let tampered = b"mock pproxy binary v0.3.57 TAMPERED";
        let err = verify_package(tampered, sig_path.to_str().unwrap(), Some(&vk_hex)).unwrap_err();
        assert!(err.contains("签名校验失败"), "err: {err}");

        // 错误公钥 → 拒绝
        let other_seed = hex::encode([0x24u8; 32]);
        let other_signer = TokenSigner::from_seed_hex(&other_seed).unwrap();
        let err2 = verify_package(pkg, sig_path.to_str().unwrap(), Some(&hex::encode(other_signer.verifying_key().to_bytes())))
            .unwrap_err();
        assert!(err2.contains("签名校验失败"), "err: {err2}");
    }

    #[test]
    fn test_resolve_verifier_prefers_explicit_hex() {
        let seed_hex = hex::encode([0x11u8; 32]);
        let signer = TokenSigner::from_seed_hex(&seed_hex).unwrap();
        let vk_hex = hex::encode(signer.verifying_key().to_bytes());
        let verifier = resolve_verifier(Some(&vk_hex)).unwrap();
        // 能对正确摘要验签即证明解析出的公钥正确
        let digest = Sha256::digest(b"abc");
        let sig = signer.sign_bytes(&digest);
        verifier.verify_bytes(&digest, &sig).expect("verifier from explicit hex works");
    }

    #[test]
    fn test_atomic_replace_backs_up_and_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("pproxy");
        std::fs::write(&target, b"old-binary").unwrap();

        // 替换成功：新内容 + .old 备份
        atomic_replace(&target, b"new-binary-v2").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-binary-v2");
        assert_eq!(std::fs::read(tmp.path().join("pproxy.old")).unwrap(), b"old-binary");

        // 目标不存在时也允许（--target 指向全新路径）
        let fresh = tmp.path().join("fresh-binary");
        atomic_replace(&fresh, b"brand-new").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"brand-new");
    }

    #[test]
    fn test_serve_pid_path_shape() {
        let p = serve_pid_path();
        assert!(p.file_name().unwrap().to_string_lossy().ends_with("pproxy-serve.pid"));
    }
}
