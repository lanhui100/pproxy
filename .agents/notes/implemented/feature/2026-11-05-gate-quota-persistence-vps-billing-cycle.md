# Agent Note: 网关配额持久化、RN VPS 账单月度周期重置与 Admin 整机配额映射

Status: implemented

## Problem

1. 桌面端 Admin 导入 `usr_live_` 管理员凭证后，界面直接消费该 Token 载荷内的 `quota_bytes`
   （例如 50GB），把 VPS 整体 500GB 大盘挤掉，并误报“额度已耗尽”；“到期时间”也显示为令牌 JWT
   过期日，与 VPS 月付账单周期无关。
2. `gate-server` 的 `user_used_bytes` 为纯内存 `DashMap`，进程重启即归零，用量无法跨重启累积。
3. 缺少账单周期（Billing Cycle）概念：RackNerd 等 VPS 按账单日循环计费（默认每月 1 日），
   系统既不会跨周期归档，也不会自动归零。
4. 计量侧缺陷：`connect_proxy::Meter::charge` 在 `quota == None`（管理员豁免）时直接 return，
   管理员自身经 CONNECT 面产生的流量完全不记账，整机水位永久偏低。

## Decision

1. **用量账本 `~/.pony/gate_usage.json`**（`crates/gate-server/src/usage.rs::UsageManager`）：
   - 形状 `{cycle_key, billing_day, users: {sub: used_bytes}, updated_at}`，`users` 用 `BTreeMap`
     保证序列化稳定；写入走「临时文件 + `rename`」原子替换，与 `revoked_tokens` 同构。
   - 启动 `load_or_init` 恢复水位；文件缺失/解析失败 → 空账本 + warn，不阻塞启动。
   - 后台任务每 10 s `check_and_rollover` + `persist`；以账本指纹做变更检测，无变化不写盘。
   - 收到 `SIGTERM`/`Ctrl-C` 时走 `axum` 优雅停机，退出前 `UsageManager::flush_now()`
     强制落盘（跳过 dirty-check），把最后一次周期刷盘之后 ≤10 s 的计量补写回去。
2. **账单周期计算**：`compute_billing_cycle(now, billing_day)` 为纯函数（Howard Hinnant 民用历算法，
   无 `chrono` 依赖），返回 `(cycle_key="YYYY-MM", next_reset_ts, days_left)`；
   `GATE_BILLING_DAY` 默认 1，clamp 到 1..=28，结算点固定为 UTC 00:00。
   跨周期时上一周期归档为 `gate_usage_<cycle_key>.json`；当前水位按归档快照**扣减**而不是粗暴
   `clear()`，归档瞬间之后新流入的字节留在新周期，堵住跨周期窗口丢账。
   扣减后归零的用户条目立刻从内存账本移除（`remove_if` 二次确认仍为 0），否则每跨一次周期
   就沉淀一批 0 值用户，`users` 表随月数单调膨胀。
3. **`/api/user/profile` 身份感知**（`crates/gate-server/src/lib.rs`）：
   - `role=admin`（`is_admin_claims`）：`quota_bytes = 536870912000`（500 GiB 整机基线）、
     `used_bytes = 本周期全租户聚合水位`、`expire_at = 下次账单重置点`、`days_left = 距重置天数`、
     `unlimited = true`、`status = active`；
   - 普通租户：沿用令牌自包含 `quota_bytes` / `exp`，`unlimited = false`。
4. **计量口径**：`connect_proxy::Meter::charge` 只要 `uid` 存在即记账，`quota == None` 仅跳过熔断判定；
   `/ws` 双向路径本就按 `uid` 记账、按 `quota` 熔断，保持一致。
5. **桌面端 `DashboardView.vue`**：`userQuotaDisplay` 消费 `unlimited` → 上限取 500 GiB，
   标签为「VPS 月度用量（管理员 · 整机 500GB）」，到期行改为「账单重置：X月X日（剩 N 天）」，
   管理员不再触发 `isQuotaExceeded`/`isExpired`；profile 30 s 轮询刷新水位与倒计时。

## Alternatives considered

- **只靠 10 s 周期刷盘，不做退出落盘**：网关重启/发布滚动时最后 ≤10 s 的流量静默丢失，
  洽好复现用户报的“重启归零”观感。改为优雅停机 + `flush_now()`。否决。
- **跨周期直接 `map.clear()`**：归档快照到 `clear()` 之间（同进程内毫秒级窗口）的增量被吞掉，
  月月漏一点。改为按快照扣减（`saturating_sub`），语义等价归零但不丢账。否决 `clear()`。
- **只在前端把 admin 写死为 500GB**：网关 profile 仍返回 50GB，移动端/Clash 订阅口径不一致；
  且解决不了重启归零与账单周期缺失。否决。
- **引入 Redis / SQLite 承载计量**：gate 部署在 1C1G 微型 VPS，外部依赖的运维与内存成本高于收益；
  单机单进程场景下文件原子落盘 + 内存原子计数已足够。否决（若将来多节点共享计量，需改为分布式账本）。
- **硬编码自然月（每月 1 日）**：RackNerd 常按开通日（如 15 日）结算，故保留 `GATE_BILLING_DAY`
  配置项，默认 1 保持开箱即用。
- **按 VPS 本机 `vnstat`/网卡计数取代网关计量**：口径包含非代理流量（系统更新、SSH 等），
  与“租户可归属配额”冲突，且桌面端无法读取远端网卡。否决。
- **桌面端本地 `traffic.json` 作为管理员水位**：只能看到本机产生的流量，换设备登录即失真。
  否决，服务端账本是唯一真相源。

## Consequences

- 网关重启不再清零；跨账单日自动归档并归零。
- 管理员在桌面端看到整机 500 GiB 水位与账单重置倒计时，普通租户语义不变。
- 已知限制（写在 `crates/gate-server/README.md` 与 `docs/ops/API.md`）：水位只统计**经网关计量**
  的字节，VPS 上绕过网关的出网流量不计入；结算点为 UTC 00:00，与厂商账单时区可能存在数小时偏差。

## 机械验收

- `cargo test -p pproxy-gate-server`：`usage::tests::{compute_billing_cycle_*,
  usage_manager_persistence_and_restore, rollover_subtraction_keeps_new_bytes,
  rollover_prunes_zeroed_users}`、`tests::tests::test_admin_profile_maps_vps_quota_and_billing_cycle`、
  `connect_proxy::tests::metering_charges_unlimited_principal` 全绿（17 单元 + 8 集成）。
- `bash crates/gate-server/scripts/smoke-usage-ledger.sh`：真实二进制跑「SIGTERM 退出落盘 /
  重启恢复 / 跨周期归档归零」，非零退出即失败（2026-11-06 实测 OK）。
- `cd desktop && npm run check`（vue-tsc）、`npm test`（116 用例）、`npm run lint` 通过。
