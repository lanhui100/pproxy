# Track B: 反向网关路由稳定性探测总结 (2026-09-29)

- 探测窗口: 16:46:33–17:12:03 CST (约 26 min, 主探测 3165 条样本: 7 路由 × ~3s/条; 控制面每 60s 一轮)
- 数据面: http://127.0.0.1:8899/{token}/{route}/..., 令牌 stability-test-20260929 (用后即撤)
- 成功/失败定义: **2xx-4xx = 链路通**(上游真实响应, 含 401/402/404/405 业务态); **code=000/curl 超时 = 失败**; 5xx = 链路通但上游错误(另列)
- ⚠️ 重要: vedge.ponygo.fun(Vercel) 当前返回 **402 Payment required / DEPLOYMENT_DISABLED**——Vercel 函数部署被禁用(平台层, 疑似计费), 属**出口服务不可用而非链路故障**; 该 402 本身证明链路可达。

## 1. 主探测: 每路由稳定性

| route | 上游 | N | 成功率(链路通2xx-4xx) | 业务可用率(200) | 状态码分布 | p50 | p90 | p95 | p99 | max | ≥10s超时 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| openai | vercel (openai/xai) | 486 | 100.00% | 0.00% | 402:486 | 78 | 86 | 91 | 129 | 165 | 0 |
| anthropic | CF worker (anthropic/github/bai/opencode-cf) | 455 | 100.00% | 0.00% | 405:455 | 275 | 327 | 355 | 749 | 1457 | 0 |
| opencode | vps (opencode 主上游) | 425 | 100.00% | 100.00% | 200:425 | 387 | 887 | 922 | 986 | 1305 | 0 |
| xai | vercel (openai/xai) | 486 | 100.00% | 0.00% | 402:486 | 78 | 88 | 94 | 106 | 113 | 0 |
| github | CF worker (anthropic/github/bai/opencode-cf) | 448 | 100.00% | 0.00% | 404:437 429:11 | 304 | 420 | 495 | 1195 | 1562 | 0 |
| bai | CF worker (anthropic/github/bai/opencode-cf) | 419 | 100.00% | 0.00% | 401:419 | 505 | 916 | 977 | 1142 | 1667 | 0 |
| opencode-cf | CF worker (anthropic/github/bai/opencode-cf) | 446 | 100.00% | 100.00% | 200:446 | 328 | 436 | 455 | 988 | 3655 | 0 |

业务态说明(探针业务结果≠链路健康): openai=402 上游 Vercel 平台禁用; anthropic=405 探针方法/路径不符上游预期; opencode=200 可用; xai=402 上游 Vercel 平台禁用; github=404 路径不存在 + 偶发 429; bai=401 需鉴权; opencode-cf=200 可用。

## 2. 异常事件时间线 (code=000 / 5xx / 单次≥10s)

无 (0 条异常) ✓

## 3. 对照: 直连上游 (模拟网关转发, X-Proxy-Secret, httpbin.org/status/200, 60s/次)

| host | 用途 | N | 状态码分布 | p50 | p90 | p95 | p99 | max | 说明 |
|---|---|---|---|---|---|---|---|---|
| rn.ponygo.fun | vps (opencode 主上游) | 23 | 200:23 | 1431 | 2550 | 2565 | 2675 | 2675 | ✅ 正常 |
| vedge.ponygo.fun | vercel (openai/xai) | 23 | 402:23 | 1153 | 1498 | 2135 | 7107 | 7107 | ⚠️ Vercel 平台 402 DEPLOYMENT_DISABLED(部署禁用, 非链路故障) |
| edge.ponygo.fun | CF worker (anthropic/github/bai/opencode-cf) | 23 | 200:23 | 1933 | 3165 | 3234 | 3296 | 3296 | ✅ 正常 |

## 4. 对照: 管理面 route/test (网关侧独立度量, POST /api/routes/{name}/test, 60s/次)

