# Agent Note: Cloud Code 系合规出口 host 跳过 CF 池化（修复 Antigravity 400 geo-gate）

Status: implemented

## Problem

Antigravity（daily-cloudcode-pa.googleapis.com 等 Cloud Code 系 host）的模型请求
在 2026-09-19 09:54（UTC 01:54）整窗失败，Google 返回

```
400 FAILED_PRECONDITION: User location is not supported for the API use.
```

特征事实（取证自 ponyllm 日志与 pproxy journal）：

1. ponyllm 侧配置正确：`providers.antigravity.proxy = "http://127.0.0.1:8899"`，
   执行器经 `http_client_for_target` 取到带代理的 client，CONNECT 确实发往 pproxy。
2. pproxy journal 显示失败窗口隧道**建立成功**（`tunnel established
   host=daily-cloudcode-pa.googleapis.com pooled=true/false`），即请求确实经
   隧道到达 Google——拒绝来自 Google 真身对**出口来源 IP 所在地区**的判定。
3. 出口选择才是根因：`handle_connect_raw` 的**池化路径**按 `ordered_gate_urls`
   的结果 checkout 待命会话，但 conserve 模式下 Vercel 端点
   `target_size=0`（`pool.rs maintain` 不为 Vercel 预建会话），池里只有 CF 会话；
   `checkout_ordered([vgate, gate])` 在 vgate 无会话时**降级命中 CF 会话**，
   绕过 ordered 排序的"vgate 优先"，使 Cloud Code 系请求固定落 CF 轮换出口
   （104.28.158/165.x anycast，地理归属不稳定）→ Google 400。
4. 运维面：同一时刻存在系统级 `pproxy.service`（配置正确：vgate 优先 +
   googleapis allowlist，08:59 启动、09:54 在服务）与用户级
   `pproxy-server.service`（`~/.pony/.pproxy.env`：gate 在前 + opencode/deepseek
   allowlist + `PPROXY_CONSERVE_VERCEL=1`）抢占 8899/8900，系统级被顶掉后陷入
   9.7 万+ 次崩溃循环。

## Decision

1. **代码修复**——判定统一下沉到 `crates/transport/src/route.rs` 共享 helper
   （`is_vercel_endpoint` / `compliant_egress_endpoints`），**三处数据面实现共用**：
   `crates/server/src/connect.rs`、`crates/engine/src/connect.rs`（`pproxy serve`
   生产路径）、`desktop/src-tauri/src/proxy/engine_tunnel.rs`（含 401 自愈重试
   `urls2` 路径）。两道防线：
   - **跳过池化**：对 `requires_compliant_egress(host)` 为真的 host，不走
     `checkout_ordered`（conserve 模式下 Vercel 不预建池会话，池里只有 CF 会话，
     checkout 会在 vgate 无会话时降级命中 CF）。
   - **冷建连端点过滤（fail-closed）**：合规出口 host 的端点列表过滤为**仅 Vercel
     端点**（`compliant_egress_endpoints`），vgate 拒绝/不可用时直接 502，
     **不再降级 CF**（CF 是轮换 anycast，降级回去仍被 Google 400，且把"出口地理
     不合规"伪装成上游错误）。**配置里完全没有 Vercel 端点时同样 fail-closed**
     （502 `no_compliant_egress` + warn），不回退全量——审核采纳：回退后客户端
     看到 200+Google 400，与故障现场不可区分，只会掩盖配置错误。
2. **回归测试**（server 31 / engine 16 / transport 16 全绿）：
   - server：`compliant_host_skips_cf_pool_and_establishes_vgate`（池中已有 CF
     会话时合规 host 必须冷建连 vgate）、`non_compliant_host_still_uses_cf_pool`
     （OAuth 等非合规 host 仍走 CF 池化）、`compliant_host_vgate_denied_does_not_fallback_to_cf`
     （vgate 拒绝时 502 且 CF 零接触）、`compliant_host_vgate_unreachable_does_not_fallback_to_cf`
     （vgate 网络不可达同样 502 不落 CF——故障窗口第二条 pooled=false 正是此路径）、
     `compliant_host_no_vercel_endpoint_refused`（无 Vercel 端点 fail-closed）。
   - engine：`compliant_host_uses_vercel_only_endpoints`、`compliant_host_no_vercel_endpoint_yields_empty`。
   - transport：`test_is_vercel_endpoint`、`test_compliant_egress_endpoints_filters_to_vercel_only`。
   - 突变验证（审核实测）：把两防线任一回退，对应回归测试即 FAIL——测试真实
     捕获修复前 bug。
