# Render 美区隧道出口（第三 egress）— Spec v0.1

> 目标：解决 Antigravity CLI 经 pproxy 隧道访问 Google 时反复出现的
> `FAILED_PRECONDITION (code 400): User location is not supported`（2026-08-31 会话 DB 取证确认，
> 见 §1），为 CONNECT 隧道提供出口地区固定在美国的兜底 gate（Render 免费容器，无 VPS、零成本）。
> 顺带把 Render 建立为 CF/Vercel 之外的第三出口（路径中继能力一并提供，接入留待后续）。

---

## 0. 状态与断点

**状态**：阶段 2 完成，待阶段 3（用户 Render 建站 + 实测）
**当前断点**：代码全部入库（`ec17e1a` + 安全回改），selftest 28/28、gate-policy 28/28 全过。
**Next Action**：T3 —— 用户按 `deploy/render-gate/README.md` 在 Render 建站（Oregon/Free）→ 配置双 gate URL → 执行 §T3 验收三步。
**Resume Hint**：恢复时读本节 + §4 门禁表；独立代码审核因子代理基础设施故障降级为主会话独立 pass（§8.2 如实标注）。

| 阶段 | 内容 | 状态 |
|------|------|------|
| 阶段 0 | spec + 双对抗审核（架构 1 路完成；安全 2 路中途失败，降级主会话 pass） | **Done** |
| 阶段 1 | `deploy/render-gate/` + CF worker colo 门禁 + 单测/自检 | **Done** |
| 阶段 2 | 测试门禁（selftest 28/28、gate-policy 28/28、cargo 回归）+ 代码审核 | **Done**（降级口径见 §8.2） |
| 阶段 3 | Render 建站（用户操作）+ 配置接入 + agy 实测 | **Pending（用户操作）** |

## 1. 背景（取证结论）

2026-08-31 对 `~/.gemini/antigravity-cli/conversations/*.db` 原始字节扫描（`scripts/agy_db_rawscan.py`）：
"Agent execution terminated due to error" 后随的后端原文为：

```
FAILED_PRECONDITION (code 400): User location is not supported for the API use. HTTP 400 Bad Request
Headers: {"Server":["ESF"], ...}   ← Google 前端真实应答，TLS 隧道本身是通的
```

根因：agy → CONNECT → pproxy → CF gate worker（`cloudflare:sockets` 出口）→ Google。
CF worker 出口地区 = 处理请求的 colo 地区，中国大陆用户常被调度到 HKG，而香港是 Gemini/Antigravity
不支持地区。colo/出口 IP 调度有波动性 → 表现为时好时坏、运行中断。历史另有 429 配额错误（与本方案无关）。

**已排除的假设**：WS 空闲断连（心跳缺失）——错误原文是后端明确 400，非流中断。

## 2. 关键既有资产（改变工作量的代码事实）

| 资产 | 事实 | 影响 |
|------|------|------|
| 桌面端多端点故障转移 | `desktop/src-tauri/src/proxy/engine_tunnel.rs::establish`：URL 按 `,`/`;`/`\n` 分隔，逐端点尝试，**denied 亦 fallover** | 桌面端零改动即可接入第二 gate |
| gate 协议 | `deploy/cf-gate-worker/worker.js`：WS Upgrade（Bearer）→ 首帧 Text `{"host","port"}` → `{"ok":true}` → Binary 双向透传；`{"ok":false,"reason":...}` 拒绝 | render-gate 行为对齐即可 |
| 出口归账 | `engine_tunnel.rs::classify_egress`：URL 含 vercel/vgate → Vercel 桶，其余 → CF 桶 | Render 命中 CF 桶（可接受的暂时口径，见 §6 非目标） |
| crates/engine（`pony serve`） | `crates/engine/src/connect.rs`：单 gate_url + TunnelPool；`Denied` 不重试 | 多端点 parity 列为演进项（§6 非目标），用户实际路径为桌面端 |
| 隧道 token | 既有 `PPROXY_TUNNEL_TOKEN`（worker 存 SHA-256 hash 比对） | render-gate 复用同一 token/hash，客户端配置不变 |

## 3. 方案

### 3.1 总体数据流

