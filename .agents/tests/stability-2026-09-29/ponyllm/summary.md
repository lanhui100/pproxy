# 轨道D：pproxy 网络波动对 ponyllm 服务的影响（端到端 + 24h 历史）

- 测试窗口（CST）：2026-09-29 16:44:56 起，持续探测约 25 分钟（详见 START_TIME.txt）
- 执行载体：K8s namespace `ponyllm` 内 pod `ponyllm-synthetic-prober`（`/app/.venv/bin/python3`）
- 被测链路：
  - 链路1（antigravity 真实路径）：pod → `pproxy-host.ponyllm.svc:8899`（tencent 100.105.241.39:8899，Basic auth）→ CONNECT → `https://daily-cloudcode-pa.googleapis.com/`（根路径 404 = 通）
  - 链路2（反向网关真实路径）：pod → `100.95.193.103:8899/pony_.../opencode/zen/v1/models`（预期 200）
- 端到端：pod → `ponyllm-pod-service.ponyllm.svc:8080/v1/chat/completions`（Bearer sk-pony-7cc4cd2c0cb646a9a571067ce89eefa9）

## 1. 链路探测统计（链路1 每 3s / 链路2 每 8s，实际窗口 16:49:44 → 17:13:20 CST）

| 指标 | 链路1 antigravity（pproxy 正向代理） | 链路2 opencode 反向网关 |
|---|---|---|
| 样本数 | 304 | 136 |
| 成功率 | 99.67%（303/304） | 100%（136/136） |
| 延迟 p50 | 1679 ms | 350 ms |
| 延迟 p90 | 2982 ms | 704 ms |
| 延迟 p95 | 3566 ms | 878 ms |
| 延迟 p99 | 5819 ms | 962 ms |
| 延迟 max | 8594 ms | 968 ms |

- 链路1 实测失败 1 次：17:04:20 `RemoteDisconnected:Remote end closed connection without response`（耗时 14.8s）——即 0.33% 请求被远端提前断连。
- 链路1 每次请求都要新建 pproxy 隧道（24h journal 中 cloudcode 域 335 次 establish 全部 `pooled=false`，中位 1340ms）——这是链路1 延迟 ~1.7s 的主因，属 pproxy 设计行为而非偶发故障。

## 2. 真实推理（流式）统计

| 指标 | 全部流式（n=12） | auto:economy（n=9） | gemini-3.8-flash-high（n=3） |
|---|---|---|---|
| 请求成功 | 12/12 (100%) | 9/9 | 3/3 |
| TTFT p50 | 12058 ms | 12282 ms | 5429 ms |
| TTFT p90 | 32691 ms | 32691 ms | — |
| TTFT p95 | 48448 ms | 48448 ms | — |
| TTFT max | 48448 ms | 48448 ms | 7154 ms |
| 块间最大 gap | 5723 ms | 5723 ms | 284 ms |
| stall（gap>2s）次数 | 2 | 2 | 0 |

- 附加：1 次非流式请求（gemini-3.8-flash-high）200，2926ms，确认模型路由生效。
- 12 次流式全部 200、无错误；但 TTFT 波动极大：auto:economy 从 3.2s 到 48.4s，p95≈48.4s。
- stall 明细：
  - seq0（auto:economy short，16:51:11）：TTFT 22.5s，块间 gap 5.72s（>2s 记 stall）
  - seq9（auto:economy long，17:03:56）：TTFT 9.3s，块间 gap 2.07s（临界 stall）
- 长输出（max_tokens=200，2 次）无中途长时间静默：块间 gap 最大 2.07s，81 chunks 平滑。

## 3. 近 24h 历史证据

### pproxy journal（`history-pproxy-journal.txt`，24h）
- tunnel established 总数：2248；establish_ms：p50=300ms，p90=1471ms，p95=1583ms，p99=2311ms，max=25864ms（2026-09-29T04:02Z models.opencode.ai，即链路2 上游）
- pooled=false 数：516（23%）
- 错误类计数：error=31，timeout=42，denied=69，retry=24，reset=1，failover=0
- 链路1 目标域 daily-cloudcode-pa.googleapis.com：24h 内 335 次 establish **全部 pooled=false**（每次新建隧道），establish_ms p50=1340/p95=1554/max=7925

