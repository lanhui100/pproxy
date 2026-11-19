# Agent Note: 待命池补给拆分超时预算并改为有界并行

Status: implemented

> 前置与关系：`2026-09-26-restore-dial-timeout-8000ms.md` 把 `DIAL_TIMEOUT` 从 800ms 放宽到 8s，
> 解决的是**交互路径**（CONNECT 建连、探针）被砍。本条不推翻它——8s 对交互路径仍然是正确口径。
> 本条处理的是被那条记录连带覆盖的**后台补给路径**：补给复用了同一个常量，而它没有人在等。
> `2026-11-13-rn-nginx-worker-capacity.md` 已证伪「nginx 连接数耗尽导致 refill 失败」，
> 并把该症状明确移交给客户端侧，本条即那次移交的落地。

## Problem

生产日志近 60 分钟稳定出现：

```
WARN pproxy_transport::pool: tunnel pool refill wss://rn.ponygo.fun/ws failed: dial timeout after 8s
```

19 次失败 / 60 分钟，全部来自 `rn.ponygo.fun`（备用 `gate.ponygo.fun` 因 `has_vps` 规则
`target_size=0` 不参与预池化，所以只有 rn 会报）。同期业务未中断（`pooled=true` 629 次），
但池水位被打穿后 `pooled=false` 60 次，即这些请求全部退化为跨洋冷建连。

两处结构性缺陷，都不在「8s 够不够大」这个层面：

### 1. 补给轮是串行 `await`，单条慢拨号阻塞整轮

旧实现在端点循环里逐条 `connect_ws(ep, &token).await`，且**任一失败即 `break`** 跳出端点循环。
跨境链路 p99 长尾（实测 dev→rn 的 WS 建连 p99≈7~11s、max≈20s）会：

1. 卡住整轮补给（`pool.size=4` 时最坏 4×15s 串行）；
2. 期间池被 checkout 抽空却无人补，后续请求全部 `pooled=false`；
3. 更糟：排在前面的坏端点失败即 `break`，**后面的健康端点永远补不上**（队头阻塞）。

### 2. 补给与交互共用 8s 预算，但两者代价完全不同

| 路径 | 超时代价 | 合理预算 |
|---|---|---|
| 交互（CONNECT/探针） | 用户可感知的延迟，直接表现为请求失败 | 8s（守延迟） |
| 后台补给 | 少一条待命会话，2s 后下一轮再补 | 应更宽 |

实测（`unset` 代理后直连，200~400 样本）：

```
并发 4 : 成功 399/400  p50=1.16 p95=4.49 p99=7.08 max=10.91   >8s: 0.75%
并发 30: 成功 300/300  p50=1.22 p90=2.99 p99=11.70 max=20.48  >8s: 3
```

8s 稳定切在 p99 附近——**失败是长尾被预算砍掉，不是链路不通**。

## Decision

1. `proto.rs` 新增 `PREWARM_DIAL_TIMEOUT`（生产 15s / `cfg(test)` 1500ms），
   抽出 `connect_ws_with_timeout(url, token, budget)`；`connect_ws` 保持原签名与 8s 预算
   （交互路径消费者零改动）。
2. `pool.rs` 补给改用 `PREWARM_DIAL_TIMEOUT`。
3. `pool.rs` 补给改为**有界并行**：先一次性算清各端点缺口 `wants: Vec<(ep, need)>`，
   再用 `stream::iter(...).map(...).buffer_unordered(pool.size.max(1))` 并发拨号。
   单条失败只记日志，不再 `break` 中断其它端点。401 熔断语义不变（仍按 `consec_401>=5` 触发，
   只是从循环内即时 sleep 改为整轮结束后统一熔断，避免持锁/挂起并发流）。

## Alternatives considered

- **只把 8s 调大（如 15s）**：能消掉告警，但把交互路径的延迟预算一起放宽——建连挂死时用户要等
  15s 才收到 502，违背「交互路径守延迟」的初衷。而且不解决串行阻塞与队头阻塞，
  池被打穿的根因仍在。否决。
- **只做并行，不改预算**：p99≈7~11s 仍会被 8s 砍，只是砍得更快、失败更密集。实测并发 30 时
  >8s 有 3 条，8s 下这些必然失败。否决。
- **补给完全取消超时**：跨境链路存在真挂死（SYN 黑洞/TLS 卡在 Encrypted Extensions），
  无上限会让 `buffer_unordered` 的槽位被永久占住，反而降低有效并发。15s 是「覆盖 p99.9
  又不让槽位被长期占死」的折中。否决。
- **并发度取 `pool.size × 端点数`**：对 gate 压力放大且无收益——每个端点缺口本就 ≤ `pool.size`。
  取 `pool.size` 已能在一个 RTT 窗口内补齐。否决。

## Consequences

- `refill ... dial timeout after 8s` 告警消失（重启后 5 分钟窗口含 40 并发压测：**0 次**）。
- 补给不再被单条慢拨号阻塞，坏端点不再饿死健康端点。
- 池水位更稳 → `pooled=false` 冷建连穿透减少。
- 新增门禁：
  - `test_prewarm_budget_wider_than_interactive` —— 断言 `PREWARM_DIAL_TIMEOUT > DIAL_TIMEOUT`，
    防后人把两个预算又合并成一个常量；
  - `test_pool_refill_tolerates_slow_handshake` —— mock gate 延迟 `DIAL_TIMEOUT+200ms`，
    断言补给仍成功（**反向验证**：把预算换回 `DIAL_TIMEOUT` 后该测试失败）；
  - `test_pool_refill_isolates_failing_endpoint` —— 死端点 + 健康端点并存，断言健康端点仍能补上
    （**反向验证**：恢复 `break` 后该测试失败）。
- 回滚：把 `pool.rs` 的 `PREWARM_DIAL_TIMEOUT` 换回 `DIAL_TIMEOUT` 即回到旧预算口径；
  并行改串行需重写该段（无开关，因为串行版已被上述测试判为缺陷）。

## 实测收据

```
# 单测（含 3 条新门禁）
cargo test -p pproxy-transport            → 20 passed; 0 failed
cargo test -p pproxy-engine -p pproxy-server → 全部 ok（16+54+3+1+1）

# 反向验证（证明测试有效，非常绿）
预算改回 DIAL_TIMEOUT      → test_pool_refill_tolerates_slow_handshake FAILED
恢复失败即 break           → test_pool_refill_isolates_failing_endpoint FAILED

# 生产（release 重建 + sudo systemctl restart pproxy，PID 3815885 → 3301118）
近 3 分钟 refill 失败 0；pooled=true 19
40 并发 curl 经 8899 → 204 × 40；近 5 分钟 refill 失败 0；pooled true/false = 44/42
```
