# PProxy 代理网络稳定性测试计划 (2026-09-29)

> 目的：全面验证 pproxy 分布式代理网络稳定性，量化网络波动对 ponyllm 服务的影响。
> 方法：四路并行探测 + 24h 历史证据。ADR: `.agents/notes/implemented/testing/2026-09-29-proxy-network-stability-test.md`

## 拓扑（已核实）

```
[devserver 100.95.193.103]
  ├─ :8899 数据面 pproxy-server (systemd, up 1d8h) —— 正向 CONNECT 隧道池 + 反向 /{token}/{route}
  ├─ :8900 管理面 (仅 tailnet)  admin_token=pony_admin_88d1b64c486f0a1cbdfbb089a5453ef6cf0b3afda0787f7a
  └─ 出口隧道池: wss://rn.ponygo.fun/ws (RackNerd VPS, 主) | wss://vgate.ponygo.fun/api/ws (Vercel, 合规冷建连) | wss://gate.ponygo.fun/ws (CF)
[tencent 100.105.241.39:8899]  pproxy-host —— K8s ponyllm 出口（Basic auth user:0d1fa1…）
[反向路由] openai→vercel(vedge.ponygo.fun) | anthropic/bai/github/opencode-cf→worker(edge.ponygo.fun) | opencode→vps(rn.ponygo.fun)+backup worker | xai→vercel
[ponyllm K8s] ponyllm-pod-service:8080 网关 key=sk-pony-7cc4cd2c0cb646a9a571067ce89eefa9
              antigravity→pproxy-host:8899 | opencode/zen→http://100.95.193.103:8899/pony_31abcbd448a003be0ea27524d60973d8/opencode/zen/v1
```

## 测试令牌
- 数据面：`pony_cdfc6aa12c466989207e5c65e34cd2bc`（stability-test-20260929, id=26, 用后即撤）
- 上游直连 secret（X-Proxy-Secret）：`b96ee9195c590bc9a08c0284814bf1fead74e2f626d931ab14c892effa8a5882`

## 输出目录（各轨道专用，写范围不相交）
`/home/dm/pproxy/.agents/tests/stability-2026-09-29/{forward,reverse,network,ponyllm}/`

## 判定阈值
- p95 延迟 > 2x 基线 或 错误率 > 0.5%（网络类） 或 窗口内 ≥2 次 10s+ 超时 → 不稳定
- 每轨道交付：原始 CSV + summary.md（分位/错误率/异常事件/结论）

## 流程
1. Lead 建 ADR + 令牌 + PLAN ✓
2. Agent A/B/C/D 并行执行（~25min 探测窗口，起点 `date +%s` 记录）
3. Lead 汇总四路结果 → `docs/ops/stability-report-2026-09-29.md`
4. 吊销测试令牌
