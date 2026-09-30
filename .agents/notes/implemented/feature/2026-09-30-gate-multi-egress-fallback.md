# Agent Note: gate-multi-egress-fallback

Status: implemented

## Problem

gate worker（`deploy/cf-gate-worker/worker.js`）出站只有一条路：`cloudflare:sockets`
`connect()` 直连目标（仅 443）。研究 cmliu/edgetunnel（46k⭐）后确认，这正是 CF
当前重点风控的代理模式（封号/滥用报告/1011 实证），且 CF 一收紧 `connect()` 出口，
pproxy 整条 gate 隧道链路失效——无兜底、无弹性。需求：直连失败/被限制时自动切
**用户自配**的反代/链式出口，且不能牺牲 Google Cloud Code 合规门禁
（`gate-policy.mjs`）。

## Decision

给 gate worker 增加**多出口兜底**：直连失败 → 依次尝试用户配置的 SOCKS5 链式出口
与 ProxyIP 反代出口，全部失败才向客户端报错（客户端既有 failover 到 Vercel 逻辑
不变，作为最后一道兜底）。

1. **配置**（`wrangler.toml` [vars]，默认全部留空 = 行为与现状完全一致，绝不默认
   走任何第三方中转——规避 edgetunnel 的"默认走作者 ProxyIP"信任问题，负向清单）：
   - `SOCKS5_PROXY`：`host:port`，用户自有 VPS SOCKS5；
   - `PROXYIP_HOST`：`host[:port]`（默认 443），SNI 反代中继；
   - `SOCKS5_COUNTRY` / `PROXYIP_COUNTRY`：兜底出口的**声明国家码**（如 US），
     供合规门禁判定；**不声明 = 该兜底对合规 host 一律禁用**（fail-closed）；
   - `EGRESS_ATTEMPT_TIMEOUT_MS`：单通道建立超时（默认 8000），防止黑洞挂死。
2. **通道顺序**：直连（现状优先）→ SOCKS5（若配置）→ ProxyIP（若配置）；每个通道
   在超时内未 `opened` 即判失败切下一个；成功通道通过响应 `via` 字段上报客户端
   （`{"ok":true,"via":"direct|socks5|proxyip"}`，客户端只读 `ok` 字段，向后兼容）。
3. **合规贯通**（P0-1 核心约束，不破坏护城河）：直连通道维持现状门禁（colo +
   出站 geo 探测）；兜底通道**必须先过同一套 `gate-policy.mjs` 判定**：
   - **SOCKS5**：CONNECT 目标 = 首帧声明 host，门禁可信——`requiresCompliantEgress`
     的 host 仅当 `SOCKS5_COUNTRY` 声明且 `shouldBlockEgress` 放行时可用，
     未声明/未放行禁用（fail-closed）；
   - **ProxyIP（SNI 反代）**：worker 仅透传字节、真实出口目标由隧道内 TLS SNI
     决定（worker 不可见），门禁判定对象（首帧 host）与真实转发对象无对应关系，
     **合规 host 一律禁用该通道**（对抗审核 P0-2 收紧）；仅非合规 host 可用，
     与直连同口径；
   - 非合规 host（通用流量）兜底不做 geo 限制（与直连现状一致，防误伤大流量）。
4. **实现**：SOCKS5 握手（greeting/connect 域名请求，参照 `crates/core/src/relay.rs`
   的 `socks5_connect` 语义）与通道编排收敛到新模块 `egress-fallback.mjs`（纯函数，
   node 可直接单测，含真实本地 SOCKS5 服务器端到端用例）；`worker.js` 只负责
   把 `cloudflare:sockets` 的 `connect()` 传入并编排重试。
5. **日志**：每次兜底切换打 `[gate] fallback via=…`，可观测兜底是否生效。

## Alternatives considered

- **A. 只做 ProxyIP 反代、不做 SOCKS5**：落选。SOCKS5 复用 pproxy 集群/HA 的
  "用户自有 VPS" 心智（edgetunnel 链式代理同为四通道之一），且不与任何第三方信任
  链耦合；ProxyIP 反代依赖中继方可靠性与信誉，默认留空后价值有限，两者并列互补。
- **B. 兜底通道绕过合规门禁（仅限非 Google 流量）**：落选。风险面分析显示
  Cloud Code 系 host 的 geo 门禁一旦绕过，Google 会直接拒连（400
  FAILED_PRECONDITION），且 CF 出口与反代出口混合后不可审计；本决策保持
  "声明国家码 + shouldBlockEgress" 的显式放行，宁可少一条逃生通道也不破坏合规。
- **C. 并发竞速（edgetunnel TCP_CONCURRENT_DIAL 语义）**：落选。workerd 每个
  isolate 并发 TCP 有限，且 pproxy 客户端侧已有 Vercel failover + TunnelPool 预建
  连接（1 RTT），gate 侧顺序兜底足够；并发竞速把复杂度留在 worker，收益边际低。
- **D. 默认走 edgetunnel 作者 ProxyIP 中转**：落选（负向清单）。默认第三方中转
  引入不可审计信任链（issue #1135 争议），pproxy 是自托管工具，默认必须留空。
- **E. DoH TXT 发现反代池**：落选 v1。TXT 池发现是 edgetunnel 为"免配置"设计；
  pproxy 多用户/合规场景下显式单点配置更可审计，池化留给未来（backlog 备注）。

## Consequences

- gate 隧道在 CF 直连受限时仍可用（SOCKS5/ProxyIP 兜底），生存级短板被补齐；
- 合规口径如实收敛（对抗审核）：Google Cloud Code 系仅可能走直连（白名单出口）
  或**CONNECT 目标可验证的 SOCKS5**（声明国家码 + `shouldBlockEgress`）；ProxyIP
  通道不参与合规出口判定（合规 host 禁用）；声明国家码是运维信任承诺，误声明
  会让 Google 拒连，部署验收需核对声明与实测一致；
- 客户端无需改动（`via` 字段向后兼容）；兜底配置态在 **Bearer 保护的
  `/debug/egress`** 可见（/debug 匿名端点不再暴露，对抗审核 P1）；
- 部署侧新增一组可选 vars（见 `wrangler.toml` 注释与 `docs/ops/DEPLOY.md`），
  默认零配置行为不变；
- 验收口径：`node deploy/cf-gate-worker/egress-fallback.test.mjs`（非零退出）覆盖
  通道编排/超时/SOCKS5 端到端/合规门禁；生产验证靠部署后断直连观察（标注靠 review）。
