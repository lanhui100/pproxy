# Agent Note: 清理已下线 Vercel 出口残留路由并校正 CLI 管理面寻址漂移

Status: implemented

## Problem

2026-10-08 例行巡检发现两处互相关联的运维漂移：

1. **openai / xai 两条路由仍指向已下线的 Vercel 出口**。Vercel 部署此前已确认
   DEPLOYMENT_DISABLED（`2026-10-01-fix-cluster-probe-and-proxy-stability.md` Decision 2、
   `2026-09-29-compliant-egress-poolable-sessions.md` 均有记载），且 `config.json`
   `upstreams` 段根本未定义 vercel 上游。实测
   `POST /api/routes/{openai,xai}/test` 返回 `{"error":"upstream not configured","ok":false}`，
   路由测试或后续该路由流量丧失明确出口语义，存在静默降级风险。
2. **CLI 管理面寻址漂移**。服务端管理面由 systemd drop-in
   `/etc/systemd/system/pproxy.service.d/override.conf` 注入
   `PPPROXY_LISTEN_ADMIN=100.95.193.103:8900`（管理面走 tailnet 承载远程管理），
   而 CLI 读取的 `~/.pony/config.toml` 仍为 `server = "http://127.0.0.1:8900"`，
   导致 `pproxy status` / `pproxy doctor` 报假阴性
   `cannot reach admin api ... pproxy-server 未运行?`，
   运维告警与自检工具双双失真。

## Decision

1. **路由迁移**（经管理面 API，热生效、零重启）：
   将 `openai`、`xai` 两条路由的 `override_upstream` 置为 `vps`（`config.json`
   已配置的具名上游，`opencode` 路由同源在役且实测连通），
   `backup_upstream` 置为 `worker`（对齐 `opencode` 主备 failover 形态，
   机制见 `2026-09-28-route-backup-upstream-failover.md`）。
   **明确不清空 override**：`crates/core/src/route.rs` `pick_upstream()`（§311-327）
   在 override 为空时按 host 规则回退，而 `api.openai.com` / `api.x.ai` / `opencode.ai`
   命中 `VERCEL_HOSTS`——清空会让它们重新落回已下线的 Vercel，必须显式指定可用上游。
2. **CLI 寻址校正**：`~/.pony/config.toml` 的 `server` 改为
   `http://100.95.193.103:8900`，与 drop-in 注入值一致；其余字段（token 等）原样不动。
3. **文档同步**：更新 `docs/ops/TROUBLESHOOTING.md` 中已过时的"切 vercel"指引
   （same-commit 规则）。

## Alternatives considered

- **A（清空 openai/xai override，依赖 host 规则回退）**：被拒。host 规则对
  `api.openai.com` / `api.x.ai` 的默认值正是 Vercel（`VERCEL_HOSTS`），清空等于
  换汤不换药，错误从"显式死上游"变"隐式死上游"，更隐蔽。
- **B（重建 vercel 上游定义保留路由语义）**：被拒。Vercel 部署已
  DEPLOYMENT_DISABLED、无恢复信号，为其保留配置只会制造"看似可用实为死路"的
  路由表，且 `PPROXY_CONSERVE_VERCEL=1` 的省额度策略本就要求非豁免流量走
  vps/CF 池化。
- **C（改代码让 CLI 自动推导管理面地址）**：本变更不采纳。真实地址注入点
  （systemd drop-in）是部署态事实，config.toml 与之一致即可；改 CLI 推导逻辑
  超出最小闭包，列为后续候选。

## Consequences

- `pproxy status` / `pproxy doctor` 恢复真实告警能力（管理面可达）；
- 7 条路由 `effective_upstream` 不再出现 vercel；openai/xai 真实链路
  `curl --proxy http://127.0.0.1:8899 https://api.openai.com/v1/models` 等
  返回 401（已到达源站），出口 IP 稳定 192.210.231.8；
- **残留风险（未在本变更处理）**：
  `pick_upstream()` 的 `VERCEL_HOSTS` 默认回退仍指向已下线 Vercel——
  未来新建 `api.openai.com` / `api.x.ai` / `opencode.ai` 类 host 路由时
  必须显式设 override（或等 Vercel 真正恢复后清理该回退），
  建议列入后续简化项：从 `VERCEL_HOSTS` 判定中摘除已死出口或整体下放为配置化。