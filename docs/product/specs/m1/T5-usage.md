# T5 — 用量计数（usage.rs）

> 依赖: T1 | 并行波次 2（与 T2/T4 并行，仅新增 usage.rs）| 上游: M1 spec §3.4

## 1. 目标

内存实时计数（route, token_id）三维 → 小时聚合 UPSERT 落库 → 查询聚合（内存 + 库）。落库与清零原子（裁决 #4）；SSE 流式 bytes_out 统计（裁决 #5）。

## 2. 文件位置

- 新增 `crates/core/src/usage.rs`；`lib.rs` 追加 `pub mod usage;`（仅此一行）。
- `crates/core/Cargo.toml`：`dashmap.workspace = true`（T1 已入 workspace）。

## 3. 公开接口（Rust 签名级）

```rust
// crates/core/src/usage.rs
use crate::store::{Store, UsageRow, StoreError, now_unix, hour_floor};
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct UsageDelta {
    pub requests: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Debug)]
pub enum UsageError { Store(StoreError) }
impl Display + Error + From<StoreError>;

/// 内层计数器，std::sync::atomic::AtomicU64 三元组
#[derive(Default)]
struct Counters { requests: AtomicU64, bytes_in: AtomicU64, bytes_out: AtomicU64 }

pub struct UsageTracker {
    store: Arc<Store>,
    /// key = (route, token_id)；当前未落库小时的活计数
    live: DashMap<(String, i64), Counters>,
}

impl UsageTracker {
    pub fn new(store: Arc<Store>) -> Self;

    /// 请求开始时调用（T3 转发 handler）：requests+1, bytes_in+n。
    /// 无锁热路径（DashMap 分片 + Atomic）。
    pub fn record_request(&self, route: &str, token_id: i64, bytes_in: u64);

    /// 响应流式期间每 chunk 调用（T3）：bytes_out+n。
    pub fn record_bytes_out(&self, route: &str, token_id: i64, n: u64);

    /// 落库：drain 当前 live 表 → UPSERT → 成功后丢弃；
    /// 失败则合并回 live（不丢计数）。
    pub async fn flush(&self) -> Result<usize, UsageError>;

    /// 查询聚合：库中 [since_hour, now] 区间（**含 since_hour 当小时**，C-P2-11，
    /// 与 store.query_usage 的闭区间语义一致）+ 当前 live 内存，合并求和。
    /// 返回按 (route, token_id) 聚合的行。
    pub async fn query(&self, since_hour: u64, route: Option<&str>,
        token_id: Option<i64>) -> Result<Vec<UsageRow>, UsageError>;
}
```

## 4. 落库原子性（裁决 #4，精确算法）

问题：flush 期间新请求继续写入，若"先落库后清零"会丢清零窗口内的计数；若"先清零后落库"会丢落库失败的数据。

**算法：drain-and-swap，不做原地清零。**

1. `flush()` 开始：`std::mem::take` 不适用于 DashMap——改为**整表换出**：`let drained: HashMap<_,_> = live.drain().map(|(k,v)| (k, v.snapshot()))`。DashMap 的 `drain()` 是原子的（迭代期间新写入进入新 entry，不会丢失）。
   - 精确实现：`live.retain` 不可用；采用 `DashMap::drain()`，其语义为移除全部元素并返回迭代器，drain 开始后的 `entry()` 写入会重新创建条目——即 drain 与并发写安全。
2. 对 drained 中每个 `(route, token_id)` 计算 `ts_hour = hour_floor(now_unix())`。
3. `store.upsert_usage(&rows)`（单事务；同步签名，经 `spawn_blocking` 调用，C-P1-9）。
4. 成功 → 丢弃 drained，返回行数。
5. 失败（StoreError）→ 将 drained **合并回** live：对每个 key `live.entry(k).and_modify(|c| c.add(&delta)).or_insert_with(|| delta.into_counters())`（**C-P1-6 裁决**：drain 后并发写可能已重建该 entry，`and_modify` 单独使用会**丢弃重建条目中的新计数**——必须链上 `or_insert_with` 兜底；entry vacant/occupied 两态都要正确），返回 Err。计数不丢，下轮 flush 重试。
6. 跨小时边界：drain 后逐 key 用**各自 drain 完成时刻**的 hour_floor（统一用 flush 时刻的 ts_hour，误差 ≤1 小时边界，个人场景可接受——**裁决**：统一 flush 时刻，不做逐条时间戳，理由：内存计数本就不携带时间，逐条时间需要 per-key 记录首写时间，复杂度不值）。

