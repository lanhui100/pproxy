//! L2-AT 对抗破坏测试（T4a 红相先行，test-agent 冻结，实现者只读不可改）。
//!
//! 契约文件：.dev-team/contracts/2026-10-09-pproxy-accept-loop.md。
//! 依赖契约冻结数值：饱和 fast-fail 测试硬界 4000ms = baseline(3000)+margin(1000)；
//! `PPROXY_MAX_CONNECTIONS` 注入（默认 256，env>常量）。
//!
//! ① 慢速/半开连接占满并发上限（slow-loris：1 字节部分请求 + 静默，hyper 首部
//!    读窗上限 30s 内持 permit）；
//! ② 超大畸形首行/请求头（垃圾首行 64KB、单行请求头 1MB、超长 CONNECT 头 >16KB）
//!    —— 断言确定终止（4xx/关闭）且不挂死、不占满上限；
//! ③ 确定性竞态（rev2 重冻结，2026-10-09）：固定种子 LCG（确定性抖动）+
//!    tokio::sync::Barrier 同步起跑 + 固定迭代次数，禁止纯 sleep 概率等待。
//!
//! 红相判据（未修复实现）：
//!   AT-1：env=2 未生效（上限恒 256）→ 超限连接被服务（200）非 503 → 红；
//!   AT-3 逾限轮：半开占满未生效 → 6 条并发全部被服务 → served>0 断言红。
//!   AT-2 原生实现已满足（hyper/handle_conn 原生限界终止）—— 属回归守护，绿不破坏
//!   套件红相（AT-1/AT-3 红）。
//!
//! AT-3 rev1→rev2 重冻结记录（executor-pproxy 机器证据 6 跑 3 败，非实现缺陷）：
//!   rev1 缺陷①：逾限轮断言 served<=上限 —— 并发上限约束的是"同时持 permit 数"而
//!     非"已完成请求数"；客户端读完 head 即 drop → 服务端任务 EOF → permit 归还 →
//!     后续连接合法 200（六跑中 served=3×2 / served=2×4 → 概率断言，FLAKY）。
//!   rev1 缺陷②：轮间无同步 —— 上一轮 keep-alive 连接 permit 未释放即起跑下一轮 →
//!     轮内连接偶发 503（run4）。
//!   rev2 确定性方案（本文件现态）：
//!     a) AT-3 全部请求带 `Connection: close` 并读到 EOF 才算完成 —— 服务端
//!        serve_connection 结束（响应后关闭连接）必然释放 permit，客户端读到 EOF
//!        即观测到"该连接 permit 已释放"，天然消除 keep-alive 滞留窗口；所有 handle
//!        join 后 sem 必然全空 → 下一轮起跑恒有满容量（无需轮询同步）。
//!     b) 逾限轮先用 open_half_open(上限) 的静默半开连接占满 sem（peek 永久阻塞、
//!        永不 EOF → permits 恒满），再 Barrier 齐发 OVER_BURST 真实请求 → 全部
//!        503/closed、served(200)==0 确定成立（FIFO：半开先行入队，真实请求队尾）。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use pproxy_server::gateway::{GatewayState, serve_data_plane};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Barrier;

/// 契约注入 env（契约 §2：PPROXY_MAX_CONNECTIONS）。
const CONTRACT_ENV_LIMIT: &str = "PPROXY_MAX_CONNECTIONS";
/// 注入上限（契约 §2 测试配方 =2）。
const INJECTED_LIMIT: usize = 2;
/// 饱和裕量（FIFO 队尾确定性，见饱和契约测试文件头）。
const SAT_MARGIN: usize = 4;
/// 契约测试硬界 ms（baseline 3000 + margin 1000，契约冻结）。
const CONTRACT_BOUND_MS: u64 = 4000;
/// 竞态轮次与并发规模（固定迭代，禁止随机次数）。
const RACE_ROUNDS: usize = 3;
const IN_BURST: usize = 2; // 注：与注入上限一致（轮内全部应被服务 200）
const OVER_BURST: usize = 6; // > 注入上限 2，须出现 fast-fail

