# Agent Note: proxy-network-stability-test

Status: implemented

## Problem

pproxy 分布式代理网络（本地数据面 `:8899` + 管理面 `:8900`@tailnet + 分布式出口
RackNerd VPS `rn.ponygo.fun` / Vercel `vedge.ponygo.fun` / CF Worker `edge.ponygo.fun`
/ 隧道网关 `gate.ponygo.fun`）已上线运行 >24h（systemd `pproxy.service` 1 天 8 小时）。
ponyllm（K8s，`ponyllm` namespace）的海外链路依赖该网络：
- antigravity provider → `http://user:…@pproxy-host.ponyllm.svc:8899`（tencent node，CONNECT 隧道池）；
- opencode/zen provider → `http://100.95.193.103:8899/<token>/opencode/zen/v1`（devserver 反向网关）。

需要一次全面、可复现的稳定性测试，量化网络波动对 ponyllm 的实际影响，并给出
评估报告与阈值判定，而不是"感觉挺稳"。

## Decision

采用**四路并行 + 历史证据**的测试方法论（2026-09-29 执行，专用数据面 token
`stability-test-20260929`，用后即撤）：

| 轨道 | 覆盖 | 关键指标 |
|---|---|---|
| A 转发链路 | 本地 `:8899` CONNECT 隧道池 → 出口（googleapis/generate_204/github/opencode.ai） | p50/p95/p99 延迟、错误率、冷建连（pooled=false）事件相关性 |
| B 反向路由 | `/{token}/{route}` 六路由（openai→vercel、anthropic/bai/github→worker、opencode→vps、xai→vercel）+ 直连上游对照 | 每路由延迟分位、错误率、备份 failover |
| C 物理路径 | 6 出口域 + 目标 API + tailnet 节点：DNS/TCP/TLS/HTTP/ping/mtr/证书 | 丢包率、RTT 分位、路径拓扑、出口 IP/ASN |
| D ponyllm 影响 | K8s 内：antigravity 路径经 pproxy-host + 反向网关 + 真实推理（TTFT/stream stall）+ 24h 历史（journal/usage/prober/telemetry） | TTFT、块间 gap、错误率、历史 failover/异常 |

阈值判定（本测试采用）：p95 延迟劣化 >2x 基线、错误率 >0.5%（网络类，非上游 4xx）、
测试窗口内出现 ≥2 次 10s+ 超时均记"不稳定"；其余记"稳定"。

产出：`docs/ops/stability-report-2026-09-29.md`（评估报告）+ 原始数据
`.agents/tests/stability-2026-09-29/{forward,reverse,network,ponyllm}/`。

## Alternatives considered

- 仅 ICMP/ping 级网络测试：覆盖不了代理数据面与隧道池行为，且多数 CDN 禁 ICMP，否决。
- 纯被动观测生产流量：样本周期长、无基准对照，无法给出量化分位结论，否决。
- 主动故障注入（杀出口/断 VPS/断 tailnet）：生产风险高、影响 ponyllm 线上，本次否决；
  改为观测自然波动 + 冷建连/重连事件作为波动证据（预留后续演练场景）。
- 单机串行测试：周期长、无法横向对照各链路，否决；改四路并行，每路专用输出目录，
  写范围不相交。

## Consequences

- 测试负载受网关 `verify_concurrency=30` 限制，四路并发探测总量小，不影响生产配额；
- 专用 token 测试完成后即吊销（`DELETE /api/tokens/{id}`），不留后门；
- 报告与数据落盘持久化，后续周期性复测可复用本方法论与脚本（`PLAN.md` 与各轨道脚本）。