```
agy → CONNECT → pproxy 桌面端
  establish 按序尝试 gate 列表：
  ① wss://gate.ponyjob.top/ws   → colo 正常：直连成功（低延迟）
                                 → colo=HKG/MFM：worker 秒拒 {"ok":false,"reason":"unsupported_colo"}
  ② wss://<app>.onrender.com/ws → 出口固定 Oregon（美国），必然成功
  全部失败 → 既有 502 语义不变
```

### 3.2 改造点 A：`deploy/render-gate/`（新增，Node 18+）

- `server.js`（~120 行，依赖仅 `ws`）：
  - `GET /healthz` → 200（Render 健康检查 + 免费层保活探测点）；
  - `WS /ws`：`Authorization: Bearer <token>` → SHA-256 与 `TUNNEL_TOKEN_HASH` 比对，不符 401 关闭；
  - 首帧 Text JSON `{"host","port"}`：port 必须为 443，host 经私网/localhost 黑名单（移植 worker.js `validHost`，含十进制/hex/八进制/IPv6 变体的 best-effort 拦截）；
  - `net.connect(host, 443)` 成功 → 回 `{"ok":true}` → WS Binary ↔ TCP 双向透传；失败 → `{"ok":false,"reason":...}` + close；
  - 双向任一端关闭/错误 → 两端同时释放；
  - 监听 `0.0.0.0:$PORT`（Render 注入，默认 3000）；
  - 日志：只记 host/port 与错误，**绝不记 token**。
- `package.json`：`ws@^8`，`npm start` = `node server.js`。
- `render.yaml`：Blueprint 声明（region: oregon，plan: free，healthCheckPath: /healthz），用户可一键 New Blueprint。
- `README.md`：Render 建站步骤 + env 配置 + 接入 pproxy 配置示例。
- `selftest.mjs`：本地自检（起服务 → 错误 token 拒 → 正确 token + 回显目标透传 → 非 443/私网拒）。

### 3.3 改造点 B：CF worker colo 门禁（`deploy/cf-gate-worker/worker.js` + `deploy/cf-gate-worker/gate-policy.mjs`）

**时序（审核 P0-2 修订）**：判定发生在**收到首帧 `{host,port}` 之后、`connect()` 之前**——
只有 host 命中 Google 系域名集合（`google.com/googleapis.com/gstatic.com/googleusercontent.com/g.co/goog`，
后缀匹配）**且** `request.cf.colo` 命中黑名单（默认 `HKG,MFM`，env `BLOCKED_COLOS` 覆盖）才拒绝：
`{"ok":false,"reason":"unsupported_colo:<COLO>"}` + close(1008)。非 Google host 任何 colo 都放行——
避免 YouTube/GitHub 等全量流量被倾泻到 Render 打爆 100GB/月免费额度。
`cf.colo` 缺失时 fail-open（维持现状语义）。
判定逻辑抽为无 CF 依赖的纯函数模块 `gate-policy.mjs`（node 可单测），worker.js 仅接线。
黑名单来源说明：`HKG` 为 2026-08-31 取证实测出现 400 的地区；`MFM` 同属 Google 不支持地区预防性加入；
后续按"实测出现 400 location 错误的 colo"增补（文档化机制，非拍脑袋全集）。

### 3.4 接入配置（用户侧，零代码）

桌面端隧道 URL 配置改为：
`wss://gate.ponyjob.top/ws,wss://<app>.onrender.com/ws`（既有分隔符语义）。

## 4. 任务拆解

| ID | 内容 | 验收门禁 |
|----|------|---------|
| T0 | spec + 双审核 | P0/P1 清零或裁决 |
| T1a | render-gate 实现 + selftest + package-lock.json | `node selftest.mjs` 全过（含 Ping/Pong、并发、半关、IP 变体拦截与 worker.js 对拍）；lockfile 入库 |
| T1b | worker.js colo 门禁 + `gate-policy.mjs` 纯函数 | `node deploy/cf-gate-worker/gate-policy.test.mjs` 全过（命中拒绝/fail-open/env 覆盖/Google host 集合/reason 与桌面端 denied 解析兼容） |
| T2 | 双代码审核（含安全维度）+ `cargo test --workspace` 回归 | 双审通过、无回归 |
| T3 | Render 建站 + 构造性验证 + agy 实测 | ① 国内网络 `wss://<app>.onrender.com/ws` 可达；② 构造性 fallover：临时把 `BLOCKED_COLOS` 设为当前 colo，确认 CF 秒拒且落到 Render 后 agy 成功；③ 连续 3 个长 agent 任务无 400 location 错误 |