static ENV_LOCK: StdMutex<()> = StdMutex::new(());
/// 容忍 panic 期间持锁导致的毒化（同仓 pool.rs 先例）：测试断言失败即 panic，
/// 若另一并发测试随后 lock 需 recover。
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Served(u16, String),
    Closed,
    TimedOut,
}

const GET_ROOT: &[u8] = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

fn gw_state(tag: &str) -> GatewayState {
    let dir = std::env::temp_dir().join(format!(
        "adv-{}-{tag}-{}",
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

async fn start_gateway(state: GatewayState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_data_plane(listener, state));
    addr
}

async fn exchange(addr: SocketAddr, req: &[u8], bound_ms: u64) -> Outcome {
    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let read_res = tokio::time::timeout(Duration::from_millis(bound_ms), async {
        loop {
            let n = c.read(&mut tmp).await.unwrap();
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf).into_owned();
                let status: u16 = head.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
                return Some((status, head));
            }
        }
    })
    .await;
    match read_res {
        Ok(Some((s, h))) => Outcome::Served(s, h),
        Ok(None) => Outcome::Closed,
        Err(_) => Outcome::TimedOut,
    }
}

/// 打开 n 个半开连接：connect 成功但**不发任何字节**（peek 永久阻塞 → 恒持 permit 且
/// 永不 EOF）。AT-3 逾限轮用其占满 sem（FIFO：先行入队，真实请求必然队尾）。
async fn open_half_open(addr: SocketAddr, n: usize) -> Vec<TcpStream> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(TcpStream::connect(addr).await.unwrap());
    }
    v
}

/// 确定性交换（AT-3 专用）：请求带 `Connection: close`，**读到 EOF 才算完成**。
/// 对端完全 close ⇒ 服务端 `serve_connection` 已结束 ⇒ 该连接 permit 必已释放
/// （消除 keep-alive 滞留窗口 = Lead 裁决"对端完全 close"确认的确定性实现）。
/// 503 fast-fail 路径按契约 §1 亦为 close 后断开 → 同样读到 EOF。
async fn exchange_close(addr: SocketAddr, req: &[u8], bound_ms: u64) -> Outcome {
    let mut c = TcpStream::connect(addr).await.unwrap();
    c.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let read_res = tokio::time::timeout(Duration::from_millis(bound_ms), async {
        loop {
            let n = c.read(&mut tmp).await.unwrap();
            if n == 0 {
                // 有头则解析状态，无头 = 直接关闭
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let status: u16 = head.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
                    return Some((status, head));
                }
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    })
    .await;
    match read_res {
        Ok(Some((s, h))) => Outcome::Served(s, h),
        Ok(None) => Outcome::Closed,
        Err(_) => Outcome::TimedOut,
    }
}

/// 固定种子 LCG（确定性抖动源，竞态测试专用，禁止 rand 不确定性）。
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    /// 返回 [0, span) 内的确定伪随机数。
    fn next_in(&mut self, span: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize % span
    }
}

/// ① 慢速/半开（slow-loris）连接占满并发上限：1 字节部分请求行 + 静默。
/// 服务端 handle_conn peek 读到 1 字节 → hyper http1 首部读窗（30s）内持 permit。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_partial_request_connections_saturate_and_over_limit_fast_fails() {
    let _guard = env_lock();
    let prev = std::env::var_os(CONTRACT_ENV_LIMIT);
    std::env::set_var(CONTRACT_ENV_LIMIT, INJECTED_LIMIT.to_string());
    struct EnvRestore(Option<std::ffi::OsString>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var(CONTRACT_ENV_LIMIT, v),
                None => std::env::remove_var(CONTRACT_ENV_LIMIT),
            }
        }
    }
    let _restore = EnvRestore(prev);

    let addr = start_gateway(gw_state("at1")).await;

    // slow-loris：每条连接发 1 字节 "G"（部分请求行）后静默 —— 占 1 permit ≥30s。
    let mut held = Vec::new();
    for _ in 0..(INJECTED_LIMIT + SAT_MARGIN) {
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"G").await.unwrap();
        held.push(c);
    }
    let o = exchange(addr, GET_ROOT, CONTRACT_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(503, h) if h.to_ascii_lowercase().contains("x-pproxy-reason: saturation"))
            || matches!(&o, Outcome::Closed),
        "L2-AT ① 违约: slow-loris（{INJECTED_LIMIT}+{SAT_MARGIN} 条 1 字节部分请求连接占满上限）后，\
         新连接必须在 {CONTRACT_BOUND_MS}ms 内收到 503(x-pproxy-reason: saturation) 或关闭，实际 {o:?}。\
         红相判据：当前实现不读 env（上限恒 256，未饱和）→ 新连接被正常服务"
    );
    println!("PASS: slow-loris saturation triggers fast-fail 503/close");
}

