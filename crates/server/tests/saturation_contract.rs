//! L2-T 功能契约测试（T4a 红相先行，test-agent 冻结，实现者只读不可改）。
//!
//! 契约文件：.dev-team/contracts/2026-10-09-pproxy-accept-loop.md（L2-P 冻结，本文件
//! 全部数值与之一一对应）；基准 .dev-team/nfr-baseline-pproxy-accept-loop.json。
//!
//! 故障现场（Lead 根因诊断）：gateway.rs `serve_data_plane` 在 accept 循环内同步
//! `sem.acquire_owned().await`（上限常量 MAX_CONCURRENT_CONNECTIONS=256）。CONNECT
//! relay 持 permit 至结束且无超时，死隧道占满 256 permit 后 accept 循环停转 →
//! 内核 accept 队列溢出（现场 recv-Q 129/128）→ 新连接挂死（ponyllm 10s connect
//! timeout → 503 upstream_unavailable）。
//!
//! 本文件断言"修复后"契约，当前实现必须红：
//!   C1) 并发上限可经 env `PPROXY_MAX_CONNECTIONS` 注入（契约 §2，默认 256、env>常量、
//!       非法/0 回落 256）；当前实现读常量 256、无 env → 红。
//!   C2) 超过上限的新连接**立即**收到 `503 Service Unavailable` + `x-pproxy-reason:
//!       saturation` + connection: close（契约 §1 fast-fail，终局 0 重试），测试硬界
//!       4000ms = baseline(3000) + margin(1000)（契约冻结，禁止自选阈值）；
//!       accept() 永不因许可获取阻塞（契约 §1 内核队列语义）。
//!   C3) 任一 permit 释放后下一连接立即正常受理（契约 §1 恢复语义）。
//!   C4) fast-fail 事件日志必须含 `reason=saturation / sem_capacity / action=close_503`
//!       （契约 §1 日志条款），测试断言 sem_capacity == 注入值（契约 §2.2，机器可断言
//!       注入真实生效，无需扩展 admin 端点）。
//!   【实现注意】仅把 permit 移入 spawned 任务而不断言拒绝仍不满足 C2（排队=无快速
//!   终止）：必须 try_acquire 失败 → 原地写 503/关闭，不 spawn 不排队（契约 §5 参考）。
//!
//! 确定性设计（零 sleep 概率等待）：内核 accept 队列 FIFO —— 先打开的连接必然先被
//! accept()。先打开 `limit + margin` 个半开连接（不发任何字节，peek 永久阻塞占
//! permit），再打开被测连接，被测连接必然 FIFO 队尾 → 饱和状态确定成立。
//! env 变更进程级共享：本二进制内仅此一个测试触碰 env（ENV_LOCK 防未来并发测试）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use pproxy_server::gateway::{GatewayState, serve_data_plane};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 契约 env 注入名（契约 §2：PPROXY_MAX_CONNECTIONS，默认 256，env > 常量）。
const CONTRACT_ENV_LIMIT: &str = "PPROXY_MAX_CONNECTIONS";
/// 注入的并发上限（契约 §2 测试配方：=2 → 开 3 条并发 → 第 3 条须 503/关闭）。
const INJECTED_LIMIT: usize = 2;
/// 半开连接裕量：`limit + margin` 先入队，保证被测连接 FIFO 队尾（见文件头）。
const SAT_MARGIN: usize = 4;
/// 硬饱和连接数：必须 > 当前硬编码上限 256（修复前）且 > 注入上限（修复后）。
const HARD_SATURATION: usize = 300;
/// 契约测试硬界 ms：baseline(3000, external_call_timeout_ms) + margin(1000) = 4000，
/// 由契约冻结（L2-AT 禁止自选阈值）。
const CONTRACT_BOUND_MS: u64 = 4000;
/// 恢复语义探测的单次探测上界（条件轮询用，非契约断言界）。
const PROBE_BOUND_MS: u64 = 1000;

/// env 变更串行化（同一二进制内多个 env 测试防并发 set_var 竞态）。
/// 容忍 panic 期间持锁导致的毒化（同仓 pool.rs 先例）。
static ENV_LOCK: StdMutex<()> = StdMutex::new(());
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 请求结果分类（真实断言基础，禁止无断言测试）。
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// 收到了带状态码的 HTTP 响应头（携带 head 供 503 内容断言）。
    Served(u16, String),
    /// 响应头之前对端关闭（EOF）。
    Closed,
    /// 上界内无任何终止信号（挂死/排队 —— 契约违约）。
    TimedOut,
}

const GET_ROOT: &[u8] = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