3. **服务收敛（审核加固）**：
   - 删除用户级 unit 文件 `~/.config/systemd/user/pproxy-server.service` 与
     `pproxy.service`（`systemctl --user daemon-reload` 后无任何用户级 pproxy 单元）；
   - `.bashrc` 的 `pproxy on` wrapper 改为 `sudo systemctl start pproxy.service`
     （启动系统级），不再拉起用户级；
   - 系统级 `pproxy.service` 保持 enabled 但**未启动**（8899/8900 无监听）；
   - `~/.pony/.pproxy.env` 与 `~/.pony/config.toml` 的隧道顺序改为 vgate 在前 +
     googleapis allowlist（与系统级一致），消除"自动配置"数据源的方向性错误；
   - 系统级 `.pproxy.env` 显式声明 `PPROXY_CONSERVE_VERCEL=1` + 注释（原靠隐式
     默认 `!= "0"`，防误改）。
4. **既有 flaky 修复**：`connect_auth_host_keeps_configured_order` 原用两个
   `/ws` stub（同属 Cf 类），`ordered_gate_urls` 的同类 round-robin 依赖全局
   `GATE_URL_COUNTER` 奇偶，并行时顺序翻转导致偶发失败；第二 stub 改用
   `/api/ws` 构造真实 Vercel 端点，消除分类混淆。

## Alternatives considered

- **A. 池化时对合规 host 只 checkout Vercel 会话**（改 `checkout_ordered` 的过滤
  逻辑）：更精细，但 conserve 模式根本不为 Vercel 预建会话，池中永远没有 vgate
  会话，过滤后恒 miss，等价于跳过池化却多一层间接；且要为 pool 增加 host 感知，
  改动面更大。
- **B. 修改 `PPROXY_CONSERVE_VERCEL=0` 全局放开 Vercel 预建**：让所有 Google/AI
  流量优先 Vercel，但会刷爆 Vercel 免费额度（Hobby 10GB 出口），且泛 Google
  （oauth2 等）无需合规出口，CF 低延迟优先是正确默认；不可取。
- **C. 只在 gate worker 侧修**（egress-geo 门禁加强）：CF gate 的
  `connect()` 出口本就轮换、与探测 IP 不一致（`gate-policy.mjs` 注释自认），
  单靠 worker 侧探测无法根治；且 worker 部署在远端，改不了本地故障。
- **D. 删除用户级服务后把系统级直接拉起来再验证**：验证隧道需要服务在线，但
  本次先按用户要求保持系统级未启动，验证交由部署侧；故只做服务归属收敛，
  不做启动动作。

选 A 的"跳过池化"实现（本质上是 A 的最简形式）+ C 的 worker 侧门禁保留（已
部署，作为 CF 兜底的最后一道 fail-closed 防线）。

## Consequences

- Cloud Code 系 host 每次请求走冷建连 vgate（Vercel 真实机房出口），establish
  延迟略增（约 1.6s vs 池化 260ms），换来出口地理确定性——Google 不再按来源
  IP 拒绝。该延迟由 vgate 冷启动主导，可接受。
- 非合规 host（oauth2、generativelanguage 等）保持 CF 池化，性能路径不受影响。
- 系统级服务与用户级服务不再抢端口；用户级 unit 文件已删除，`pproxy on` 改走
  系统级；`~/.pony` 家族配置方向已对齐，消除「自动配置」数据源的复发通道。
- 合规 host 且配置缺失 Vercel 端点时 fail-closed（502 no_compliant_egress +
  warn），运维可据此快速定位配置错误，而非误判为上游故障。
- 已用多连跑验证测试稳定（server 31 / engine 16 / transport 16 全绿），修复前
  该组存在偶发失败（GATE_URL_COUNTER 奇偶 flaky 已修）。
