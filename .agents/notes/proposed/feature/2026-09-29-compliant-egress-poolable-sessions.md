# Agent Note: compliant-egress-poolable-sessions

Status: proposed

## Problem

antigravity 链路（`daily-cloudcode-pa.googleapis.com` 等 Cloud Code 系 host）每请求
冷建连，观测 24h 内 335 次 `tunnel established` 全部 `pooled=false`，establish
p50≈1.3s / 结构上加到链路延迟（轨道A 实测该 host p50=2.15s，24h establish max=25.9s），
是 ponyllm TTFT 的重要固定开销。

代码根因（已定位）：生产数据面 `crates/server/src/connect.rs` 沿用 2026-09-19 的
`attempt == 0 && !compliant_egress` 条件——合规出口 host 一律跳过待命池。而
`crates/engine/src/connect.rs`（HA Forwarder）在后续提交
`da82bd1 perf(ha): enable tunnel pool on compliant egress` 已引入
`has_poolable_endpoint` 优化（合规端点列表含 NativeVps 时允许池化）——**server 数据面
未同步该优化，两处代码分叉**。生产 8899（systemd pproxy-server）即 server 数据面，
故 antigravity 始终冷建连。

## Proposal

在 `crates/server/src/connect.rs` 对齐 engine 的既有逻辑：

1. 合规过滤后（`compliant_egress_endpoints` 已保留 NativeVps + Vercel，过滤掉 CF）
   计算 `has_poolable_endpoint = ordered_refs.iter().any(|ep| !is_vercel_endpoint(ep))`；
2. 池化条件改为 `if attempt == 0 && (!compliant_egress || has_poolable_endpoint)`，
   即合规 host 只要端点列表含可池化物理出口（rn/独立VPS）就允许 `checkout_ordered`；
   仅 vgate 时保持现状（冷建连，安全侧）。

安全性论证（已实证）：
- `checkout()` 只遍历传入的 ordered 端点（`transport/pool.rs`），不会跨列表回退 CF——
  合规过滤已排除 CF，池化命中只可能是 rn（NativeVps）或 vgate；
- 今天 cloudcode-pa 的冷建连本就先打 rn（实测窗口内唯一目标对端 192.210.231.8，
  vgate 0 连接——Vercel 部署 DEPLOYMENT_DISABLED 已确认），Google 已接受 rn 出口
  （链路1 24h 成功率 99.67%）→ 池化只是复用同一出口，不改变合规属性；
- 池中 rn 会话已满额预热（实测 184 条活跃 rn 连接），复用不新增 VPS 负载。

预期收益：antigravity 链路延迟由驱动建连（~1.3s）降为池化绑定（~0.15s 起），
链路 p50 预计 2.2s → 1.0s 量级，并消除 25.9s 建连尾部风险；
对 ponyllm TTFT 的链路部分贡献同样收敛。

## Alternatives considered

1. **维持现状（接受 +1.4s 结构性延迟）**：零改动零风险，但 ponyllm TTFT 长尾
   （p95=48.4s）中链路固定成分持续存在，且 25.9s 建连极值暴露面不消除。否决。
2. **把 cloudcode-pa 加入 vgate 池化（Vercel target_size>0）**：直接违背
   `PPROXY_CONSERVE_VERCEL=1` 省额度策略（Vercel Fluid 按 compute 计费），且 vgate
   当前部署禁用、不可用。否决。
3. **desktop/engine 已有、server 未同步 → 仅补 server 对齐**（本提案）：engine 已在
   桌面端/Forwarder 实践该路径，属于"补齐生产数据面与已优化实现的一致性"，改动
   一行条件 + 单测。采纳。
4. **反向网关模式（opencode 走 `/{token}/{route}`）用于 antigravity**：改动模型面
   广（ponyllm 配置 + 网关路由），且反向网关无预建隧道池语义，收益不如正向池化
   直接。不纳入本次范围。

## Acceptance criteria

- [ ] `crates/server/src/connect.rs` 池化条件与 engine 语义一致（含
      `has_poolable_endpoint` 判断）；
- [ ] 单测：合规 host + 端点含 rn → 池化 checkout 被使用；合规 host + 仅 vgate →
      不 checkout（维持冷建连）；非合规 host 行为不变；现有测试全绿
      （`cargo test --workspace` / `scripts/check-egress-parity.sh`）；
- [ ] 部署后实证：journal 中 cloudcode-pa 出现 `pooled=true`，建立时间 p50 显著下降
      （对比 1.3s 冷建连基线），antigravity 端到端无 400/失败率上升；
- [ ] `scripts/self-check-pproxy.sh` §2 的"Vercel Fluid 调用计数"口径修正或退役
      （其假设 pooled=false=vgate 在 rn 主出口下已失真，池化后计数趋零将失去告警意义）。

## Risks

- **rn 出口被 Google 重新判定地区**：低。rn 是已实证合规出口且为当前唯一实际出口，
  池化不改变出口 IP 与 geo 属性；若未来 rn 失效，冷建连回退路径不变（checkout miss
  → 冷建连 → 502 语义一致），不会比现状更差。
- **池会话串用目标 host**：无风险——WS 隧道无状态，bind 每次声明目标，client TLS
  端到端加密，池化是现有通用路径。
- **自检口径误读运维动作**：中等（流程类）——需同步更新 self-check 计数口径，否则
  运维会把"计数趋零"误判为"Vercel Fluid 用量消失"。
- **goroutine/资源**：无新增（复用已预热的 rn 池）。