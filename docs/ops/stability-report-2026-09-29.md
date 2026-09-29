# PProxy 代理网络稳定性评估报告

- **日期**：2026-09-29（CST）
- **对象**：pproxy 分布式代理网络（上线已 1 天 8 小时，systemd `pproxy.service` up since 09-28 08:25）
- **目的**：全面验证代理网络稳定性，量化网络波动对 ponyllm 服务的影响
- **方法**：四路并行探测（agent team 分工）+ 近 24h 历史证据；方法论 ADR：
  `.agents/notes/implemented/testing/2026-09-29-proxy-network-stability-test.md`
- **测试窗口**：16:45–17:13 CST（约 25–28 分钟）；数据与脚本：`.agents/tests/stability-2026-09-29/`

---

## 0. 摘要（结论速览）

| 链路/层 | 判定 | 一句话依据 |
|---|---|---|
| A. 转发链路（本地 CONNECT 隧道池） | ✅ **稳定** | 704 样本 100% 成功、0 超时；p95 均 < 3s；25 分钟分桶平坦；窗口内隧道池 0 错误日志 |
| B. 反向网关 7 路由 | ✅ **链路稳定（有 1 项轻微统计超阈值）** | 3165 样本 0 失败；唯一"不稳定"标记为 opencode 路由 p95=922ms ≈ 2.4x 自身 p50，绝对值小而稳 |
| C. 物理路径（中国→五出口） | ✅ **稳定** | 五出口 TCP/TLS/HTTP 全低抖动、端到端 0% 丢包；tailnet→tencent 30ms 极稳 |
| D. ponyllm 端到端影响 | ⚠️ **成功率高、首包延迟显著** | 端到端 13/13 成功、链路失败率 0.33%；但 TTFT p95=48.4s（p50 的 3.9x），历史存在 120s 尾部 stall 与重试风暴，用户感知"慢"而非"不可用" |

**关键异常（非链路抖动，而是出口服务可用性）**：⚠️ 出口V（Vercel 函数 `vedge.ponygo.fun`）当前返回 **402 DEPLOYMENT_DISABLED**——Vercel 部署被平台禁用（疑似计费/额度），openai/xai 两路由实际**未到达上游**，返回 402 是 Vercel 自身响应。出口R（RackNerd）与出口C（Cloudflare edge/gate）均健康。

---

## 1. 被测网络拓扑（已核实）

```
[devserver 100.95.193.103]                    [K8s ponyllm (tencent 集群)]
  ├─ :8899 数据面 pproxy-server (systemd)  ←—— pproxy-host.ponyllm.svc → 100.105.241.39:8899 (tencent)
  │   ├─ 正向 CONNECT 隧道池: wss://rn.ponygo.fun/ws (RackNerd, 主)   antigravity 走此链
  │   │                      wss://vgate.ponygo.fun/api/ws (Vercel)   (合规 host 冷建连)
  │   │                      wss://gate.ponygo.fun/ws (Cloudflare)
  │   └─ 反向网关 /{token}/{route}/*:                                   opencode/zen 走此链
  │        openai/xai → vedge.ponygo.fun (Vercel  ⚠️ DEPLOYMENT_DISABLED)
  │        anthropic/github/bai/opencode-cf → edge.ponygo.fun (CF Worker)
  │        opencode → rn.ponygo.fun (RackNerd VPS, backup=worker)
  └─ :8900 管理面 (tailnet only)
```

- 出口身份：本地代理与出口R 均为 **192.210.231.8 / US-Santa Clara / AS36352**；edge 出口 172.71.31.54（Cloudflare Atlanta）。
- 直接访问（不代理）：`api.openai.com` 与 `daily-cloudcode-pa.googleapis.com` 被**系统性阻断**（DNS 污染 + TCP/TLS 全超时），`api.anthropic.com` TCP/TLS 通但 HTTP 403（风控）——**ponyllm 的海外上游必须依赖 pproxy**，这正是本代理网络的核心价值与风险面。

## 2. 轨道A：转发链路（CONNECT 隧道池）

探测：devserver 本机 `curl -x 127.0.0.1:8899`，4 目标 × 176 次，窗口 16:45–17:10 CST。

| host | N | 成功率 | p50 | p90 | p95 | p99 | max | 超时 |
|---|---|---|---|---|---|---|---|---|
| daily-cloudcode-pa.googleapis.com（antigravity 目标） | 176 | 100% | 2156 | 2375 | 2426 | 2565 | 3198 | 0 |
| www.google.com/generate_204 | 176 | 100% | 1091 | 1208 | 1239 | 1314 | 2139 | 0 |
| api.github.com | 176 | 100% | 1155 | 1994 | 2194 | 2592 | 2649 | 0 |
| opencode.ai | 176 | 100% | 1721 | 2725 | 2974 | 3270 | 3544 | 0 |
| **合计** | **704** | **100%** | — | — | 全局 p95≈2.47s | — | — | **0** |