### ponyllm 网关日志（`history-gateway-logs.txt`，24h）
- 关键字计数：error=491，timeout=205，retry=140，failed=82，stall=3，failover=2，503=12
- 3 次 `[timeout:tail-stall] upstream stream stalled after 120s without bytes`：04:51Z / 05:51Z / 08:12Z（=今日 12:51 / 13:51 / 16:12 CST）
- 49 次 `Antigravity empty-STOP` 透明重试，全部集中在 06:00–07:59Z（=今日 14:00–16:00 CST），单请求最高 11 次重试（07:06Z）——antigravity 上游在测试窗口前 2 小时内刚经历过一轮空响应风暴

### telemetry snapshot（`history-telemetry.json`，saved 2026-09-27T12:04Z，最近可用快照）
- 全周期：total=94219，failed=3775（4.01%），failover_count=18937，stalls_sum=34579，max_gap_ms=118868
- 最后 24 桶（09-25 23:00 → 09-27 12:00）：total=6122，failed=139（2.27%）
- antigravity（链路1 上游）：成功率 99.88%，ewma_ttft≈6.78s，stalls=1542，max_gap=90.0s
- opencode-zen（链路2 上游）：成功率 98.46%，ewma_ttft≈3.35s，stalls=14077，max_gap=118.9s
- ppx（直连 pproxy 隧道）：成功率 92.52%，ewma_ttft≈30.4s，stalls=738，max_gap=56.7s，latest_latency=19.5s

### 用量与探针（`history-usage.csv` / `metrics.txt`）
- usage_hourly 近 30h 仅 opencode 路由有流量（今日 10:00–16:00 高峰，最高 222 req/h@13:00）；pony_31abcbd... token 路由近 24h 无独立记录（经 opencode 路由汇总）
- prober /metrics：synthetic probe success=1，failures=0（探针自身历史健康）

## 4. 影响量化结论

**网络波动对 ponyllm 服务的影响：可观测且集中在"首包延迟"，端到端成功率影响小。**

- 链路层面（实测窗口 ~24 分钟，16:49:44–17:13:20）：链路1 成功率 99.67%（303/304，1 次 RemoteDisconnected），延迟 p50=1.68s/p95=3.57s/max=8.6s；链路2 成功率 100%（136/136，p95=0.88s）。链路1 每请求新建隧道（pooled=false）是延迟基线的结构性因素。
- 端到端推理：12/12 流式请求成功（0 错误），但 TTFT 严重劣化——auto:economy 路线 p50=12.3s、p90=32.7s、p95=48.4s（最坏 48.4s @ seq8 17:02:31），远超 gemini-3.8-flash-high 直连的 4.5–7.2s；2/12 出现 >2s 块间 stall（最坏 5.72s @ seq0 16:51:11）。
- 历史佐证：24h 内 3 次"120s 无字节"尾部 stall、今日 14:00–16:00 antigravity empty-STOP 风暴（49 次重试）、telemetry 中 antigravity max_gap 90s / opencode 118.9s 的极端静默记录，说明波动是持续存在的现象而非本次偶发。

**阈值判定**：
- 失败率：端到端 0%（<0.5% 阈值，未触发）；链路1 0.33%（<0.5%，未触发）
- p95 延迟：链路1 p95=3.6s 约为 p50（1.7s）的 2.1 倍（>2x 基线，触发）；auto:economy TTFT p95=48.4s 约为 p50 的 3.9 倍（显著触发）
- 10s+ 超时：1 次 14.8s RemoteDisconnected（链路1）+ TTFT 多次 >20s（端到端共 4/12 请求 TTFT>20s，其中 2 次 >30s）——单请求耗时层面显著，但均以 200 完成、无客户端可见失败

**结论：pproxy 网络波动对 ponyllm 的端到端成功率影响可忽略（测试窗口 13/13 请求成功、链路失败率 0.33%），但对首包延迟（TTFT）影响显著——auto:economy 路线 TTFT p50=12.3s / p95=48.4s、多次 >20s，叠加历史 3 次 120s 尾部 stall 与今日 14:00–16:00 antigravity empty-STOP 重试风暴（49 次），用户可感知为"慢"而非"不可用"。**

## 产出文件清单
- START_TIME.txt / pod_probe.py / inference.py / analyze.py
- pod_proxy.csv / pod_reverse.csv（探测原始数据，见 logs/probe-run1-mixed-backup.csv 与第三轮合并数据）
- inference.csv（13 次推理明细）
- history-pproxy-journal.txt / history-usage.csv / metrics.txt / history-gateway-logs.txt / history-telemetry.json
- logs/gateway-logs-24h.txt（网关 24h 原始日志）
