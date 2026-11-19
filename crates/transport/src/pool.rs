//! 待命隧道连接池（TunnelPool）
//!
//! 在后台预先建立跨洲 WebSocket 待命会话（完成 TCP+TLS+WS Upgrade），
//! 真实请求到来时仅需发送 1 次首帧 JSON 声明目标（1 RTT），彻底消除 4~5 RTT 的冷建连开销。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::proto::{connect_ws_with_timeout, WsSink, WsStream, PREWARM_DIAL_TIMEOUT};
use crate::route::{classify_egress, is_native_vps_endpoint, Egress};

// 默认生产连接生命周期与补给频率（Gate 等待首帧为 60s，客户端 TTL 设为 25s 确保绝不命中对端超时）
const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(25);
const DEFAULT_REFILL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_REFILL_BACKOFF: Duration = Duration::from_secs(5);

/// 待命 WS 会话
pub struct IdleSession {
    pub tx: WsSink,
    pub rx: WsStream,
    pub born: tokio::time::Instant,
    /// 建连所用 token 的 sha256 前 8 hex（仅用于新旧区分，非安全用途）。
    /// `checkout_with_fp` 失配即视为 miss，堵自愈 send→清池竞速窗内的旧池命中。
    pub token_fp8: String,
}

/// 池可观测计数（取证用：命中率/过期/熔断）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub idle: usize,
    pub hits: u64,
    pub misses: u64,
    pub expired: u64,
    pub fused401: u64,
}

/// token 指纹：sha256 前 8 hex 小写（与桌面端 `token_fp8` 同口径，只打指纹禁明文）。
pub fn token_fp8_of(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(token.as_bytes())[..4])
}

/// 待命隧道连接池
pub struct TunnelPool {
    /// 按端点 URL 分组的待命会话队列
    idle: Mutex<HashMap<String, Vec<IdleSession>>>,
    /// 每个端点维持的目标待命会话数
    size: usize,
    /// 动态配置订阅通道
    tunnel_watch: tokio::sync::Mutex<watch::Receiver<(Option<String>, Option<String>)>>,
    /// checkout 取出后即时唤醒后台补给（常态 refill 用；401 熔断期间不监听它，防击穿）
    notify: tokio::sync::Notify,
    /// 启动标记，确保后台维护任务幂等唯一
    started: AtomicBool,
    /// 待命会话空闲存活时长
    idle_ttl: Duration,
    /// 常态补给检测间隔
    refill_interval: Duration,
    /// 补货提前量：剩余寿命已不足该值的会话不再计入“可用”，提前补建。
    /// 取值 `2 × refill_interval`，并夹在 `idle_ttl / 2` 以内——节拍与 TTL 相当时，
    /// 无上限的提前量会让每条会话一出生就“不可用”，退化成自激补池。
    refill_lead: Duration,
    /// 失败退避间隔
    refill_backoff: Duration,
    /// 可观测计数
    hits: AtomicU64,
    misses: AtomicU64,
    expired: AtomicU64,
    fused401: AtomicU64,
}

impl std::fmt::Debug for TunnelPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelPool")
            .field("size_per_endpoint", &self.size)
            .field("idle_ttl", &self.idle_ttl)
            .finish()
    }
}

impl TunnelPool {
    pub fn new(tunnel_watch: watch::Receiver<(Option<String>, Option<String>)>) -> Arc<Self> {
        Self::with_size(tunnel_watch, 2)
    }

    pub fn with_size(
        tunnel_watch: watch::Receiver<(Option<String>, Option<String>)>,
        size: usize,
    ) -> Arc<Self> {
        Self::with_timing(
            tunnel_watch,
            size,
            DEFAULT_IDLE_TTL,
            DEFAULT_REFILL_INTERVAL,
            DEFAULT_REFILL_BACKOFF,
        )
    }

