use crate::{ProxyRecord, PoolConfig};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct Pool {
    inner: Arc<RwLock<PoolInner>>,
}

#[derive(Debug)]
struct PoolInner {
    proxies: Vec<ProxyRecord>,
    static_upstreams: Vec<String>,
    countries: Vec<String>,
    config: PoolConfig,
    last_refresh: u64,
}

impl Pool {
    pub async fn new(config: PoolConfig) -> Self {
        let inner = PoolInner {
            proxies: Vec::new(),
            static_upstreams: config.static_upstreams.clone(),
            countries: config.countries.clone(),
            config,
            last_refresh: 0,
        };
        let pool = Pool {
            inner: Arc::new(RwLock::new(inner)),
        };
        pool.refresh().await;
        pool
    }

    pub async fn refresh(&self) {
        let config = self.inner.read().await.config.clone();
        let countries = config.countries.clone();

        if countries.is_empty() {
            return;
        }

            match crate::source::fetch_all(&countries).await {
                Ok(candidates) => {
                    let batch: Vec<_> = candidates.into_iter().take(500).collect();
                    info!("fetched {} candidates (took 500), verifying...", batch.len());
                    let verified = crate::check::verify_batch(
                        &batch,
                        config.verify_concurrency,
                    )
                    .await;
                    info!("verified {} proxies", verified.len());

                    let mut inner = self.inner.write().await;
                    inner.proxies = verified;
                    inner.last_refresh = now_secs();
                }
            Err(e) => warn!("refresh failed: {}", e),
        }
    }

    pub async fn next(&self) -> Option<String> {
        // 1. static upstream first
        {
            let inner = self.inner.read().await;
            if !inner.static_upstreams.is_empty() {
                return Some(inner.static_upstreams[0].clone());
            }
        }

        // 2. dynamic pool: random pick by country latency sorted
        let inner = self.inner.read().await;
        if inner.proxies.is_empty() {
            return None;
        }

        // sort by latency, pick from top N randomly
        let mut sorted = inner.proxies.clone();
        sorted.sort_by_key(|p| p.latency_ms.unwrap_or(u64::MAX));
        let n = sorted.len().min(10);
        let idx = rand_index(n);
        Some(sorted[idx].addr())
    }

    pub async fn mark_failed(&self, addr: &str) {
        let mut inner = self.inner.write().await;
        inner.proxies.retain(|p| p.addr() != addr);
    }

    pub async fn stats(&self) -> PoolStats {
        let inner = self.inner.read().await;
        PoolStats {
            dynamic_count: inner.proxies.len(),
            static_count: inner.static_upstreams.len(),
            countries: inner.countries.clone(),
            last_refresh: inner.last_refresh,
        }
    }

    pub async fn keepalive_loop(self) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let config = self.inner.read().await.config.clone();
            let now = now_secs();
            let last = self.inner.read().await.last_refresh;
            if now.saturating_sub(last) > config.pool_ttl_sec {
                info!("ttl expired, refreshing pool");
                self.refresh().await;
            }

            // keepalive: verify pool entries
            let to_verify: Vec<ProxyRecord> = {
                let inner = self.inner.read().await;
                inner.proxies.clone()
            };
            if to_verify.is_empty() {
                continue;
            }

            let config = self.inner.read().await.config.clone();
            let verified = crate::check::verify_batch(
                &to_verify,
                config.verify_concurrency.min(5),
            )
            .await;

            let verified_addrs: std::collections::HashSet<String> =
                verified.iter().map(|p| p.addr()).collect();
            let mut inner = self.inner.write().await;
            inner.proxies.retain(|p| verified_addrs.contains(&p.addr()));
            info!("keepalive: {} proxies alive", inner.proxies.len());

            // low watermark trigger refresh
            if inner.proxies.len() < 3 {
                info!("low watermark hit, triggering emergency refresh");
                drop(inner);
                self.refresh().await;
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    pub dynamic_count: usize,
    pub static_count: usize,
    pub countries: Vec<String>,
    pub last_refresh: u64,
}

#[inline]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[inline]
fn rand_index(n: usize) -> usize {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(now_secs());
    (h.finish() as usize) % n
}
