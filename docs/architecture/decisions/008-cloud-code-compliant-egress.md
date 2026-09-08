# ADR-008: Cloud Code 系目标固定走合规物理出口 + CF gate 出站地理门禁

- 状态: 已实现（Rust 待发布；CF worker 待部署） | 日期: 2026-09-08

## 背景

Antigravity CLI（agy）在本机频繁中断，报错形如：

```
Agent execution terminated due to error.
Error ID: ab8b023b-7b45-4039-b861-450368d935ad-4
```

取证结论（2026-09-08）：

- `Error ID` 是 CLI 本地关联 ID（`<trajectory_id>-<步号>`），底层错误是 Google 侧
  `HTTP 400 FAILED_PRECONDITION: User location is not supported for the API use.`
  （`daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent`，SSE 携带该错误）。
  日志累计 99 次 agent-executor 级失败、12 个对话受影响，跨 09-05 ~ 09-08。
- 本机直连 Google 完全不通（v4/v6 均超时），agy 全程走 `http_proxy=127.0.0.1:8899`
  → pproxy 数据面 → gate 隧道。Google 判定的"来源地区"就是**隧道出站 IP 的地区**。
- 实测两条出口：CF gate 出口是轮换的 Cloudflare anycast 池
  （`104.28.152.x / 104.28.158.x / 104.28.165.x`，AS13335，8 次探测含 1 次超时）；
  Node/Vercel 侧 gate 出口稳定落在真实机房网段（AS14618 us-east-1，实测 `18.233.6.83` → `3.93.80.51`）。
- 既有防线为何没挡住：
  1. `gate-policy.mjs::shouldBlockColo` 只看**入站握手 colo**（`request.cf.colo`），
     而 `connect()` 的**出站 egress IP 由 CF 另行分配**，两者不保证同地区；
  2. `route.rs::order_endpoints`（Google → 非 CF 出口优先）**只被桌面端调用**，
     `crates/server` / `crates/engine` 两份数据面 CONNECT 实现都按配置顺序建连（CF 在前）；
  3. 运行中的 pproxy-server 带 `PPROXY_CONSERVE_VERCEL=1`，按既有策略泛 Google 优先 CF；
  4. failover 只在**建连失败**时触发；Google 的 400 发生在隧道建立之后，
     代理只是端到端密文转发，看不到也改不了状态码。

## 决策

**A. CF gate 增加"出站 IP 地理"门禁（仅对 Cloud Code 系 host 生效，fail-closed）**

- 新增 `egress-probe.mjs`：用 `connect()` + `startTls()` 探测本 Worker 出站 IP 的国家码。
  用 `connect()` 而非 `fetch()`——要测的就是转发 Google 流量走的那条出站链路。
- 新增 `egress-geo.mjs`：isolate 级缓存（TTL 5min 复用、stale 30min 回退、并发去重）。
- 新增 `gate-policy.mjs::shouldBlockEgress`：仅对 `COMPLIANT_EGRESS_SUFFIXES` 生效；
  国家码不在白名单或探测结果 unknown → 返回 `unsupported_egress:<CC|UNKNOWN>` 并关闭 WS，
  由客户端 failover 到真实机房出口。非合规 host 一律放行（探测异常时不得把泛 Google
  流量倾泻到 Vercel）。

**B. 数据面按目标 host 重排 gate 端点，合规 host 例外高于省额度策略**

- `route.rs` 新增 `COMPLIANT_EGRESS_SUFFIXES` / `requires_compliant_egress` /
  `ordered_gate_urls`；`order_endpoints` 拆出纯函数内核 `order_endpoints_with`。
- `requires_compliant_egress` 命中时无视 `PPROXY_CONSERVE_VERCEL`，始终非 CF 出口优先
  （省额度不能以牺牲可用性为代价；例外只覆盖 3 个 host，Vercel 流量可控）。
- `crates/server`（线上数据面）与 `crates/engine`（`pproxy serve` 嵌入式网关）
  两份 CONNECT 实现统一改走 `ordered_gate_urls`，池化 `checkout_ordered` 同步按序取会话；
  桌面端经 `order_endpoints` 自动生效。
- 保留 CF 端点作为兜底：单端点硬钉会把 Vercel 侧抖动放大成 Antigravity 全挂。

**C. 顺带修 gate worker 的"半死隧道"bug**

`worker.js` 的 `sock.readable.pipeTo(...)` 只挂了 `.catch`：上游**正常** EOF 时
`pipeTo` 是 resolve 而非 reject，WS 永不关闭，隧道悬挂（实测 CF 侧 >210s 仍存活，
而 Node 版 gate 正确返回 `upstream closed`）。改为 `.then(closeUpstream, closeUpstream)`。

## 后果

- ✅ agy 的模型调用固定先走真实机房出口，地区限制类 400 从"间歇性"变为"不出现"
- ✅ Vercel 出口只承接 3 个 Cloud Code host，泛 Google（YouTube/Play 等）仍走 CF，额度开销可控
- ✅ CF gate 的自检能兜住"入站 colo 合规但出站 IP 不合规"这一此前无解的盲区
- ✅ 半死隧道消除，`EOF` 类报错的一个来源被移除
- ⚠️ CF worker 侧依赖 `connect(..., { secureTransport: 'starttls' })` + `socket.startTls()`；
  缺少该选项时 `startTls()` 会抛 `secureTransport must be set to 'starttls'`（首次部署即踩到）。
  若运行时不可用，探测恒为 unknown → 该 host 恒 failover 到兜底出口（安全侧降级，不是静默放行）