    pub fn with_timing(
        tunnel_watch: watch::Receiver<(Option<String>, Option<String>)>,
        size: usize,
        idle_ttl: Duration,
        refill_interval: Duration,
        refill_backoff: Duration,
    ) -> Arc<Self> {
        // 提前一个补给周期（×2 留一次错过的余量）开始补建，保证新会话在旧会话
        // 过期前就位；上界 idle_ttl/2 防节拍与 TTL 相当时退化成自激补池。
        let refill_lead = (refill_interval * 2).min(idle_ttl / 2);
        Arc::new(Self {
            idle: Mutex::new(HashMap::new()),
            // size=0 即"禁池化"（server main.rs 的 PPROXY_TUNNEL_POOL=0 回滚开关）：
            // 此前是 size.max(1)，该开关名不副实——仍会为每个端点预建 1 条待命会话。
            size,
            tunnel_watch: tokio::sync::Mutex::new(tunnel_watch),
            notify: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
            idle_ttl,
            refill_interval,
            refill_lead,
            refill_backoff,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            fused401: AtomicU64::new(0),
        })
    }

    /// 启动后台维护任务（幂等），返回 JoinHandle
    pub fn start_maintain(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        if self.started.swap(true, Ordering::SeqCst) {
            return None;
        }
        let weak = Arc::downgrade(self);
        Some(tokio::spawn(Self::maintain(weak)))
    }

