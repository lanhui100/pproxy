//! 待命隧道连接池（TunnelPool）
//!
//! 在后台预先建立跨洲 WebSocket 待命会话（完成 TCP+TLS+WS Upgrade），
//! 真实请求到来时仅需发送 1 次首帧 JSON 声明目标（1 RTT），彻底消除 4~5 RTT 的冷建连开销。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::proto::{connect_ws, WsSink, WsStream};

// 默认生产连接生命周期与补给频率
const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_REFILL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_REFILL_BACKOFF: Duration = Duration::from_secs(5);

/// 待命 WS 会话
pub struct IdleSession {
    pub tx: WsSink,
    pub rx: WsStream,
    pub born: tokio::time::Instant,
}

/// 待命隧道连接池
pub struct TunnelPool {
    /// 按端点 URL 分组的待命会话队列
    idle: Mutex<HashMap<String, Vec<IdleSession>>>,
    /// 每个端点维持的目标待命会话数
    size: usize,
    /// 动态配置订阅通道
    tunnel_watch: tokio::sync::Mutex<watch::Receiver<(Option<String>, Option<String>)>>,
    /// checkout 取出后即时唤醒后台补给
    notify: tokio::sync::Notify,
    /// 启动标记，确保后台维护任务幂等唯一
    started: AtomicBool,
    /// 待命会话空闲存活时长
    idle_ttl: Duration,
    /// 常态补给检测间隔
    refill_interval: Duration,
    /// 失败退避间隔
    refill_backoff: Duration,
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
            refill_backoff,
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

    /// 按端点优先级取出一条未过期待命会话；过期会话自动丢弃并唤醒补给
    pub fn checkout(&self, ordered_urls: &[&str]) -> Option<(WsSink, WsStream, String)> {
        let mut guard = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        let mut popped_any = false;
        for u in ordered_urls {
            if let Some(vec) = guard.get_mut(*u) {
                while let Some(s) = vec.pop() {
                    popped_any = true;
                    if s.born.elapsed() < self.idle_ttl {
                        drop(guard);
                        self.notify.notify_one();
                        return Some((s.tx, s.rx, (*u).to_string()));
                    }
                }
            }
        }
        if popped_any {
            self.notify.notify_one();
        }
        None
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

    /// 后台补给任务：Weak 自引用防泄漏 + 主动清理过期连接 + Token 变动清池
    async fn maintain(weak: std::sync::Weak<Self>) {
        let mut last_key: Option<(Vec<String>, String)> = None;
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
                    // 端点集合或 Token 变动 → 立即清池重建
                    if last_key.as_ref() != Some(&current_key) {
                        pool.idle.lock().unwrap_or_else(|p| p.into_inner()).clear();
                        last_key = Some(current_key);
                    }

                    // 逐端点补足到 size
                    let mut healthy = true;
                    for ep in &endpoints {
                        let need = {
                            let g = pool.idle.lock().unwrap_or_else(|p| p.into_inner());
                            pool.size.saturating_sub(g.get(ep).map(|v| v.len()).unwrap_or(0))
                        };
                        for _ in 0..need {
                            match connect_ws(ep, &token).await {
                                Ok((tx, rx)) => {
                                    if weak.upgrade().is_none() {
                                        return;
                                    }
                                    pool.idle
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .entry(ep.clone())
                                        .or_default()
                                        .push(IdleSession {
                                            tx,
                                            rx,
                                            born: tokio::time::Instant::now(),
                                        });
                                }
                                Err(e) => {
                                    tracing::warn!("tunnel pool refill {ep} failed: {e}");
                                    healthy = false;
                                    break;
                                }
                            }
                        }
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