/// ── 契约日志捕获（C4）──────────────────────────────────────────────────────
/// fast-fail 事件行须经 tracing 输出 `reason=saturation / sem_capacity / action=close_503`
/// （契约 §1 日志条款）。测试进程内安装 fmt subscriber 捕获全部日志到共享缓冲，
/// 供 sem_capacity == 注入值的机器断言（契约 §2.2）。
static LOG_BUF: StdMutex<Vec<u8>> = StdMutex::new(Vec::new());
/// 已捕获的日志行数（供断言"至少一条 fast-fail 行"）。
static LOG_LINES: AtomicUsize = AtomicUsize::new(0);

struct BufLogWriter;
impl std::io::Write for BufLogWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        LOG_BUF.lock().unwrap().extend_from_slice(b);
        LOG_LINES.fetch_add(1, Ordering::SeqCst);
        let _ = std::io::stdout().write_all(b); // 保留 cargo test 可见输出
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

fn install_log_capture() {
    use tracing_subscriber::fmt::MakeWriter;
    struct M;
    impl<'a> MakeWriter<'a> for M {
        type Writer = BufLogWriter;
        fn make_writer(&'a self) -> BufLogWriter {
            BufLogWriter
        }
    }
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(M)
        .try_init();
}

/// 从捕获日志中提取 fast-fail 行的 sem_capacity 数值。
/// 容忍 `sem_capacity=2` / `sem_capacity: 2` / `"sem_capacity":2` 等格式。
fn captured_sem_capacity() -> Option<usize> {
    let buf = LOG_BUF.lock().unwrap();
    let s = String::from_utf8_lossy(&buf);
    let mut search = s.as_bytes();
    while let Some(pos) = find_subslice(search, b"sem_capacity") {
        // 扫描字段名后的首个数字串
        let rest = &search[pos + b"sem_capacity".len()..];
        let digits_start = rest
            .iter()
            .position(|c| c.is_ascii_digit())
            .map(|i| &rest[i..])
            .unwrap_or(&[]);
        let digits: String = digits_start
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .map(|c| *c as char)
            .collect();
        if !digits.is_empty() {
            if let Ok(v) = digits.parse::<usize>() {
                return Some(v);
            }
        }
        search = &search[pos + 1..];
    }
    None
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 组装 GatewayState（模式同 connect.rs 测试：临时库 + 空路由表；隧道 None）。
fn gw_state(tag: &str) -> GatewayState {
    let dir = std::env::temp_dir().join(format!(
        "sat-contract-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (store, _) = pproxy_core::Store::open(&dir.join("state.db")).unwrap();
    let store = Arc::new(store);
    GatewayState {
        tokens: Arc::new(pproxy_core::TokenService::new(Arc::clone(&store)).unwrap()),
        edges: Arc::new(std::collections::HashMap::new()),
        routes: Arc::new(
            pproxy_core::RouteTable::new(
                store.clone(),
                Arc::new(std::collections::HashMap::new()),
            )
            .unwrap(),
        ),
        usage: Arc::new(pproxy_core::UsageTracker::new(store)),
        tunnel: None,
    }
}

/// 启动真实 serve_data_plane（黑盒：accept 循环 + Semaphore + hyper 驱动）。
async fn start_gateway(state: GatewayState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_data_plane(listener, state));
    addr
}

/// 打开 n 个半开连接：connect 成功但**不发任何字节**。
/// 服务端 `handle_conn` 的 peek 永久阻塞 → 每个连接持 1 个 permit（修复前/后均如此）。
async fn open_half_open(addr: SocketAddr, n: usize) -> Vec<TcpStream> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(TcpStream::connect(addr).await.unwrap());
    }
    v
}

/// 读取响应头（直到 \r\n\r\n），返回 (status, head)。EOF 返回 None。
async fn read_until_head(c: &mut TcpStream) -> Option<(u16, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = c.read(&mut tmp).await.unwrap();
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf);
            let status: u16 = head
                .split_whitespace()
                .nth(1)
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            return Some((status, head.into_owned()));
        }
    }
}

/// 连接 + 发送请求 + 在 bound 内读取终止信号（超时 → TimedOut）。
async fn exchange(addr: SocketAddr, req: &[u8], bound_ms: u64) -> Outcome {
    let mut c = match TcpStream::connect(addr).await {
        Ok(c) => c,
        Err(e) => panic!("connect to gateway {addr} failed: {e}"),
    };
    c.write_all(req).await.unwrap();
    match tokio::time::timeout(Duration::from_millis(bound_ms), read_until_head(&mut c)).await {
        Ok(Some((s, h))) => Outcome::Served(s, h),
        Ok(None) => Outcome::Closed,
        Err(_) => Outcome::TimedOut,
    }
}