    /// 按端点优先级取出一条未过期待命会话；过期会话自动丢弃并唤醒补给。
    /// `expected_fp8` 为 Some 时只返回同指纹会话（失配视为 miss，不消耗旧会话），
    /// 堵自愈 send→清池竞速窗内的旧池命中；None 表示不校验（测试/兼容入口）。
    pub fn checkout(
        &self,
        ordered_urls: &[&str],
        expected_fp8: Option<&str>,
    ) -> Option<(WsSink, WsStream, String)> {
        let mut guard = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        let mut popped_any = false;
        // 失配（旧指纹）会话暂存，循环结束后原 key 放回，避免 pop/push 原地打转
        let mut stashed: Vec<(String, IdleSession)> = Vec::new();
        let mut result: Option<(WsSink, WsStream, String)> = None;
        for u in ordered_urls {
            if result.is_some() {
                break;
            }
            if let Some(vec) = guard.get_mut(*u) {
                while let Some(s) = vec.pop() {
                    popped_any = true;
                    if s.born.elapsed() >= self.idle_ttl {
                        self.expired.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    if let Some(fp) = expected_fp8 {
                        if s.token_fp8 != fp {
                            stashed.push(((*u).to_string(), s));
                            continue;
                        }
                    }
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    result = Some((s.tx, s.rx, (*u).to_string()));
                    break;
                }
            }
        }
        for (key, s) in stashed.into_iter().rev() {
            // 逆序放回：stash 是 pop 序，原样逆序 push 才保住同 key 内 LIFO 相对顺序（B-P2-2）
            guard.entry(key).or_default().push(s);
        }
        if result.is_some() {
            drop(guard);
            self.notify.notify_one();
            return result;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        if popped_any {
            self.notify.notify_one();
        }
        None
    }

    /// 同步排空全部待命会话并唤醒补给（自愈 send 后调用，堵竞速窗）。
    pub fn invalidate(&self) {
        self.idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.notify.notify_one();
    }

    /// 可观测计数快照（取证用）。
    pub fn pool_stats(&self) -> PoolStats {
        PoolStats {
            idle: self.idle_total(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
            fused401: self.fused401.load(Ordering::Relaxed),
        }
    }

    /// 当前池内会话总数（监控与断言用）
    pub fn idle_total(&self) -> usize {
        self.idle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|v| v.len())
            .sum()
    }

    /// 会话剩余寿命（已过期返回 zero）。
    fn remaining(&self, s: &IdleSession) -> Duration {
        self.idle_ttl.saturating_sub(s.born.elapsed())
    }

    /// 计入目标容量的会话数：剩余寿命必须大于 `refill_lead`。
    ///
    /// 旧口径按“未过期”计数，缺口 `target - 未过期数` 在过期前一刻恒为 0，
    /// 于是补建只在过期**之后**才开始 → 过期瞬间池见底、直到下一轮 refill 才恢复，
    /// 窗口内到达的请求 100% 冷建连（生产实测 `pooled=false` 占 3%）。
    fn available_count(&self, sessions: &[IdleSession]) -> usize {
        sessions
            .iter()
            .filter(|s| self.remaining(s) > self.refill_lead)
            .count()
    }

    /// 同批建连的出生错峰量（第 `index` 条 / 共 `batch` 条）。
    ///
    /// 一轮并行补给建出的会话 `born` 几乎相同 → 整批在同一瞬间过期 → 池成段清空。
    /// 把 `born` 按“可用寿命”均分前移（即视为更老）后，过期点被摊开，稳态退化为
    /// 每次只补 1 条。只允许前移：会话被视为更老只会提前废弃（安全方向）；
    /// 反向偏移会让会话活得比 TTL 更久，越过 Gate 的首帧等待上限。
    fn stagger_offset(&self, index: usize, batch: usize) -> Duration {
        if batch <= 1 {
            return Duration::ZERO;
        }
        let usable = self.idle_ttl.saturating_sub(self.refill_lead);
        usable * (index as u32) / (batch as u32)
    }

    /// 未过期会话数（仅测试：`idle_total` 含已过期但尚未清理的残留）
    #[cfg(test)]
    pub(crate) fn fresh_total(&self) -> usize {
        let g = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        g.values()
            .map(|v| {
                v.iter()
                    .filter(|s| self.remaining(s) > Duration::ZERO)
                    .count()
            })
            .sum()
    }

    /// 各会话年龄升序（仅测试：验证同批建连的出生错峰）
    #[cfg(test)]
    pub(crate) fn ages(&self) -> Vec<Duration> {
        let g = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<Duration> = g
            .values()
            .flat_map(|v| v.iter().map(|s| s.born.elapsed()))
            .collect();
        out.sort_unstable();
        out
    }

    /// 后台补给任务：Weak 自引用防泄漏 + 主动清理过期连接 + Token 变动清池。
    /// 已知取舍（B-P1-3）：401 熔断计数跨端点全局累加——一端持续 401 会连带熔断另一端
    ///（双端同熔最多 60s，多一次冷建连延迟）。按端点隔离需重构计数结构，留待后续专项。
    async fn maintain(weak: std::sync::Weak<Self>) {
        let mut last_key: Option<(Vec<String>, String)> = None;
        // 连续 401 计数：服务端轮换/凭据失效时停建熔断，防 5s 无限打 gate
        let mut consec_401: u32 = 0;
        loop {
            // 1. 若宿主已 drop，干净退出后台协程
            let pool = match weak.upgrade() {
                Some(p) => p,
                None => return,
            };

            // 2. 主动清理过期会话（消除空闲超时后遗留死连接导致的 100% 冷建连穿透）
            {
                let mut g = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                for vec in g.values_mut() {
                    vec.retain(|s| s.born.elapsed() < pool.idle_ttl);
                }
            }

            // 3. 读取当前 watch 配置
            let (url_raw, token) = {
                let rx = pool.tunnel_watch.lock().await;
                let (u, t) = rx.borrow().clone();
                (u, t)
            };

            match (url_raw, token) {
                (Some(url_raw), Some(token)) => {
                    let endpoints: Vec<String> = url_raw
                        .split([',', ';', '\n'])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let current_key = (endpoints.clone(), token.clone());
                    // 端点集合或 Token 变动 → 立即清池重建；401 连续计数同步清零
                    //（轮换后不得把旧凭据的熔断带到新凭据上，A-P1-7）
                    if last_key.as_ref() != Some(&current_key) {
                        pool.idle.lock().unwrap_or_else(|p| p.into_inner()).clear();
                        last_key = Some(current_key);
                        consec_401 = 0;
                    }

                    // 逐端点补足到 target_size（仅 Vercel 出口强制为 0 以防 Fluid compute 持续计费，NativeVps 全力预热；
                    // 若已配置独立原生 VPS，CF Anycast 仅作 Fallback，无需浪费资源预池化避免 Anycast 握手超时拖慢主池）
                    let has_vps = endpoints.iter().any(|ep| is_native_vps_endpoint(ep));
                    let mut healthy = true;
                    // 本轮缺口清单：(端点, 待补条数)。先一次性算清再并发拨号——
                    // 串行 await 时单条慢拨号会阻塞整轮（实测跨境 p99≈7~11s、max≈20s），
                    // 期间池被抽空，后续请求全部退化为冷建连。
                    let mut wants: Vec<(String, usize)> = Vec::new();
                    for ep in &endpoints {
                        let target_size = if classify_egress(ep) == Egress::Vercel {
                            0
                        } else if has_vps && !is_native_vps_endpoint(ep) {
                            0
                        } else {
                            pool.size
                        };
                        let need = {
                            let g = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                            let ready = g.get(ep).map(|v| pool.available_count(v)).unwrap_or(0);
                            target_size.saturating_sub(ready)
                        };
                        if need > 0 {
                            wants.push((ep.clone(), need));
                        }
                    }

                    // 有界并行：并发度取池容量（每端点目标条数），不额外放大对 gate 的压力。
                    // 每条拨号带上“批次内序号/批次长度”，建连落地时据此错开 born（见 stagger_offset）。
                    let concurrency = pool.size.max(1);
                    let mut dials: Vec<(String, usize, usize)> = Vec::new();
                    for (ep, n) in &wants {
                        for i in 0..*n {
                            dials.push((ep.clone(), i, *n));
                        }
                    }
                    let mut stream = futures_util::stream::iter(dials)
                        .map(|(ep, idx, batch)| {
                            let token = token.clone();
                            async move {
                                let r = connect_ws_with_timeout(&ep, &token, PREWARM_DIAL_TIMEOUT)
                                    .await;
                                (ep, idx, batch, r)
                            }
                        })
                        .buffer_unordered(concurrency);

                    let mut fuse_now = false;
                    while let Some((ep, idx, batch, res)) = stream.next().await {
                        match res {
                            Ok((tx, rx)) => {
                                if weak.upgrade().is_none() {
                                    return;
                                }
                                consec_401 = 0;
                                let fp8 = token_fp8_of(&token);
                                let stagger = pool.stagger_offset(idx, batch);
                                pool.idle
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .entry(ep)
                                    .or_default()
                                    .push(IdleSession {
                                        tx,
                                        rx,
                                        born: tokio::time::Instant::now() - stagger,
                                        token_fp8: fp8,
                                    });
                            }
                            Err(e) => {
                                let es = e.to_string();
                                if es.contains(crate::proto::AUTH_401_MARKER) {
                                    consec_401 += 1;
                                    tracing::warn!("tunnel pool refill {ep} failed: {e} (consec_401={consec_401})");
                                    if consec_401 >= 5 {
                                        fuse_now = true;
                                    }
                                } else {
                                    consec_401 = 0;
                                    tracing::warn!("tunnel pool refill {ep} failed: {e}");
                                }
                                healthy = false;
                            }
                        }
                    }
                    drop(stream);

                    if fuse_now {
                        pool.fused401.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "tunnel pool refill fused by 401 x{consec_401}: pause 60s"
                        );
                        // 熔断等待 60s：只听 watch 变化（token/端点轮换）提前醒，
                        // 不听 checkout-notify——持续建连 miss 的唤醒不得击穿熔断，
                        // 否则回到 5s 级重试风暴（A-P0-5 / B-P1-3）。
                        // 熔断期间仍做 TTL 清理（过期会话不滞留），只是不新建连。
                        {
                            let mut g = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                            for vec in g.values_mut() {
                                vec.retain(|s| s.born.elapsed() < pool.idle_ttl);
                            }
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                            _ = async {
                                let mut rx = pool.tunnel_watch.lock().await;
                                let _ = rx.changed().await;
                            } => {}
                        }
                        consec_401 = 0;
                    }

                    let wait = if healthy { pool.refill_interval } else { pool.refill_backoff };
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = pool.notify.notified() => {}
                        _ = async {
                            let mut rx = pool.tunnel_watch.lock().await;
                            let _ = rx.changed().await;
                        } => {}
                    }
                }
                // 未配置隧道：清池并等待配置唤醒
                _ => {
                    let backoff = pool.refill_backoff;
                    pool.idle.lock().unwrap_or_else(|p| p.into_inner()).clear();
                    last_key = None;
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = async {
                            let mut rx = pool.tunnel_watch.lock().await;
                            let _ = rx.changed().await;
                        } => {}
                    }
                }
            }
        }
    }
}
