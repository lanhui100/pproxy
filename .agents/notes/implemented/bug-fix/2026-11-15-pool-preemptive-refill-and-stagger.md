# Agent Note: 待命池改为过期前补建并对同批会话出生错峰

Status: implemented

> 前置与关系：`2026-11-14-pool-refill-prewarm-budget.md` 修好了补给**怎么拨**（15s 专用预算 + 有界并行），
> 把 `dial timeout after 8s` 的失败率压到 0；但那条记录明确把"池水位仍会被打穿"留给后续。
> 本条处理的是补给**何时拨**与**每批会话何时过期**——`pooled=false` 的剩余来源。
> 容量侧的历史：`2026-10-09-fix-tunnel-pool-dead-session-and-timeout.md` 曾把 `POOL_SIZE` 2→4。

## Problem

重启后 7 分钟窗口（已剔除压测）的生产收据：

| 指标 | `pooled=true` | `pooled=false` |
|---|---|---|
| 次数 | 723 | 24 |
| establish p50 | 251 ms | **1332 ms**（5.3×） |
| establish p90 | 697 ms | 1864 ms |

同期 `tunnel establish failed` = 0、`pooled session bind failed` = 0、401 熔断 = 0 ——
穿透**不是**死会话兜底失败，而是 checkout 时池里确实没货。三条结构性成因：

### 1. 池深 4 恰好等于 1 s 窗口建连峰值，零余量

剔除压测后的真实建连密度（去重后滑动窗口峰值）：1 s = 4、5 s = 6、10 s = 8、30 s = 12、60 s = 21。
`POOL_SIZE = 4` 在 1 s 窗口刚好打平，5 s 以上窗口必然穿透。

### 2. TTL 集体过期：一轮并行补给建出的会话同生同死

补给一轮并行建 4 条，`born` 相差不超过几毫秒 → 25 s 后**同时**过期。而缺口公式是
`target - 未过期数`：过期前一刻缺口恒为 0（4 条都还没"过期"），于是不补货；到期瞬间 4 条一起消失，
池全空，直到下一轮 refill（2 s 后）才恢复。日志实证：

```
08:48:56–57  连建 4 条（同一批）
08:49:21–22  4 条集体到期
08:49:25     请求到达 → 池空 → 冷建连
```

### 3. 无预防性补货

第 2 条的直接后果：只要"未过期"口径不变，补给永远在过期**之后**才开始，
每 25 s 必然出现一个宽约 `refill_interval + 拨号耗时` 的空洞。受害面集中在 1 s 内多条并发的场景，
`github.com` 占 8/24（33%）。

## Decision

1. **池容量 4 → 8**（`crates/server/src/connect.rs::POOL_SIZE`）：按 10 s 窗口峰值取值，
   覆盖真实突发；常驻 8 条 WS 在 rn 侧 2048 `worker_connections` 下无压力。
2. **缺口口径改为"剩余寿命 > 补货提前量"**（`crates/transport/src/pool.rs`）：
   `refill_lead = min(2 × refill_interval, idle_ttl / 2)`，剩余寿命不足该值的会话不再计入目标容量，
   于是补建在旧会话过期**之前**（生产 4 s 提前量）就启动，新会话先就位、旧会话后下落。
   上界 `idle_ttl / 2` 是防自激：节拍与 TTL 相当时，无上限的提前量会让每条会话一出生就"不可用"。
3. **同批出生错峰**：拨号清单携带 `(批次内序号 i, 批次长度 n)`，落地时
   `born = now - usable × i / n`（`usable = idle_ttl - refill_lead`）。过期点被摊开，
   稳态退化为每次只补 1 条。**只允许前移**（会话被视为更老）：反向偏移会让会话活得比 TTL 更久，
   越过 Gate 的 60 s 首帧等待上限。首轮填充有一次性代价——部分会话寿命被压到 ~7 s，
   之后每次只补 1 条，自愈为全长寿命。
4. 两条门禁（均做过反向验证，见"实测收据"）。

范围边界：`crates/engine/src/connect.rs::POOL_SIZE = 2` 未改。它是本地网关路径，
本次证据（`pooled` 日志、建连密度）全部来自 `crates/server` 的生产实例，对 engine 无数据，
按"无证据不扩面"留待有观测后再定。