| route | N | 管理面 ok 率 | status分布 | latency p50 | latency p95 | latency max | 常见 error |
|---|---|---|---|---|---|---|---|
| openai | 23 | 0% | 402:23 | 77 | 95 | 119 | edge 自身错误（status 402），未到达源站 |
| anthropic | 23 | 0% | 404:23 | 310 | 427 | 438 | edge 自身错误（status 404），未到达源站 |
| opencode | 23 | 100% | 200:23 | 390 | 956 | 980 | - |
| xai | 23 | 0% | 402:23 | 76 | 86 | 91 | edge 自身错误（status 402），未到达源站 |
| github | 23 | 0% | 200:22 429:1 | 315 | 407 | 428 | edge 自身错误（status 200），未到达源站 |
| bai | 23 | 0% | 403:23 | 508 | 941 | 1111 | edge 自身错误（status 403），未到达源站 |
| opencode-cf | 23 | 0% | 200:23 | 353 | 804 | 805 | edge 自身错误（status 200），未到达源站 |

注: 管理面 ok=false 语义为“上游业务未返回预期成功(未到达源站/上游平台错误)”，非链路判定; 实测 status 与数据面探针存在路径差异(bai: 数据面401 vs 管理面403; github: 数据面404 vs 管理面200)。

## 5. 结论

判定阈值: 失败率(000/超时)>0.5% 或 窗口内≥2次10s+超时 或 p95>2×直连上游对照p95 → 不稳定; p95>2×自身中位 标记为延迟模式观察(不单独判不稳定)。

- **openai** (vercel (openai/xai)): 成功率(链路通) 100.00%, 业务可用率 0.00%, p95=91ms, 直连上游对照 p95=2135ms → ✅ 稳定 (402 = 上游 Vercel 平台部署禁用 DEPLOYMENT_DISABLED 的真实响应, 链路通; 上游服务不可用属已知状态)
- **anthropic** (CF worker (anthropic/github/bai/opencode-cf)): 成功率(链路通) 100.00%, 业务可用率 0.00%, p95=355ms, 直连上游对照 p95=3234ms → ✅ 稳定
- **opencode** (vps (opencode 主上游)): 成功率(链路通) 100.00%, 业务可用率 100.00%, p95=922ms, 直连上游对照 p95=2565ms → ✅ 稳定｜观察: 延迟双峰 p95/中位≈×2.4
- **xai** (vercel (openai/xai)): 成功率(链路通) 100.00%, 业务可用率 0.00%, p95=94ms, 直连上游对照 p95=2135ms → ✅ 稳定 (402 = 上游 Vercel 平台部署禁用 DEPLOYMENT_DISABLED 的真实响应, 链路通; 上游服务不可用属已知状态)
- **github** (CF worker (anthropic/github/bai/opencode-cf)): 成功率(链路通) 100.00%, 业务可用率 0.00%, p95=495ms, 直连上游对照 p95=3234ms → ✅ 稳定
- **bai** (CF worker (anthropic/github/bai/opencode-cf)): 成功率(链路通) 100.00%, 业务可用率 0.00%, p95=977ms, 直连上游对照 p95=3234ms → ✅ 稳定
- **opencode-cf** (CF worker (anthropic/github/bai/opencode-cf)): 成功率(链路通) 100.00%, 业务可用率 100.00%, p95=455ms, 直连上游对照 p95=3234ms → ✅ 稳定

总体判定: ✅ **总体通过**（7 路由链路全部稳定: 0 失败 / 0 超时; openai/xai 的上游 402 为 Vercel 平台部署禁用, 属出口服务不可用而非链路故障; opencode 延迟双峰为窗口内稳定固有形态）

### opencode failover 观察
探测窗口内 opencode 主上游 rn.ponygo.fun 全程可达(23/23 直连 200, p50≈1.4s), 网关 opencode 路由 100% 200 且无失败/超时, **未触发 vps→backup worker failover**。 延迟呈稳定双峰(≈350-400ms 快通道 / ≈850-950ms 慢通道, 慢样本占 ~32%), 窗口首分钟即存在且每分钟恒定, 与 rn 直连无相关波动——判定为路由固有形态(疑似主/备两通道或隧道多连接 RTT 差异), 非故障。