- 每 5 分钟桶均值全程平坦（如 google 1077→1136ms，无爬升/尖峰）。
- 窗口隧道池：835 次 establish（pooled=true 641 / **cold 194，23.2%**），establish_ms p50=290/p95=1481/max=6296ms；错误类日志 **0 条**。
- ⚠️ 结构性事实：`daily-cloudcode-pa.googleapis.com` 每次探测都触发**冷建连**（窗口内 185 次 establish 全部 pooled=false，establish p50≈1.33s）——该域不走池化复用，是链路1 延迟 ~2.2s 基线的主因（设计行为，非故障）。
- 慢样本（>p99）与冷建连事件相关性：cloudcode 命中 2/2，其余 host 无关；**无由冷建连导致的失败**。

## 3. 轨道B：反向网关路由稳定性

探测：`http://127.0.0.1:8899/{token}/{route}`（专用测试令牌），7 路由 × ~450 次，共 3165 样本；另直连上游对照 + 管理面 `/api/routes/{name}/test` 每 60s。

| route | 上游 | N | 链路通率 | p50 | p90 | p95 | p99 | max | 10s+超时 |
|---|---|---|---|---|---|---|---|---|---|
| openai | vercel ⚠️ | 486 | 100% | 78 | 86 | 91 | 129 | 165 | 0 |
| anthropic | worker | 455 | 100% | 275 | 327 | 355 | 749 | 1457 | 0 |
| opencode | vps | 425 | 100% | 387 | 887 | 922 | 986 | 1305 | 0 |
| xai | vercel ⚠️ | 486 | 100% | 78 | 88 | 94 | 106 | 113 | 0 |
| github | worker | 448 | 100% | 304 | 420 | 495 | 1195 | 1562 | 0 |
| bai | worker | 419 | 100% | 505 | 916 | 977 | 1142 | 1667 | 0 |
| opencode-cf | worker | 446 | 100% | 328 | 436 | 455 | 988 | 3655 | 0 |

- **0 失败**（无 000/超时/5xx），但 openai/xai 持续 402 且 p95 极小（~90ms）——实锤为 **Vercel 平台 402 DEPLOYMENT_DISABLED**（直连 vedge 根路径也是 402，2.0s），**链路本身通、出口服务不可用**。
- 直连上游对照：rn（出口R）23/23 全 200，p95=2565ms ✅；edge（出口C）23/23 全 200，p95=3234ms ✅；vedge（出口V）23/23 402 ⚠️。
- opencode 主上游 rn 窗口内全程可达，**未触发 vps→worker failover**。
- 判定：按阈值（p95>2x 自身 p50）opencode 路由 p95=922ms ≈ 2.38x p50=387ms 触发"不稳定"标记——但绝对延迟小、0 错误、直连出口稳定，属**统计尾部偏斜而非故障**；总体链路判定**稳定**（出口V 服务不可用除外）。

## 4. 轨道C：物理网络路径质量（中国侧 → 五出口）

| 端点 | TCP p50/p95 | TLS p50/p95 | HTTP p50·码 | ICMP 丢包/RTT | 证书到期 | 结论 |
|---|---|---|---|---|---|---|
| rn.ponygo.fun（出口R） | 265/292 | 788/841 | 728·200 | 0% / 254ms | 2026-12-24 | ✅ 美西稳定 |
| vedge.ponygo.fun（出口V） | 82/91 | 261/274 | 244·402 | 0% / 87ms | 2026-12-24 | ✅ 延迟最低，业务 402 ⚠️ |
| vgate.ponygo.fun | 82/89 | 260/274 | 237·402 | 0% / 70ms | 2026-12-24 | ✅ 亚洲边缘 |
| edge.ponygo.fun（出口C） | 258/285 | 803/932 | 742·404 | 0% / 187ms | 2026-12-24 | ✅ 稳定 |
| gate.ponygo.fun | 255/277 | 853/976 | 825·404 | 0% / 186ms | 2026-12-24 | ✅ 稳定（hop10 偶发抖动） |
| opencode.ai | 196/208 | 605/642 | 902·200 | 10% / 187ms | 2026-12-27 | ✅ 可达 |
| api.anthropic.com | 200/208 | 622/628 | 593·403 | 10% / 193ms | 2026-12-20 | ⚠️ 链路通，HTTP 403 属风控非网络 |
| daily-cloudcode-pa.googleapis.com | **全超时** | **全超时** | 000 | 100% | — | 🚨 直连硬阻断（mtr 止于圣何塞） |
| api.openai.com | **全超时** | **全超时** | 000 | 100% | — | 🚨 直连硬阻断 + DNS 三源污染 |
| 100.105.241.39（tencent/tailnet） | :8899 31/32 | — | — | 10%(2/20) / 24ms | — | ✅ tailscale 直连 30ms，TCP 20/20 全通 |