/// ② 超大畸形首行/请求头：断言确定终止（4xx/关闭）、不挂死、不占满上限。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_malformed_first_line_and_headers_terminate() {
    let addr = start_gateway(gw_state("at2")).await;

    // 2a) 垃圾首行 64KB（"????"+64KB 'x'）—— hyper 应 400/关闭。
    let mut junk = b"????".to_vec();
    junk.extend(std::iter::repeat(b'x').take(64 * 1024));
    junk.extend_from_slice(b"\r\n\r\n");
    let o = exchange(addr, &junk, CONTRACT_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(s, _) if (400..600).contains(s)) || matches!(&o, Outcome::Closed),
        "L2-AT ②a 违约: 64KB 垃圾首行必须在 {CONTRACT_BOUND_MS}ms 内 4xx/关闭，实际 {o:?}"
    );

    // 2b) 单行请求头 1MB —— 超 hyper http1 max_buf_size 默认值，应 431/400/关闭。
    let mut big = b"GET / HTTP/1.1\r\nX-Big: ".to_vec();
    big.extend(std::iter::repeat(b'z').take(1024 * 1024));
    big.extend_from_slice(b"\r\n\r\n");
    let o = exchange(addr, &big, CONTRACT_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(s, _) if (400..600).contains(s)) || matches!(&o, Outcome::Closed),
        "L2-AT ②b 违约: 1MB 单行请求头须在 {CONTRACT_BOUND_MS}ms 内 4xx/关闭（不得挂死），实际 {o:?}"
    );

    // 2c) CONNECT 超大头部 >16KB —— gateway.rs 首部读窗 16KB 上限须终止（400/403/关闭）。
    let mut cj = b"CONNECT ".to_vec();
    cj.extend(std::iter::repeat(b'a').take(64 * 1024));
    cj.extend_from_slice(b":443 HTTP/1.1\r\n\r\n");
    let o = exchange(addr, &cj, CONTRACT_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(s, _) if (400..600).contains(s)) || matches!(&o, Outcome::Closed),
        "L2-AT ②c 违约: 64KB CONNECT 头部须在 {CONTRACT_BOUND_MS}ms 内 4xx/关闭（16KB 上限），实际 {o:?}"
    );

    // 畸形攻击后服务不受影响（未占满 permit）：正常请求仍 200。
    let o = exchange(addr, GET_ROOT, CONTRACT_BOUND_MS).await;
    assert!(
        matches!(&o, Outcome::Served(200, _)),
        "L2-AT ② 恢复违约: 畸形攻击后正常 GET / 须仍 200, {o:?}"
    );
    println!("PASS: oversized/garbage first-line & headers all terminate, service unaffected");
}