## 5. 风险与回滚

| 风险 | 缓解 | 回滚 |
|------|------|------|
| Render 免费层 15 分钟闲置休眠 → 冷启动 ~50s，击穿桌面端 10s 拨号超时（审核 P0-1：**桌面端无 TunnelPool**，预热仅存在于 pony serve） | 三层保活：① render-gate 内置 `KEEPALIVE_URL` 自拨（每 10min 对自身公网 `/healthz` 发 HTTPS GET，Render 按入站 HTTP 判定活跃）；② 推荐叠加免费外部监控（UptimeRobot/cron-job.org，5min 间隔 ping `/healthz`，用户 2 分钟配置）；③ 桌面端 Dashboard 隧道拨测（`probe_via_gate`）在使用期间贡献活跃。冷启动偶发窗口内 fallover 当次失败列为残余风险（§7.1） | 配置移除第二 URL |
| Render 免费额度（750h/月、100GB）耗尽 | 750h 为**账号级共享额度**：README 写明"Render 账号独占本服务"前提，单实例常驻 744h/月恰好覆盖；流量经 §3.3 host 过滤后仅 Google 系走 Render，远小于 100GB | 同左 |
| render-gate 鉴权被爆破 | token SHA-256 恒定时间比对；每 IP 失败 5 次锁 60s；失败不返回任何区分信息 | - |
| render-gate SSRF（Render 容器有真实内网，worker.js 的字面量黑名单不够） | 字面量多形态（十进制/hex/八进制/IPv6）+ **DNS 解析结果双重校验**（`dns.lookup` 全结果过私网/保留段）；TOCTOU 为 best-effort 残余（§7.4） | - |
| colo 门禁误伤（cf.colo 缺失） | fail-open 放行；仅"Google host ∧ colo 黑名单"才拒 | wrangler 回滚 worker |
| onrender.com 域名国内不可达 | T3 验收含国内网络 wss 可达性实测；若命中换 Render 自定义域名 | - |
| 双端 token hash 配置不一致 → fallover 后连环 401 | render-gate 提供 `/debug`（返回 `{set: bool}`，无机密，对齐 worker.js 先例）；README 接入清单含 hash 一致性核对步骤 | - |
| 依赖漂移（无 lockfile） | T1a 验收含提交 `package-lock.json` | - |

## 6. 非目标

- 不改桌面端 `engine_tunnel.rs`（多端点 fallover 已具备）；`classify_egress` 增加 Render 桶归账列为后续演化（当前 Render 流量计入 CF 桶，语义偏差已知且无害）。
- 不做 crates/engine（`pony serve`）多端点 parity（单 gate + Denied 不重试维持现状；桌面端是 agy 实际路径）。
- 不把路径中继路由切到 Render（render-gate 只实现 `/ws`；`?url=` 路径中继接入为后续里程碑）。
- 不改 pproxy token/鉴权体系。

## 7. 已知残余风险

1. 保活失效（免费监控额度/自拨被 Render 策略变化抵消）时，冷启动 ~50s 窗口内 fallover 当次失败；三层保活使其概率低，用户重发 prompt 即可。
2. 桌面端出口归账把 Render 流量计入 CF 桶（Dashboard 数字偏差，排障时注意"CF 桶流量 ≠ CF 出口"；README 已知问题登记）。
3. CF 侧 `cf.colo` 与出口 IP 的 Google geo 判定非严格等价（出口 IP 池标记可能漂移）；colo 门禁是 best-effort 前置优化，Render 兜底才是保证。
4. DNS 校验的 TOCTOU（lookup 后 connect 前记录变更）为 best-effort；叠加端口 443 限制 + token 鉴权，实际可利用面低。
5. Render 免费实例的 WS 空闲/时长上限未核实，T3 观察长任务实测。

## 8. 审核记录

### 8.1 Spec 审核（阶段 0，2026-08-31）

