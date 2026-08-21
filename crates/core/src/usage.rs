//! 用量计数（T5）：内存实时计数 → 小时聚合 UPSERT 落库 → 查询聚合（内存 + 库）。
//!
//! 热路径（record_request / record_bytes_out）无 Mutex、无 IO、无 await：
//! DashMap 分片 + AtomicU64（T5 §3）。
//! 落库原子性：drain-and-swap（裁决 #4，T5 §4）——整表换出后落库，成功丢弃、
//! 失败合并回 live，计数不丢。落库循环由 main.rs spawn（T5 §6，C-P2-3），
//! 本模块不负责。

use crate::store::{hour_floor, now_unix, Store, StoreError, UsageRow};
use dashmap::DashMap;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 一次落库的增量快照（drain 与 merge_back 之间的搬运单位）。
/// Copy：merge_back 的 and_modify 借用与 or_insert_with 按值捕获需共存。
#[derive(Debug, Default, Clone, Copy)]
pub struct UsageDelta {
    pub requests: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// 全模块统一错误：目前仅存储失败（落库/查询经 spawn_blocking 触达 SQLite）。
#[derive(Debug)]
pub enum UsageError {
    Store(StoreError),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "usage store error: {e}"),
        }
    }
}

impl std::error::Error for UsageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
        }
    }
}