/// 契约断言：饱和 fast-fail 结果必须为 503（含 x-pproxy-reason: saturation）或关闭。
/// 契约 §1：`HTTP/1.1 503 Service Unavailable` + `connection: close` + `x-pproxy-reason: saturation`。
fn assert_saturation_fast_fail(o: Outcome, context: &str) {
    match &o {
        Outcome::Served(503, head) => assert!(
            head.to_ascii_lowercase().contains("x-pproxy-reason: saturation"),
            "{context}: 503 响应必须携带 x-pproxy-reason: saturation（契约 §1），head: {head}"
        ),
        Outcome::Served(s, _) => panic!(
            "{context}: 超上限连接必须收到 503，实际收到 HTTP {s} —— 契约 C1/C2 违约 \
             （当前实现不读 env PPROXY_MAX_CONNECTIONS，上限仍为常量 256，未饱和）"
        ),
        Outcome::Closed => {}
        Outcome::TimedOut => panic!(
            "{context}: 超上限连接在契约硬界 {CONTRACT_BOUND_MS}ms 内未收到 503/关闭 \
             —— accept 循环阻塞在 sem.acquire_owned()（生产故障形态，recv-Q 溢出挂死）"
        ),
    }
}

/// 条件轮询（显式条件轮询，非盲等）：释放饱和后直到新连接被正常受理 (200)。
async fn wait_served(addr: SocketAddr) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match exchange(addr, GET_ROOT, PROBE_BOUND_MS).await {
            Outcome::Served(200, _) => return,
            other => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "恢复语义违约：饱和释放后 accept 未在 5s 内恢复受理新连接，最近结果 {other:?}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

/// env 恢复守卫：测试结束（含 panic）时还原 PPROXY_MAX_CONNECTIONS。
struct EnvRestore(Option<std::ffi::OsString>);
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var(CONTRACT_ENV_LIMIT, v),
            None => std::env::remove_var(CONTRACT_ENV_LIMIT),
        }
    }
}

/// 契约主用例：env 注入饱和 → fast-fail 503/关闭 → 释放后恢复 → 硬饱和不阻塞 accept。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_limit_fast_fail_and_recovery_contract() {
    let _guard = env_lock();
    let prev = std::env::var_os(CONTRACT_ENV_LIMIT);
    std::env::set_var(CONTRACT_ENV_LIMIT, INJECTED_LIMIT.to_string());
    let _restore = EnvRestore(prev);
    install_log_capture();

    let addr = start_gateway(gw_state("c1")).await;

    // ── Phase A：C1+C2（env 注入 + 超上限快速 503/关闭，契约 §2 配方 PPROXY_MAX_CONNECTIONS=2）──
    // FIFO：先入队 limit+margin 个半开连接占满 permit，被测连接必然队尾。
    let held = open_half_open(addr, INJECTED_LIMIT + SAT_MARGIN).await;
    let o = exchange(addr, GET_ROOT, CONTRACT_BOUND_MS).await;
    assert_saturation_fast_fail(o, &format!(
        "Phase A（env {CONTRACT_ENV_LIMIT}={INJECTED_LIMIT}，第 {} 个连接超上限）",
        INJECTED_LIMIT + SAT_MARGIN + 1
    ));
    println!("PASS: over-limit conn fast-fail (503/close) within contract hard bound {CONTRACT_BOUND_MS}ms");

    // ── Phase C4：注入真实生效 —— fast-fail 日志 sem_capacity == 注入值（契约 §2.2）──
    // 当前实现：无 fast-fail 路径（无 reason=saturation 日志）→ 本断言红。
    let cap = captured_sem_capacity();
    assert_eq!(
        cap,
        Some(INJECTED_LIMIT),
        "契约 §2.2 违约: fast-fail 日志须含 sem_capacity == 注入值 {INJECTED_LIMIT} \
         （reason=saturation / action=close_503，契约 §1 日志条款）。实际捕获: {cap:?} \
         红相判据：当前实现无饱和 fast-fail 事件日志（env 注入未实现）"
    );
    println!("PASS: fast-fail log carries sem_capacity={INJECTED_LIMIT} (env injection machine-verified)");

    // ── Phase B：C3（饱和释放后 accept 继续受理，契约 §1 恢复语义）────────────────
    drop(held); // 关闭半开连接 → 服务端任务 EOF → permit 释放（RAII）
    wait_served(addr).await;
    println!("PASS: after permit release, gateway resumes accepting (GET / -> 200)");

    // ── Phase C：硬饱和（>256 连接）下 accept 循环不得停转（契约 §1 内核队列语义）──
    // 当前实现：256 permit 占满后 accept 循环阻塞在 acquire → 新连接无响应 → 红。
    // 修复后：try_acquire 拒绝 → 快速 503/关闭（此处注入上限 2 仍饱和）。
    let held2 = open_half_open(addr, HARD_SATURATION).await;
    let o = exchange(addr, GET_ROOT, CONTRACT_BOUND_MS).await;
    assert_saturation_fast_fail(o, &format!(
        "Phase C（{HARD_SATURATION} 个半开连接占满上限后的新连接）"
    ));
    drop(held2);
    println!("PASS: hard saturation ({HARD_SATURATION} conns) does not block accept loop");
}
