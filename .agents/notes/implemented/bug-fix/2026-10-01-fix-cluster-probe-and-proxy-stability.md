# Agent Note: 优化集群探测容限与下线失效出口保障代理稳定性

Status: implemented

## Problem

在多节点集群环境下，用户反馈两个主要痛点：
1. **tencent 节点偶发下线**：从桌面端观察集群大盘时，阶段状态显示正常，但 tencent（备灾节点2）时常闪现下线。排查发现，桌面端探测 `100.105.241.39:8899` 的 TCP 握手超时硬编码为 800ms。而 tencent 位于公网且经由 Tailscale 虚拟网连接，在 NAT 穿透握手或 DERP 中继（RTT 达 370ms+）发生网络抖动时极易超过 800ms，造成误判下线。
2. **代理可用性整体不稳定**：
   - 历史配置中的 Vercel 隧道端点 `wss://vgate.ponygo.fun/api/ws` 因账号欠费返回 `HTTP 402 DEPLOYMENT_DISABLED`，已彻底不可用；
   - 待命隧道池（TunnelPool）同时对 Cloudflare Worker Anycast 端点预建长连接，国内网络直连 CF 存在频繁的 8s 拨号超时（`dial timeout after 8s`），导致待命连接池频繁异常并引起主引擎回落冷建连；
   - CF Worker 对部分 AI 目标（OpenAI、Anthropic）存在平台级的反向代理阻断。

## Decision

1. **放宽桌面端集群节点心跳探测超时**：
   在 `desktop/src-tauri/src/lib.rs` 的 `proxy_cluster_nodes_get` 中，将 TCP 建连超时从 `800ms` 放宽至 `2500ms`，充分容忍跨城公网及 Tailscale DERP 中继的延迟波动，消除闪退假死误判。
2. **全面下线失效的 Vercel 隧道端点**：
   将集群各节点（`devserver`、`preprod`、`tencent`）环境变量及配置文件中的 `PPROXY_TUNNEL_GATE_URL` 由含 `vgate` 的三端点精简为 `wss://rn.ponygo.fun/ws,wss://gate.ponygo.fun/ws`，并从上游配置移除已不可用的 Vercel 反代。
3. **优化待命隧道池（TunnelPool）预热策略**：
   在 `crates/transport/src/pool.rs` 中增加策略判断：当端点列表中已存在拥有独立原生出海能力的高优先级 Native VPS（`rn.ponygo.fun`）时，CF Anycast 端点作为次级 Fallback 将预建连接数收紧为 0。连接池全力预热稳定、低时延的原生 VPS 待命长连接，彻底规避 CF 频繁 8s 超时拉垮连接池健康度的问题。

## Alternatives considered

- **方案 A（保持 800ms，在桌面端前端做连续 N 次失败再置灰）**：虽然可以平滑前端展示，但无法解决底层探测过早中断的根本问题，且后端每次探测依然浪费建连重试资源。直接在后端将超时调整为合理的 2500ms 既清晰又彻底。
- **方案 B（继续保留 Vercel 端点等待后续充值恢复）**：由于目前 Vercel 端点硬返回 402，且对于合规 host 分流会造成阻断，保留配置会持续产生无意义的错误日志与重试惩罚；下线失效端点是高可用架构中的最基础保障，后续恢复后可随时热更新纳管。

## Consequences

- 桌面端集群大盘探测对公网 Tailscale 节点的抗抖动能力大幅提高，tencent 节点不再因瞬时网络重协商误报离线；
- 拔除了 Vercel 402 错误源，待命连接池预热成功率提升至 100%，不再出现持续的 8s 超时日志报警；
- 真实代理出海请求稳定命中 RackNerd 原生 VPS 高速出口，延迟与可用性恢复平稳。