/// ③ 确定性竞态（rev2 重冻结）：固定种子 LCG + Barrier 同步起跑 + 固定迭代次数。
/// 轮内并发（≤ 上限）全部须 200；逾限轮以半开连接占满 sem → 全部 503/关闭、
/// served(200)==0 确定成立。rev1→rev2 缺陷与方案见文件头。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn deterministic_race_fixed_seed_barrier_fixed_iterations() {
    let _guard = env_lock();
    let prev = std::env::var_os(CONTRACT_ENV_LIMIT);
    std::env::set_var(CONTRACT_ENV_LIMIT, INJECTED_LIMIT.to_string());
    struct EnvRestore(Option<std::ffi::OsString>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var(CONTRACT_ENV_LIMIT, v),
                None => std::env::remove_var(CONTRACT_ENV_LIMIT),
            }
        }
    }
    let _restore = EnvRestore(prev);

    let addr = start_gateway(gw_state("at3")).await;
    let mut rng = Lcg::new(0x5EED_2026);

    // 固定迭代轮（rev2-a：Connection: close + 读到 EOF ⇒ handle join 后 sem 必全空，
    // 轮间零滞留窗口，无需轮询即确定性满容量进入下一轮）。
    for round in 0..RACE_ROUNDS {
        let barrier = Arc::new(Barrier::new(IN_BURST));
        let mut handles = Vec::new();
        for _i in 0..IN_BURST {
            let barrier = Arc::clone(&barrier);
            // 确定性抖动：每任务一个固定 padding 长度（LCG 固定种子，无运行时随机）。
            let pad_len = rng.next_in(64) + 1;
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let mut req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Jitter: ".to_vec();
                req.extend(std::iter::repeat(b'j').take(pad_len));
                req.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
                exchange_close(addr, &req, CONTRACT_BOUND_MS).await
            }));
        }
        for (i, h) in handles.into_iter().enumerate() {
            let o = h.await.unwrap();
            assert!(
                matches!(&o, Outcome::Served(200, _)),
                "L2-AT ③ 轮 {round} 违约: 轮内并发(≤上限 {INJECTED_LIMIT}) 第 {i} 条连接必须在 \
                 {CONTRACT_BOUND_MS}ms 内 200（读到 EOF），实际 {o:?}"
            );
        }
        // 所有 handle 已读到 EOF ⇒ 服务端 serve_connection 全结束 ⇒ permit 全释放
        // （Lead 裁决"对端完全 close"确认的确定性实现，非轮询概率）。
    }

    // 逾限轮（rev2-b）：半开连接占满 sem（peek 永久阻塞、永不 EOF → permits 恒满，
    // FIFO 先行入队）→ OVER_BURST 真实请求全部 fast-fail 503/关闭、served==0 确定。
    // 当前（未修复）实现 env 未生效（上限 256，未饱和）→ 全部 200 → served>0 → 红。
    let _holders = open_half_open(addr, INJECTED_LIMIT).await;
    let barrier = Arc::new(Barrier::new(OVER_BURST));
    let mut handles = Vec::new();
    for _ in 0..OVER_BURST {
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".to_vec();
            exchange_close(addr, &req, CONTRACT_BOUND_MS).await
        }));
    }
    let mut served = 0usize;
    let mut fast_failed = 0usize;
    for h in handles {
        let o = h.await.unwrap();
        match &o {
            Outcome::Served(200, _) => served += 1,
            Outcome::Served(503, head) if head.to_ascii_lowercase().contains("x-pproxy-reason: saturation") => {
                fast_failed += 1;
            }
            Outcome::Closed => fast_failed += 1,
            other => panic!(
                "L2-AT ③ 逾限轮违约: 连接既未 200 也未 503/关闭（挂死/其他）{other:?} —— \
                 所有 {OVER_BURST} 条并发连接须在 {CONTRACT_BOUND_MS}ms 内确定终止"
            ),
        }
    }
    assert_eq!(
        served + fast_failed,
        OVER_BURST,
        "L2-AT ③ 逾限轮违约: 终止连接数 != {OVER_BURST}（served={served}, fast_failed={fast_failed}）"
    );
    assert!(
        fast_failed > 0,
        "L2-AT ③ 逾限轮违约: fast_failed=0 —— 半开占满上限后无任何 fast-fail。\
         红相判据：当前实现不读 env PPROXY_MAX_CONNECTIONS（上限恒 256，未饱和）→ 逾限未被拒绝"
    );
    assert_eq!(
        served, 0,
        "L2-AT ③ 逾限轮违约: 半开占满上限（{INJECTED_LIMIT} 条恒持 permit）后真实请求 served={served} \
         != 0 —— 并发占满未被 fast-fail（红相判据：env 注入未生效，上限恒 256 未饱和）"
    );
    println!(
        "PASS: deterministic race — served={served}, fast_failed={fast_failed} (== OVER_BURST), iterations={RACE_ROUNDS}"
    );
}