| Reviewer | 结论 | 关键发现 |
|----------|------|---------|
| architect（fc1da0a0） | 不通过 | P0-1 "桌面端 TunnelPool 预热"为编造事实（桌面端无池）；P0-2 首帧前 colo 门禁致全量流量倾泻 Render；P1×5 |
| security（两路均中途失败） | 降级为主会话独立安全 pass | 见下 |

**采纳表**：

| # | 问题 | 处置 |
|---|------|------|
| P0-1 | 桌面端无 TunnelPool，冷启动击穿 fallover | **采纳**：§5 改为三层保活（KEEPALIVE_URL 自拨 + 外部监控 + Dashboard 拨测）+ §7.1 残余风险 |
| P0-2 | 首帧前门禁误伤全流量 | **采纳**：§3.3 改为首帧后 host∧colo 联合判定，Google 域名集合显式列出 |
| P1-1 | colo 黑名单无取证 | **采纳**：§3.3 写明 HKG 为取证实测、增补机制文档化 |
| P1-2 | T3 不可测试 | **采纳**：T3 改为可达性 + 构造性 fallover + 3 个长任务量化 |
| P1-3 | worker 侧无自动化测试 | **采纳**：抽 `gate-policy.mjs` 纯函数 + node 单测入 T1b |
| P1-4 | 750h 账号级共享 | **采纳**：§5 + README 写明独占前提 |
| P1-5 | token hash 双端一致性 | **采纳**：`/debug` 探针 + README 核对步骤 |
| P2-1 | 回滚顺序 | **采纳**：摘除第二 URL 为一级回滚（§5 各行） |
| P2-2 | 无 lockfile | **采纳**：package-lock.json 入 T1a 验收 |
| P2-3 | selftest 覆盖不足 | **采纳**：Ping/Pong、并发、半关、IP 变体对拍入 T1a |
| P2-4 | Blueprint/免费行为未验证 | **采纳**：T3 前置核实（render.yaml 仅为便利，手动建站路径为主） |
| P3 | 归账偏差误导、域名可达性入门禁 | **采纳**：§7.2 排障提示 + T3① |

安全 reviewer 两次子代理运行均中途失败，按 dev-team 规则降级：安全维度由主会话独立 pass 兜底并已在 §5 落地（恒定时间比对、失败锁定、SSRF 双重校验、日志零 token）；T2 代码审核阶段再派两路子代理（正确性/安全）仍均中途失败（基础设施故障，非审核结论），最终由主会话独立代码审核 pass 兜底，发现并已修复 4 项（见 §8.2）。**独立对抗审核在本任务实际未达成，如实标注；阶段 3 实测后建议补派一轮 reviewer 复审 `ec17e1a` 及安全回改 diff。**

### 8.2 代码审核（T2，主会话独立 pass，2026-08-31）

| # | 发现 | 级别 | 处置 |
|---|------|------|------|
| 1 | `clientIp` 取 XFF 最左值 → 客户端可伪造头绕过锁定/诬陷他人；应取最右（Render 反代追加的真实 IP） | P1 | **已修复** + 注释 |
| 2 | `authFails` Map 无上限 → 伪造源 IP 旋转可撑爆内存 | P1 | **已修复**（10k 上限，超限整体重置） |
| 3 | IPv6 私网封禁绕过：`::ffff:7f00:1`（hex 形式 v4-mapped，Node/OS 真实映射到 127.0.0.1）、NAT64 `64:ff9b::/96`、6to4 `2002::/16` 内嵌 IPv4 未拦截 | P0（SSRF 绕过） | **已修复**（三类内嵌地址解析后按 v4 判定）+ 5 条新 selftest 用例 |
| 4 | selftest "WS 关闭后连接计数回落" 为恒真占位断言（放水） | P1 | **已修复**（echo 连接计数真实断言半关释放） |
| 5 | 预存失败：`cmd::sync::tests::sync_roundtrip_url_safe_and_replay_protection` 在本机失败（res1 import Err）——本次变更零 Rust 改动；`pony-desktop` 进程持有 `~/.pony/state.db` 致环境冲突，判定预存/环境问题，非本次引入 | P2 | 未修复（非本 spec 范围）；复现条件已记录 |

审核后门禁：selftest 28/28 pass（含 5 条 IPv6 绕过 + 真实半关断言）、gate-policy 28/28 pass、`cargo test --workspace` 除上述预存失败外全绿、worker.js 语法校验通过。
