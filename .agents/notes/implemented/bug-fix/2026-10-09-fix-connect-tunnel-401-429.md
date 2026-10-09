# Agent Note: 正向 CONNECT 出海隧道全量失效修复（数据面凭据与租户令牌分离）

Status: implemented

## Problem

2026-10-09 巡检发现正向 CONNECT 出海隧道全量失效（反向路径式路由正常）：

- `pproxy doctor` 唯一 fail 项：`CONNECT tunnel probe: oauth2.googleapis.com:443 → 无响应`；
- `pproxy.service` 日志以 6 秒为周期刷
  `tunnel pool refill wss://rn.ponygo.fun/ws failed: HTTP error: 429 Too Many Requests`，
  伴随 `tunnel establish failed ... error=network: HTTP error: 401 Unauthorized`
  （目标 host 无关：opencode.ai / googleapis / oauth2 / mtalk 全部一致）；
- 客户端（集群内外）一致得到 `502 Bad Gateway / x-pproxy-reason: tunnel_failed`；
- 对照组路径式反向路由 `http://100.95.193.103:8899/pony_<PATH_TOKEN>/opencode/zen/v1/...`
  实测 200（请求能抵达 zen 应用层）。

### 根因定位（实测证据链）

数据面端点列表 `PPROXY_TUNNEL_GATE_URL=wss://rn.ponygo.fun/ws,wss://gate.ponygo.fun/ws`，
数据面建连统一携带 `PPROXY_TUNNEL_TOKEN`（一个 `usr_live_` 租户令牌，`sub=usr_dm, max_conns=10`）：

1. **RN gate（主端点）429**：Rust gate（`crates/gate-server`）对 `usr_live_` 用户令牌
   执行租户并发熔断 `max_conns.max(10)*4 = 40`（`handle_ws` §344-366），
   而 `usr_dm` 的并发计数长期饱和在 40/40（数据面自身 refill 风暴 + 桌面端探活 +
   其他客户端共享该令牌额度，gate 侧会话在双向 relay 未完全退出前不释放计数）。
   实测 HTTP/1.1 WS Upgrade + 该令牌 → `429 Concurrent Connections Limit`（nginx 日志同证）。
2. **CF gate（兜底端点）401**：CF Worker（`deploy/cf-gate-worker/worker.js` §142-148）
   按 `sha256(presented) == env.TUNNEL_TOKEN_HASH` 鉴权；租户令牌的 sha256 ≠ 该 hash → 恒 401。
   实测该端点接受明文 operator 令牌 `GATE_TUNNEL_TOKEN`（`gate_ea73485...`）→ 101。
3. **两个端点依次失败**（RN 429 → CF 401），`establish_with_endpoints` 上报末次错误 401，
   最终全部 502 `tunnel_failed`。**结论：VPS 侧 Gate 的凭据/额度问题，与目标站点限流无关。**

## Decision

**数据面 CONNECT 隧道凭据与下发到桌面端的租户令牌分离**：

1. `crates/server/src/connect.rs::TunnelConfig::from_pool_config_and_env`
   新增数据面专用令牌环境变量 `PPPROXY_TUNNEL_TOKEN_DATA`（优先），
   缺省回退 `PPPROXY_TUNNEL_TOKEN`（向后兼容）；
2. `.pproxy.env` 注入 `PPPROXY_TUNNEL_TOKEN_DATA=<GATE_TUNNEL_TOKEN 明文>`——
   该令牌与 RN gate 与 CF worker 的 `TUNNEL_TOKEN_HASH` 均对应（实测 RN hash
   == sha256(GATE_TUNNEL_TOKEN) MATCH、CF 101），且走 RN gate 单口令兼容路径
   （无租户并发/配额判定），双端点全通；
3. `tunnel.rs`（桌面端「自动配置」下发）保持使用 `PPPROXY_TUNNEL_TOKEN`
   （租户令牌），不动——租户额度/并发语义不回归、operator 令牌不外发。

关联记录：数据面 operator 凭据不走租户额度判定，与
`2026-10-03-admin-token-unmetered-gateway-quota.md` 的"管理员豁免租户配额"同构
（均为 operator 面与租户面额度语义分离）；CF 兜底端点鉴权行为见
`deploy/cf-gate-worker/worker.js`（`2026-09-30-gate-multi-egress-fallback.md`）。

## Alternatives considered

- **A（直接改 `PPPROXY_TUNNEL_TOKEN` 为明文 operator 令牌）**：被拒。该变量同时是
  管理面 `tunnel.rs` 下发桌面端的租户令牌源；改之等于把 operator 明文凭据发给所有
  租户，绕过 RN gate 租户额度/并发判定，且令牌轮换时全桌面端需重配——安全回归。
- **B（重签发租户令牌抬高 `max_conns` 缓解 429）**：被拒。只治 429 不治 401
  （CF 兜底端点对租户令牌恒 401，故障日志不达标）；且提高共享令牌的并发上限等于
  放大租户间互相挤占，掩盖 gate 会话计数不释放的潜在问题。
- **C（从数据面端点列表摘除 CF 端点）**：被拒。RN 主端点故障时失去兜底；
  且未解决"为什么 401"——错误从显式 401 变隐式缺失，排障更差。
- **D（gate 侧修复租户并发计数/会话回收）**：本变更不采纳。会话计数饱和是租户
  令牌共享下的既有行为（桌面端/探活/数据面共用 usr_dm 额度），非数据面失效的
  必要条件；gate 侧会话生命周期加固（空闲回收、relay 任一侧完成即释放）列为
  后续候选（见 Consequences 残留风险）。

## Consequences

- 数据面 CONNECT：RN 主端点与 CF 兜底端点均接受 operator 令牌，refill 预池化恢复
  （pooled=true），doctor CONNECT probe 通过，CONNECT 请求返回上游应用层状态码而非 502；
- 日志不再出现 401/429（数据面视角）；租户令牌的桌面端/探活流量不受本变更影响
  （其与数据面不再共享并发计数）；
- **残留风险（未在本变更处理）**：
  1. RN gate 对 `usr_live_` 租户令牌的并发计数（40）由多客户端共享仍可能饱和，
     gate 侧会话在双向 relay 未完全退出前不释放（`handle_ws_socket` 用
     `tokio::join!` 等两侧退出），且 nginx `/ws` `proxy_read_timeout 86400s`
     会把静默死客户端连接保持 24h——建议后续给 gate 加 WS 空闲回收与
     relay `select!` 释放，并将 nginx 该 timeout 降到分钟级；
  2. 桌面端对 CF 兜底端点仍持租户令牌（恒 401）——桌面端自动配置若下发租户令牌，
     其 CF 出口探活会失败（现有 egress 探活告警即此表现），需要时按本 ADR 同款
     思路为桌面端引入 per-出口令牌。