- ⚠️ 首次 bind 需多等一次探测（~200ms，TTL 内复用）；探测超时上限 1.5s，
  低于数据面 `FIRST_FRAME_TIMEOUT`(3.5s)，不会把 bind 拖成假失败
- ⚠️ 两份 Rust 清单与一份 JS 清单需人工保持一致——已用
  `scripts/check-egress-parity.sh` 机械校验
- ❌ 未处理：Vercel 用量仍未计量（`/api/quota` 的 cf/vercel source 均 disabled，
  正向隧道字节数走 `TrafficCounter` 的 Noop 实现）；账号侧 429/403 与本决策无关

## Alternatives considered

1. **泛 Google 全走 Vercel**：违背省额度目标（YouTube/Play 等大流量会刷爆出口配额），
   且把全部 Google 依赖压到单条出口上。否决。
2. **只调整 CF 侧的 colo 白名单**（加更多"合规"colo）：管不到 `connect()` 的实际出站 IP，
   09-02 已加过 colo 门禁，09-05/06/08 仍继续报错，实证无效。否决。
3. **把 Cloud Code host 硬钉到 vgate 单端点**：Vercel 侧限额/函数时长上限会把
   Antigravity 直接打挂（`vercel.json` 声明 `maxDuration=120`，与 `api/ws.js` 注释的 300 不一致，
   单条 SSE 可能被截断）。改为"优先 + 兜底"两级。否决。
4. **在 pproxy 里对 Google 400 做应用层重试/改写**：CONNECT 是端到端 TLS 密文，
   代理看不到状态码，无法在不 MITM 的前提下识别该错误。否决。
5. **复活 Render 兜底出口**（`31b2053` 曾以"未上线"移除）：引入第三家出口的运维与额度成本，
   且不能解决"选错出口"的判定问题。搁置。
6. **只改桌面端 `order_endpoints`**：agy 走的是数据面 8899，桌面端路径与本次故障无关。否决。
7. **CF worker 用 `fetch()` 探测出站 IP**：子请求可能被 CF 骨干改道到另一个 colo，
   与 `connect()` 出口不保证一致，测的不是 Google 看到的那条路径。否决。
8. **探测失败时 fail-open（放行）**：等于在不确定地区继续打 Google，
   故障照旧且更难定位。改为 fail-closed（仅限这 3 个 host）。否决。

## 验证

```bash
cargo test --workspace                                    # 含新增 8 条 route 用例 + 2 条数据面用例
node deploy/cf-gate-worker/gate-policy.test.mjs           # 75 pass
node deploy/cf-gate-worker/egress-geo.test.mjs            # 19 pass
bash scripts/check-egress-parity.sh                       # 两侧 host 清单一致性
# 出站地理探测自检（需 tunnel token）：返回本 Worker 出站 IP/国家码与是否合规
curl -s https://gate.ponyjob.top/debug/egress \
  -H "Authorization: Bearer <tunnel_token>"
# 线上出口实测（只读，需 tunnel token）
node deploy/vercel-gate-worker/smoke-test.mjs wss://vgate.ponyjob.top/api/ws \
  daily-cloudcode-pa.googleapis.com --token <tunnel_token>
```

**上线记录（2026-09-08）**：CF worker 已部署（`pony-gate`，此前线上版本停留在 2026-08-31，
即 A/C 从未上线——这解释了 09-05/06/08 的继续报错）。部署后实测：

- `/debug/egress` → `{"ip":"104.28.165.52","country":"US","compliant":true}`
- bind `daily-cloudcode-pa.googleapis.com` → `{"ok":true}`
- 负向验证（临时 `EGRESS_ALLOWED_COUNTRIES=CN` 部署）→ `/debug/egress` 显示
  `compliant:false`，bind 返回 `{"ok":false,"reason":"unsupported_egress:US"}`，随后已恢复正式配置
- 数据面实测（`ss -tinp` 字节计数器差分）：合规 host 走 vgate `66.33.60.x`，
  对照 `oauth2.googleapis.com` 仍走 CF gate，省额度策略未退化

部署顺序：先发 Rust（数据面立即生效）→ 再部署 CF worker（`cd deploy/cf-gate-worker && npx wrangler deploy`）。
CF 部署凭据：`CLOUDFLARE_API_TOKEN`（需 Account → Workers Scripts → Edit）+ `CLOUDFLARE_ACCOUNT_ID`；
本机可用的 token 在 `~/.wrangler/config/default.toml`（`.pproxy.env` 里的 `cfat_` 账户级 token
缺 Workers 权限，仅够读账户信息，不能部署）。

后续排查同类问题请走手册：[`docs/ops/ANTIGRAVITY-CLI-ERRORS.md`](../ANTIGRAVITY-CLI-ERRORS.md)
（错误图谱 + 分层判定决策树 + `scripts/antigravity/diag/` 工具）。