## Alternatives considered

- **只把 `POOL_SIZE` 提到 8**：不解决集体过期——过期瞬间池照样全空，只是空得更"饱"；
  且 30 s 窗口峰值 12 仍会穿透。容量与节奏是两个独立维度，只补一个必然留洞。否决。
- **缩短 `refill_interval`（2 s → 500 ms）**：把"过期后补"的空洞压窄，代价是 4 倍轮询与更频繁的空转，
  且空洞仍在（只是更短）——用概率换结构。否决。
- **增大 TTL（25 s → 40 s）**：Gate 首帧等待 60 s 决定上限，25 s 已留 2.4× 余量；拉长会让
  "建好却长期不用"的会话占用 gate 连接，并放大陈旧会话（对端已断）被路由出去的概率。否决。
- **固定相位轮转（每 tick 无条件重建 1 条）**：稳态抖动确实小，但突发 checkout 抽空后要等
  `size` 个 tick 才补满，比缺口驱动更慢。否决。
- **错峰用"出生后移"（延迟出生）**：等于人为延长会话寿命，会越过 Gate 的首帧等待上限。否决。
- **在 `checkout` 侧拒绝快过期会话**：把空洞移交给调用方，仍然 miss，只是换个地方 miss，
  还多了一次无谓的 pop/push。否决。

## Consequences

- 池不再成段清空：稳态每次只补 1 条，过期点按 `usable / size` 摊开。
- 池瞬时可达 2 × `size` = 16 条（新旧批次在 `refill_lead` 窗口内重叠），容量侧可承受。
- 可观测口径未变：`pool_stats()` 的 `idle` 仍含未清理的过期残留；新增的 `fresh_total()` /
  `ages()` 只在 `cfg(test)` 下可见，不扩公共契约。
- 门禁（都在 `crates/transport/src/lib.rs`）：
  - `test_pool_refills_before_expiry` —— 单端点 size=1、TTL 600 ms / 补给 50 ms，5 ms 采样覆盖 3 个 TTL，
    断言可用会话数始终 ≥1（旧口径在过期瞬间掉到 0）；
  - `test_pool_staggers_same_batch_birth` —— size=3 首轮填充后，断言三会话年龄的极差 ≥120 ms
    （旧实现极差 ≈0.1 ms）。
- 回滚：`POOL_SIZE` 改回 4 只影响容量，可独立回滚；算法部分需同时还原"缺口口径"与"born 偏移"两处，
  且已被上述两条门禁判为缺陷，不建议单独回滚其中一处。

## 实测收据

```
# 单测（含 2 条新门禁）
cargo test -p pproxy-transport              → 22 passed; 0 failed
cargo test -p pproxy-server -p pproxy-engine → 全部 ok

# 反向验证（证明门禁有效，非常绿）
缺口口径还原为 v.len()（未过期计数） → test_pool_refills_before_expiry FAILED
                                     （"稳态下池不得见底，第 99 次采样可用会话 = 0"）
born 偏移置 0                        → test_pool_staggers_same_batch_birth FAILED
                                     （spread=113.176µs，期望 ≥120ms）

# 生产（cargo build --release + sudo systemctl restart pproxy，PID 1739819 → 3136921，17:44:20 CST）
重启前 90 min 基线   ：pooled=true 292 / pooled=false 0 / refill failed 6
重启后（至 18:00）   ：refill failed 0（窗口内唯一 1 条来自旧 PID 1739819）
经 8899 generate_204 → 204 × 3（0.70 / 0.92 / 0.94 s）
12 路并发 api.github.com → 200 × 12
静置 40 s 后 8 路并发 → pooled=true ×10 / pooled=false ×0
                      （旧池深 4 时同一场景只能命中 4 条，其余 4 条必为冷建连）
```

生产侧的口径说明：重启前 90 min 窗口的 `pooled=false` 已经是 0，因此本条**不**对"命中率提升"做 A/B 断言；
容量维度用受控突发（8 路并发全部首试命中）作收据，节奏维度（过期前补建 + 错峰）以两条单测门禁
及其反向验证作收据——生产没有暴露池水位的观测点，不拿日志猜。
