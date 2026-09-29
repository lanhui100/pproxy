# 轨道A：转发链路稳定性测试报告（pproxy CONNECT 隧道池）

- **测试时间**：2026-09-29 16:45 – 17:10 CST（探测窗口 ≈25 分钟）
- **链路**：devserver 本地数据面 `127.0.0.1:8899`（正向 CONNECT，无鉴权）→ 隧道池 `wss://rn.ponygo.fun/ws`、`wss://gate.ponygo.fun/ws`、`wss://vgate.ponygo.fun/ws` → 目标站
- **方法**：4 目标 host 顺序循环探测，轮间 sleep 2s，`curl -x http://127.0.0.1:8899 -m 20`，CSV 记录 epoch_ms/connect/appconnect/total/size
- **链路通定义**：http_code ∈ [200,499]；失败 = 000（含 -m 20 超时）

## 1. 每 host 结果

| host | N | 成功率 | p50(ms) | p90(ms) | p95(ms) | p99(ms) | max(ms) | p95/基线 | 超时(rc=28) | 判定 |
|---|---|---|---|---|---|---|---|---|---|---|
| daily-cloudcode-pa.googleapis.com | 176 | 100.00% | 2155.5 | 2374.5 | 2426.0 | 2564.5 | 3198 | 1.13x | 0 | 稳定 |
| www.google.com | 176 | 100.00% | 1091.0 | 1208.0 | 1239.0 | 1314.25 | 2139 | 1.14x | 0 | 稳定 |
| api.github.com | 176 | 100.00% | 1154.5 | 1993.5 | 2194.25 | 2591.5 | 2649 | 1.9x | 0 | 稳定 |
| opencode.ai | 176 | 100.00% | 1720.5 | 2725.0 | 2973.75 | 3270.0 | 3544 | 1.73x | 0 | 稳定 |
| **合计** | 704 | 100.00% | - | - | 全局p95 2471.350000000001ms | - | - | 基线 1500.5ms | 0 | - |

### 失败明细（probe-failures.log）
- 无（0 次失败）

## 2. 每 5 分钟桶平均 total_ms（成功样本，ms）

| host | 0-5min | 5-10min | 10-15min | 15-20min | 20-25min |
|---|---|---|---|---|---|
| daily-cloudcode-pa.googleapis.com | 2226.0(0✗) | 2104.6(0✗) | 2128.5(0✗) | 2175.3(0✗) | 2131.9(0✗) |
| www.google.com | 1100.5(0✗) | 1077.4(0✗) | 1112.2(0✗) | 1135.7(0✗) | 1081.9(0✗) |
| api.github.com | 1295.6(0✗) | 1265.4(0✗) | 1246.5(0✗) | 1277.7(0✗) | 1344.6(0✗) |
| opencode.ai | 1782.2(0✗) | 1830.1(0✗) | 1881.4(0✗) | 1933.1(0✗) | 1940.2(0✗) |

## 3. 隧道池窗口统计（journalctl -u pproxy.service，窗口内全部 tunnel 事件）

- 窗口内 `tunnel established` 总数：**835**（pooled=true **641** / pooled=false **194**，冷建连占比 23.2%）
- establish_ms（全部）：min=220 p50=290 p90=1415.8000000000002 p95=1481.3 p99=1714.5599999999995 max=6296
- `allowlist advisory only` 事件：835（其中 allowlisted=true 553）
- 错误类日志行（timeout/denied/error/retry/fail/refused/closed/reconnect）：**0** 条（明细见 journal-errors.txt）

| host | establishes | pooled=false | establish_ms p50/p95/max |
|---|---|---|---|
| vedge.ponygo.fun | 29 | 0 | 278/1455.0/6296 |
| daily-cloudcode-pa.googleapis.com | 185 | 185 | 1327/1498.6000000000001/2276 |
| www.google.com | 176 | 0 | 267.0/307.0/1311 |
| api.github.com | 192 | 3 | 289.5/1309.25/1704 |
| opencode.ai | 176 | 1 | 276.0/1505.75/1854 |
| rn.ponygo.fun | 24 | 1 | 285.5/1422.55/1499 |
| edge.ponygo.fun | 23 | 3 | 311/1772.6/1813 |
| herdr.dev | 20 | 1 | 287.5/1565.0000000000002/1831 |
| default.exp-tas.com | 2 | 0 | 285.0/310.2/313 |
| exp.individual.githubcopilot.com | 1 | 0 | 282/282/282 |
| mobile.events.data.microsoft.com | 1 | 0 | 293/293/293 |
| models.opencode.ai | 2 | 0 | 955.0/1537.3/1602 |
| productionresultssa17.blob.core.windows.net | 1 | 0 | 306/306/306 |
| ipinfo.io | 3 | 0 | 277/1270.6/1381 |

## 4. 异常关联：慢样本/失败 vs pooled=false 冷建连（±15s 内最近事件）

- **daily-cloudcode-pa.googleapis.com**：慢样本(total>p99) 2 个，其中 2 个命中冷建连；失败 0 个，其中 0 个命中冷建连。 示例慢样本 [('16:48:08', 2728, 1477), ('17:00:33', 3198, 2283)]
- **www.google.com**：慢样本(total>p99) 2 个，其中 0 个命中冷建连；失败 0 个，其中 0 个命中冷建连。
- **api.github.com**：慢样本(total>p99) 2 个，其中 0 个命中冷建连；失败 0 个，其中 0 个命中冷建连。
- **opencode.ai**：慢样本(total>p99) 2 个，其中 0 个命中冷建连；失败 0 个，其中 0 个命中冷建连。

## 5. 判定

- 触发条件：无（p95 未超 2x 基线、失败率未超 0.5%、10s+ 超时 <2 次）
- **结论：转发链路 稳定**

## 6. 原始数据
- `probe_<host>.csv`（4 个，逐请求原始行）；`tunnel-pool-window.json`；`journal-errors.txt`；`../logs/journal-window.txt`；`../logs/probe-failures.log`
