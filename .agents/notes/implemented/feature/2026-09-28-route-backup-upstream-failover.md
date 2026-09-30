# Agent Note: route-backup-upstream-failover

Status: implemented

## Problem

路由的出口（override_upstream）是单值绑定：主出口故障时请求直接 502，
无自动兜底。2026-09-27 实况：opencode 路由绑 Vercel 出口，Vercel 部署被禁用
（HTTP 402 DEPLOYMENT_DISABLED）后整个 opencode-zen provider 不可用，只能靠
人工 PATCH 改出口恢复。需求：主出口指数退避重试 N 次不通时，同请求自动转
备用出口（如 VPS → CF Worker），且不影响"4xx 业务响应不重试"的既有语义。

## Decision

给路由增加**备用出口**（backup_upstream），数据面转发实现请求级主备 failover：

1. **数据模型**：`routes` 表新增 `backup_upstream TEXT` 列（幂等 ALTER 迁移，
   老库自动补列）；`RouteRow`/`NewRoute` 增字段；`route.backup_upstream(name)`
   读取（值域与 override 同：worker|vercel|已配置上游名，create/update 过
   `valid_override` 校验）。
2. **failover 执行**（gateway forward_handler）：主出口 `execute_with_attempts(fwd, 3)`
   ——沿用既有指数退避（连接错误恒重试、幂等 5xx 重试、无幂等键 POST 不因
   5xx 重试防幽灵扣费）；失败（连接层 Err 或 5xx 响应）且配了备用出口 →
   同请求转备出口（默认 5 次预算）。**4xx 不 failover**（402/403/429 为上游
   正常业务响应，重试无益）。
3. **面**：管理面 POST/PATCH `/api/routes` 支持 `backup_upstream`（三态
   double_option）；CLI `route add --backup`、`route list` 展示；GET 列表含
   `backup_upstream` 字段。
4. **测试**：新增 2 个网关单测——主出口连接失败自动转备（stub 回显 target
   200）；上游 4xx 原样透传不 failover。全量测试通过（111+105+16+43）。
5. **部署**：重编译 release，`systemctl restart pproxy`（8899 短暂断流）。
   生产 opencode 路由 = 主 vps / 备 worker；CF 出口备用通道（opencode-cf 路由）
   保留。

## Alternatives considered

1. **ponyllm 层双 provider 自动 fallback**：免费模型 economy 打分恒 0
   （`EconomyScorer::score_candidate` 免费一律 0.0），候选排序依赖 HashMap
   迭代序，无法保证主出口恒优先；双 provider 配置维护成本高。否决。
2. **外部探活脚本自动 PATCH**：分钟级全量切换、切换间隙请求失败、非请求级
   粒度。否决（保留为无代码改动时的兜底选项）。
3. **主备共享同一重试预算**（主 3 次后不重试直接转备）：与"4xx 不转"的
   业务语义冲突面最小，但连接层失败与 5xx 的区分仍须显式——最终实现即
   Err 或 5xx 触发，4xx 排除。

## Consequences

- opencode 主 vps 故障时请求级自动转 CF（实测冒烟：主不可达 → 3 次退避 →
  转 CF 200，总耗时 ~1.6s）；正常时零额外开销（主出口成功不触碰备份）。
- 非幂等 POST 5xx 时**不**转备（沿用防幽灵扣费语义）；连接层失败（请求未
  确认送达）恒可转。
- 运维提示：改出口仍以管理面 PATCH 为准；`config.json` 的 `route_upstreams`
  仅首次迁移生效（既有事实，未变）。