- **无传输抖动热点**：五出口端到端 0% 丢包、RTT 分位稳定；googleapis/openai 是**系统性阻断而非质量劣化**。
- DNS：本地 dnsmasq 解析正常（vedge 池 IP 随 anycast 正常波动）；8.8.8.8 对照 2/3 超时（GFW udp/53 干扰，已知现象）；`api.openai.com` 本地/114/8.8.8.8 三源结果均非同源（污染），但代理链路不受影响（网关侧解析）。

## 5. 轨道D：网络波动对 ponyllm 服务的影响（端到端 + 24h 历史）

真实链路（K8s pod 内探测，窗口 16:49–17:13 CST）：

| 链路 | 样本 | 成功率 | p50 | p95 | max |
|---|---|---|---|---|---|
| 链路1：pod→pproxy-host(tencent)→antigravity googleapis（**ponyllm 真实路径**） | 304 | 99.67% | 1.68s | 3.57s | 8.59s |
| 链路2：pod→devserver 反向网关→opencode zen（**ponyllm 真实路径**） | 136 | 100% | 0.35s | 0.88s | 0.97s |

- 链路1 唯一失败：17:04:20 `RemoteDisconnected`（14.8s，0.33%）。
- **真实流式推理**（经 ponyllm 网关，13 次全 200）：TTFT `auto:economy` p50=12.3s / **p95=48.4s** / max=48.4s；gemini-3.8-flash-high 直连 4.5–7.2s；stall（块间 gap>2s）2 次（最坏 5.72s）；长输出 81 chunks 平滑。
- **24h 历史**（`history-pproxy-journal.txt` / `history-gateway-logs.txt` / `history-telemetry.json`）：
  - pproxy：2248 次 tunnel established（cold 516 次，23%），establish p50=300ms/**max=25.9s**（models.opencode.ai @12:02）；错误类 error=31/timeout=42/denied=69/retry=24，failover=0；
  - ponyllm 网关日志：**3 次 120s 尾部 stall**（12:51/13:51/16:12 CST）+ 今日 14:00–16:00 **antigravity empty-STOP 透明重试风暴 49 次**（单请求最高 11 次重试）；
  - telemetry（09-27 快照）：antigravity max_gap=90s / opencode max_gap=118.9s；最近 24 桶失败率 2.27%。

**影响量化结论**：
- **成功率影响可忽略**：端到端 0 失败，链路失败率 0.33%（<0.5% 阈值）。
- **首包延迟影响显著**：TTFT p95=48.4s（p50 的 3.9 倍，显著超 2x 阈值）、4/12 请求 TTFT>20s；叠加历史 120s 尾部 stall、antigravity 重试风暴与 25.9s 隧道建连极值——**用户感知为"慢"而非"不可用"**。
- 波动集中在：① 链路1 的每请求冷建连基线（1.3–1.5s）与偶发远端断连；② antigravity 上游自身的空响应/慢首包（非纯网络）；③ opencode 上游曾出现 25.9s 单次建连。

## 6. 风险与建议

1. **⚠️ 出口V（Vercel `vedge.ponygo.fun`）DEPLOYMENT_DISABLED——优先级最高**：
   openai/xai 路由未真正到达上游。若 ponyllm 客户端使用 openai（gpt-4o）或 xai 模型，将拿到 402。建议：恢复 Vercel 部署/计费，或按 **B003** 决策将 openai 路由 `override_upstream=worker` 止血（历史 B002 已有先例与完整动作清单）。
2. **antigravity 链路基线重构**：链路1 目标域 24h 内 335 次 establish 全部 pooled=false（每请求冷建连 ~1.4s+）。若需降低 ponyllm TTFT，需让该域进入隧道池复用（改动需评审 `PPROXY_CONSERVE_VERCEL` 与 allowlist 策略），或接受"每请求 +1.4s 结构性延迟"。
3. **ponyllm 侧可观测性强化**：24h 内存在 120s 尾部 stall 与 empty-STOP 重试风暴而未被系统告警（本测试才暴露）——建议对 TTFT>20s、stall>10s、单请求重试>3 次打点告警（ponyllm 网关已具备 flight_recorder 基础）。
4. **周期性复测**：本方法论与脚本已落盘（`PLAN.md`、各轨脚本、解析/分析脚本），建议每周在同一时段复测一次（复用本 ADR 的阈值）。
5. **已处置**：专用测试令牌 `stability-test-20260929` 测试完毕即吊销（见 §7）。

## 7. 测试卫生

- 专用测试令牌（id=26）于测试完成后 **已吊销**（`DELETE /api/tokens/26`）。
- 全部 K8s 操作只读（exec/logs/get/cp），未修改任何资源；未改动任何生产配置；测试负载远低于网关 `verify_concurrency=30`。
- 数据与脚本：`.agents/tests/stability-2026-09-29/`（forward/、reverse/、network/、ponyllm/ 各轨原始 CSV+总结、PLAN.md）。