impl From<StoreError> for UsageError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// 内层计数器：单 key (route, token_id) 的原子三元组。
#[derive(Default)]
struct Counters {
    requests: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl Counters {
    /// 读取快照（drain 时用）。Relaxed 足够：各字段独立累加，无跨变量不变量。
    fn snapshot(&self) -> UsageDelta {
        UsageDelta {
            requests: self.requests.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
        }
    }

    /// 合并增量（flush 失败 merge_back 用）。
    fn add(&self, d: &UsageDelta) {
        self.requests.fetch_add(d.requests, Ordering::Relaxed);
        self.bytes_in.fetch_add(d.bytes_in, Ordering::Relaxed);
        self.bytes_out.fetch_add(d.bytes_out, Ordering::Relaxed);
    }
}

impl From<UsageDelta> for Counters {
    fn from(d: UsageDelta) -> Self {
        Counters {
            requests: AtomicU64::new(d.requests),
            bytes_in: AtomicU64::new(d.bytes_in),
            bytes_out: AtomicU64::new(d.bytes_out),
        }
    }
}

impl Counters {
    /// UsageDelta → Counters（merge_back vacant 分支用）。
    fn from_delta(d: UsageDelta) -> Self {
        Self::from(d)
    }
}

/// spawn_blocking 的 JoinError（阻塞任务 panic）→ StoreError。
fn join_error(e: tokio::task::JoinError) -> StoreError {
    StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
}

pub struct UsageTracker {
    store: Arc<Store>,
    /// key = (route, token_id)；当前未落库小时的活计数。
    live: DashMap<(String, i64), Counters>,
}

impl UsageTracker {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            live: DashMap::new(),
        }
    }

    /// 请求开始时调用（T3 转发 handler）：requests+1、bytes_in+n。
    pub fn record_request(&self, route: &str, token_id: i64, bytes_in: u64) {
        let c = self.live.entry((route.to_owned(), token_id)).or_default();
        c.requests.fetch_add(1, Ordering::Relaxed);
        c.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
    }

    /// 响应流式期间每 chunk 调用（T3，裁决 #5）：bytes_out+n。
    pub fn record_bytes_out(&self, route: &str, token_id: i64, n: u64) {
        let c = self.live.entry((route.to_owned(), token_id)).or_default();
        c.bytes_out.fetch_add(n, Ordering::Relaxed);
    }

    /// 整表换出（T5 §4.1）：原子移除全部条目并返回快照；换出开始后的并发
    /// entry() 写入会重建条目，不丢失。
    /// WHY retain 而非 DashMap::drain：dashmap 6.2.1 无 drain()（spec 撰写时
    /// 依据的 API 不存在），且 Cargo.toml 禁改无法启用 raw-api。retain 的
    /// 实现持分片写锁遍历——每 key「快照 + 擦除」原子完成，与 drain 语义
    /// 等价：期间并发写要么阻塞至该分片锁释放、要么写入重建的新条目。
    fn drain(&self) -> HashMap<(String, i64), UsageDelta> {
        let mut out = HashMap::new();
        self.live.retain(|k, c| {
            out.insert(k.clone(), c.snapshot());
            false // 全部擦除 → 整表换出
        });
        out
    }

    /// flush 失败时把 drained 合并回 live（T5 §4.5，C-P1-6）。
    /// WHY and_modify 链 or_insert_with：drain 后并发写可能已重建同 key 条目，
    /// 只用 and_modify 会丢弃重建条目中的新计数；or_insert_with 兜底 vacant 态。
    fn merge_back(&self, drained: HashMap<(String, i64), UsageDelta>) {
        for (k, delta) in drained {
            self.live
                .entry(k)
                .and_modify(|c| c.add(&delta))
                .or_insert_with(|| Counters::from_delta(delta));
        }
    }

    /// 落库：drain 当前 live 表 → 单事务 UPSERT → 成功丢弃 / 失败合并回 live，
    /// 返回落库行数。跨小时边界统一取 flush 时刻的 hour_floor（T5 §4.6 裁决：
    /// 内存计数不携带时间，逐条时间戳复杂度不值）。
    pub async fn flush(&self) -> Result<usize, UsageError> {
        let drained = self.drain();
        if drained.is_empty() {
            return Ok(0);
        }
        let ts_hour = hour_floor(now_unix());
        let rows: Vec<UsageRow> = drained
            .iter()
            .map(|((route, token_id), d)| UsageRow {
                ts_hour,
                route: route.clone(),
                token_id: *token_id,
                requests: d.requests,
                bytes_in: d.bytes_in,
                bytes_out: d.bytes_out,
            })
            .collect();
        let n = rows.len();

        let store = Arc::clone(&self.store);
        // C-P1-9：Store 同步签名，经 spawn_blocking 调用；route 借用在此结束
        //（rows 已持有 String 拷贝），闭包满足 'static。
        match tokio::task::spawn_blocking(move || store.upsert_usage(&rows))
            .await
            .map_err(join_error)
        {
            Ok(Ok(())) => Ok(n),
            // 失败一律合并回 live：计数不丢，下轮 flush 重试（裁决 #4）
            Ok(Err(e)) => {
                self.merge_back(drained);
                Err(e.into())
            }
            Err(e) => {
                self.merge_back(drained);
                Err(e.into())
            }
        }
    }

    /// 查询聚合（T5 §7）：库 [since_hour, now] 闭区间（含 since_hour 当小时，
    /// C-P2-11）+ 当前 live 内存，按 (route, token_id) 合并求和。
    /// 聚合行 ts_hour 置 0（无意义，T6 DTO 不透出，C-P2-10）。
    pub async fn query(
        &self,
        since_hour: u64,
        route: Option<&str>,
        token_id: Option<i64>,
    ) -> Result<Vec<UsageRow>, UsageError> {
        let store = Arc::clone(&self.store);
        // spawn_blocking 闭包要求 'static：借用 route 先转 owned
        let route_owned = route.map(str::to_owned);
        let db_rows = tokio::task::spawn_blocking(move || {
            store.query_usage(since_hour, route_owned.as_deref(), token_id)
        })
        .await
        .map_err(join_error)??;

        // BTreeMap：输出按 (route, token_id) 稳定排序，便于 T6 响应断言
        let mut agg: BTreeMap<(String, i64), (u64, u64, u64)> = BTreeMap::new();
        for r in db_rows {
            let slot = agg.entry((r.route, r.token_id)).or_default();
            slot.0 += r.requests;
            slot.1 += r.bytes_in;
            slot.2 += r.bytes_out;
        }
        for entry in self.live.iter() {
            let (k, c) = entry.pair();
            let (live_route, live_token) = k;
            if let Some(f) = route {
                if live_route != f {
                    continue;
                }
            }
            if let Some(t) = token_id {
                if *live_token != t {
                    continue;
                }
            }
            let slot = agg.entry((live_route.clone(), *live_token)).or_default();
            slot.0 += c.requests.load(Ordering::Relaxed);
            slot.1 += c.bytes_in.load(Ordering::Relaxed);
            slot.2 += c.bytes_out.load(Ordering::Relaxed);
        }

        Ok(agg
            .into_iter()
            .map(|((route, token_id), (requests, bytes_in, bytes_out))| UsageRow {
                ts_hour: 0,
                route,
                token_id,
                requests,
                bytes_in,
                bytes_out,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 临时库 + tracker；TempDir 须存活到断言结束。
    fn setup(tag: &str) -> (tempfile::TempDir, Arc<Store>, UsageTracker) {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(&dir.path().join(format!("{tag}.db"))).unwrap();
        let store = Arc::new(store);
        let tracker = UsageTracker::new(Arc::clone(&store));
        (dir, store, tracker)
    }

    fn db_totals(store: &Store) -> (u64, u64, u64) {
        store
            .query_usage(0, None, None)
            .unwrap()
            .iter()
            .fold((0, 0, 0), |acc, r| {
                (acc.0 + r.requests, acc.1 + r.bytes_in, acc.2 + r.bytes_out)
            })
    }

    // ---- §9.1 record → flush → store.query_usage 读回一致 ----
    #[tokio::test]
    async fn flush_persists_and_reads_back() {
        let (_dir, store, tracker) = setup("t1");
        tracker.record_request("anthropic", 1, 100);
        tracker.record_bytes_out("anthropic", 1, 40);
        assert_eq!(tracker.flush().await.unwrap(), 1);
        let rows = store.query_usage(0, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].requests, rows[0].bytes_in, rows[0].bytes_out),
            (1, 100, 40)
        );
    }

    // ---- §9.2 flush 幂等：二次 flush 0 行且库值不翻倍 ----
    #[tokio::test]
    async fn flush_is_idempotent() {
        let (_dir, store, tracker) = setup("t2");
        tracker.record_request("r", 1, 5);
        assert_eq!(tracker.flush().await.unwrap(), 1);
        assert_eq!(tracker.flush().await.unwrap(), 0, "live 已清空，二次 0 行");
        assert_eq!(db_totals(&store), (1, 5, 0), "库值不翻倍");
    }

    // ---- §9.3 并发计数：10 任务 × 1000 次 = 10000 ----
    #[tokio::test]
    async fn concurrent_records_all_counted() {
        let (_dir, store, tracker) = setup("t3");
        let tracker = Arc::new(tracker);
        let handles: Vec<_> = (0..10)
            .map(|t| {
                let tr = Arc::clone(&tracker);
                tokio::spawn(async move {
                    for _ in 0..1000 {
                        tr.record_request("r", t, 1);
                    }
                })
            })
            .collect();
        for h in handles {
            h.await.unwrap();
        }
        tracker.flush().await.unwrap();
        assert_eq!(db_totals(&store).0, 10_000);
    }

    // ---- §9.4 flush 期间并发写不丢不重：多次落库之和 = 总写入量 ----
    #[tokio::test]
    async fn concurrent_writes_during_flush_not_lost() {
        let (_dir, store, tracker) = setup("t4");
        let tracker = Arc::new(tracker);
        const TOTAL: u64 = 50;
        let writer = {
            let tr = Arc::clone(&tracker);
            tokio::spawn(async move {
                for _ in 0..TOTAL {
                    tr.record_request("r", 1, 1);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
        };
        let mut flush_count = 0;
        while !writer.is_finished() {
            tracker.flush().await.unwrap();
            flush_count += 1;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        writer.await.unwrap();
        tracker.flush().await.unwrap();
        assert!(flush_count >= 1, "写入窗口内应至少 flush 一次");
        assert_eq!(db_totals(&store), (TOTAL, TOTAL, 0));
    }

    // ---- §9.5 落库失败回滚：合并回 live（含 drain 后并发写入，C-P1-6）----
    #[tokio::test]
    async fn flush_failure_merges_back_and_retry_succeeds() {
        let (_dir, store, tracker) = setup("t5");
        // WHY 不删 DB 文件：Linux 上 unlink 后已打开的连接仍可写，不会失败；
        // 改为 DROP TABLE 使 upsert 确定性地报 "no such table"。
        store
            .lock_conn()
            .execute_batch("DROP TABLE usage_hourly")
            .unwrap();
        let tracker = Arc::new(tracker);
        tracker.record_request("r", 1, 10);
        tracker.record_bytes_out("r", 1, 5);

        // drain（flush 内第一步，同步）之后的并发写：current_thread 运行时下
        // 该任务在 flush 首个 await 点执行，落在 merge_back 的 occupied 分支
        let writer = {
            let tr = Arc::clone(&tracker);
            tokio::spawn(async move {
                tr.record_request("r", 1, 5);
            })
        };
        let err = tracker.flush().await.unwrap_err();
        assert!(matches!(err, UsageError::Store(_)));
        writer.await.unwrap();
        assert!(!tracker.live.is_empty(), "失败后计数必须合并回 live");
        let live: (u64, u64, u64) = tracker.live.iter().fold((0, 0, 0), |acc, e| {
            let d = e.value().snapshot();
            (acc.0 + d.requests, acc.1 + d.bytes_in, acc.2 + d.bytes_out)
        });
        assert_eq!(live, (2, 15, 5), "原计数 + drain 后并发写入的新计数");

        // 恢复表后重试 flush：总量不丢
        store
            .lock_conn()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS usage_hourly (
                    ts_hour   INTEGER NOT NULL,
                    route     TEXT NOT NULL,
                    token_id  INTEGER NOT NULL,
                    requests  INTEGER NOT NULL,
                    bytes_in  INTEGER NOT NULL,
                    bytes_out INTEGER NOT NULL,
                    PRIMARY KEY (ts_hour, route, token_id)
                )",
            )
            .unwrap();
        tracker.flush().await.unwrap();
        assert_eq!(db_totals(&store), (2, 15, 5));
    }

    // ---- §9.5 merge_back 覆盖 occupied / vacant 两分支 ----
    #[tokio::test]
    async fn merge_back_covers_occupied_and_vacant() {
        let (_dir, _store, tracker) = setup("t5b");
        tracker.record_request("r", 1, 10);
        let drained = tracker.drain();
        assert!(tracker.live.is_empty());

        // vacant：drain 后无并发写，merge_back 重建条目
        tracker.merge_back(drained);
        // WHY 块作用域：Ref 持分片读锁，必须先释放再 drain/merge_back
        //（取写锁会死锁，dashmap 文档明示）
        {
            let c = tracker.live.get(&("r".to_owned(), 1)).unwrap();
            assert_eq!(c.snapshot().requests, 1);
            assert_eq!(c.snapshot().bytes_in, 10);
        }

        // occupied：drain 后并发写重建了同 key 条目，merge_back 累加而非覆盖
        let drained = tracker.drain();
        tracker.record_request("r", 1, 100);
        tracker.merge_back(drained);
        {
            let c = tracker.live.get(&("r".to_owned(), 1)).unwrap();
            assert_eq!(c.snapshot().requests, 2);
            assert_eq!(c.snapshot().bytes_in, 110);
        }
    }

    // ---- §9.6 query 聚合：库 2 行 + 内存 1 行同 key 合并；过滤生效 ----
    #[tokio::test]
    async fn query_merges_db_and_live_with_filters() {
        let (_dir, store, tracker) = setup("t6");
        store
            .upsert_usage(&[
                UsageRow { ts_hour: 0, route: "r".into(), token_id: 1, requests: 1, bytes_in: 10, bytes_out: 0 },
                UsageRow { ts_hour: 3600, route: "r".into(), token_id: 1, requests: 2, bytes_in: 20, bytes_out: 0 },
                UsageRow { ts_hour: 0, route: "r".into(), token_id: 2, requests: 100, bytes_in: 0, bytes_out: 0 },
            ])
            .unwrap();
        tracker.record_request("r", 1, 5);
        tracker.record_bytes_out("r", 1, 7);

        let rows = tracker.query(0, None, None).await.unwrap();
        assert_eq!(rows.len(), 2, "同 key 库 2 行 + 内存 1 份合并为 1 行");
        let r1 = rows.iter().find(|r| r.token_id == 1).unwrap();
        // 库 (1,10,0)+(2,20,0) + live (1,5,7) = (4,35,7)
        assert_eq!((r1.requests, r1.bytes_in, r1.bytes_out), (4, 35, 7));

        // route 过滤
        assert!(tracker.query(0, Some("nope"), None).await.unwrap().is_empty());
        // token_id 过滤
        let rows = tracker.query(0, None, Some(2)).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].requests, 100);
        // since_hour 过滤：ts_hour=0 的库行被排除，live 行仍计入
        let rows = tracker.query(3600, None, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].requests, 3, "库 3600 行(2) + live(1)");
    }

    // ---- §9.7 hour_floor 对齐 ----
    #[tokio::test]
    async fn flush_rows_are_hour_aligned() {
        let (_dir, store, tracker) = setup("t7");
        tracker.record_request("r", 1, 1);
        tracker.flush().await.unwrap();
        let rows = store.query_usage(0, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ts_hour % 3600, 0);
        assert!(rows[0].ts_hour <= now_unix());
    }

    // ---- §9.8 bytes_out 分次累计 ----
    #[tokio::test]
    async fn bytes_out_accumulates_per_chunk() {
        let (_dir, store, tracker) = setup("t8");
        tracker.record_bytes_out("r", 1, 10);
        tracker.record_bytes_out("r", 1, 20);
        tracker.record_bytes_out("r", 1, 30);
        tracker.flush().await.unwrap();
        assert_eq!(db_totals(&store).2, 60);
    }
}