## 5. bytes_out 的 SSE 流式统计（裁决 #5）

- 统计点在 T3 转发 handler：`Body::from_stream(resp.bytes_stream())` 改为 `Body::from_stream(stream::map(resp.bytes_stream(), |chunk| { usage.record_bytes_out(route, token_id, chunk.len()); chunk }))`——**在透传前对每个 chunk 计数，不缓冲、不改变字节**，SSE 逐事件输出特性不受影响。
- `bytes_in`：请求 body 读取完成后一次性 `record_request(route, token_id, body.len())`。
- 客户端中途断连（SSE 提前关闭）：已计数部分保留，不回滚——与上游实际产生的流量一致，**裁决**：断连不扣减。
- 上游失败（502 路径）：bytes_in 已计（请求确实到达网关），bytes_out 为 0。**不**撤销 requests 计数。

## 6. 落库循环（main.rs spawn，非 usage.rs 内部）

```text
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    interval.tick().await;              // 第一次立即触发，跳过
    loop {
        interval.tick().await;
        if let Err(e) = tracker.flush().await { tracing::warn!("usage flush: {e}"); }
    }
})
```

- 首个 tick 立即返回的特性：用 `interval.tick().await` 一次丢弃后再进入循环，避免启动即 flush 空表（无害但避免无意义写库）。
- 进程退出不 flush（M1 无优雅停机要求，最多丢最后一个不足小时窗口的内存计数——可接受，注释说明）。

## 7. 查询聚合语义（T6 /api/usage 依赖）

`query(since_hour, route, token_id)`：
1. 库：`store.query_usage(since_hour, route, token_id)`（**含 since_hour 当小时**，闭区间 [since_hour, now]，C-P2-11——`hours=24` 即"过去 24 个完整小时 + 当前进行中的小时"）。
2. 内存：遍历 live，过滤 route/token_id，key 匹配的计入。
3. 合并：同 `(route, token_id)` 求和（库行 ts_hour 不同需先按 key 聚合）。
4. 返回 `Vec<UsageRow>`（每 key 一行；`ts_hour` 字段在聚合行中无意义，T6 响应 DTO 不透出该字段——见 T6 §4 GET /api/usage，C-P2-10。**裁决**：M1 查询返回聚合行不做逐小时序列，GUI 图表 M2 需要序列时再加 `group_by_hour` 参数，YAGNI）。

## 8. 依赖任务

T1（Store::upsert_usage / query_usage / UsageRow / hour_floor）。

## 9. 单元测试清单

1. record → flush → store.query_usage 读回一致（requests/bytes_in/bytes_out）。
2. flush 幂等：连续两次 flush，第二次返回 0 行且库值不翻倍。
3. 并发计数：10 任务 × 1000 次 record_request，flush 后 requests=10000（`tokio::test` + `Arc`）。
4. flush 期间并发写不丢：spawn flush 的同时持续 record（循环 10ms），flush 完成后再次 flush，两次库值之和 = 总写入量（验证 drain-and-swap 不丢不重）。
5. 落库失败回滚：用指向无效路径的 Store 构造 tracker（或 mock：临时 DB 文件 flush 前删除），flush → Err，**断言 flush 失败后 live 非空（C-P1-6：合并回 live 的数据必须可观测，含 drain 后并发写入的新计数）**；随后换正常 Store（或恢复 DB）再 flush 成功，总量不丢。（实现提示：为可测性，`flush` 拆出 `drain()` 与 `merge_back()` 私有方法，测试经 pub(crate) 或同模块 `#[cfg(test)]` 直接调用；`merge_back` 需覆盖 entry occupied 与 vacant 两分支——vacant 分支即 C-P1-6 修复点。）
6. query 聚合：库中 2 行 + 内存 1 行同 key → 合并 1 行求和正确；route/token_id 过滤生效。
7. hour_floor 对齐：flush 产生行的 ts_hour % 3600 == 0。
8. bytes_out 分次累计：3 次 record_bytes_out(10,20,30) → 60。

## 10. 验收标准

- `cargo test -p pproxy-core usage::` 全绿。
- 数据面热路径（record_request/record_bytes_out）无 Mutex、无 IO、无 await（代码审查项）。
- lib.rs 改动仅 `pub mod usage;` 一行。
- flush 失败不丢计数的语义有测试覆盖（第 9.5 项，含 live 非空断言）。
- 无 `UsageTrackerHandle` 类型（C-P2-3：落库循环由 main.rs spawn，§6）。
