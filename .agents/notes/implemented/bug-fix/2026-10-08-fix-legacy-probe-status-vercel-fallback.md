# Agent Note: 修复遗留探针/状态自检假阴性并移除已下线 Vercel 默认回退

Status: implemented

## Problem

2026-10-08 巡检后遗留三处缺陷（前条 ADR
`2026-10-08-fix-vercel-dead-upstream-and-admin-addr-drift.md` 已记录残留风险，本条闭环实现）：

1. **`pproxy doctor` CONNECT 隧道探针假阴性**（`crates/cli/src/cmd/doctor.rs`）：探针请求
   `CONNECT {host} HTTP/1.1\r\n\r\n` **缺 Host 头**（HTTP/1.1 规范要求，网关回 400）；
   且第 5 段探针目标取 `cfg.data_plane`（= 远程 tunnel gate `https://edge.ponygo.fun`，
   CF Worker 对裸 CONNECT 一律回 400）——本地自检打到了远程只读端点，永久假阴性。
2. **`pproxy status` systemd 单位名/作用域漂移**（`crates/cli/src/cmd/service.rs`、
   `upgrade.rs`）：非 root 只查 `--user` 作用域，系统级 `pproxy.service`
   （`/etc/systemd/system/pproxy.service`，systemd drop-in 注入管理面地址）
   永远显示 `inactive`；`upgrade.rs` 补查也只探 `pproxy-server` 单名（规范单位为
   `pproxy.service`，install.sh 另装 `pproxy-server.service`，两命名并存），
   service.rs 与 upgrade.rs 对同一状态判定不一致。
3. **已下线 Vercel 的默认回退未拔除**（`crates/core/src/route.rs`）：`VERCEL_HOSTS`
   （`api.openai.com`/`opencode.ai`）在无 override 时默认回退 `Upstream::Vercel`，
   Vercel 部署已 DEPLOYMENT_DISABLED，新路由无 override 即落入死上游。
4. **CF Worker 透传响应缺 `x-proxy-edge` 标记头**（`deploy/cf-worker/worker.js`）：
   `test_route` 以该标记头判定"响应透传到达源站"（vps/vercel 端都附加），worker 端不加 →
   4 条 worker 上游路由（anthropic/bai/github/opencode-cf）连通性测试永久假阴性
   "edge 自身错误"。另实测线上 `edge.ponygo.fun` 对 config.json 的正确
   `worker_secret` 仍返回未授权伪装 404 → 线上 `PROXY_SECRET` 与仓库 config.json 漂移。

## Decision

1. **探针协议与目标双修**（doctor.rs + main.rs）：
   补 `Host: {host}` 头（抽出纯函数 `connect_request()`，外层白名单校验保留防 CRLF 注入）；
   第 5 段 CONNECT 探针目标改为**本地自检语义**：显式 `--data-plane` > `derive_from_base(admin)`
   （admin host:8899 即本机网关），**忽略 `cfg.data_plane`**（远程 tunnel gate 不适用裸 CONNECT）；
   第 4 段 HTTP 数据面抽样仍用 `cfg.data_plane`（语义不变）。
2. **systemd 检测对齐**（service.rs + upgrade.rs）：对 `["pproxy-server", "pproxy"]`
   每单位先 user 后 system 作用域探测，任一 `active` 即采用，mode 标签与实际命中作用域一致；
   upgrade.rs 与 service.rs 同口径。
3. **移除 VERCEL_HOSTS 死回退**（route.rs）：删除常量与特判，无 override 一律 `Upstream::Worker`；
   显式 `Some("vercel")` override 语义保留（Vercel 恢复可显式启用）；同名内联断言作衍生更新。
4. **worker 端补标记头 + secret 对齐 SOP**（worker.js + DEPLOY.md）：透传路径
   `respHeaders.set("x-proxy-edge", "worker")`（早退错误分支天然不带头）；
   DEPLOY.md 记录线上 `PROXY_SECRET` 漂移实证与对齐步骤
   （`wrangler secret put PROXY_SECRET`，值以仓库 config.json 的 `worker_secret` 为唯一权威源；
   wrangler 4 用 `versions secret put` + 版本化部署，禁裸 `wrangler deploy`）。
   **本环境无 CF 凭据，重部署不可行** → 线上效果标记 `[RED DEGRADED: DEPLOY-DEFERRED]`，
   部署后验证命令 `bash scripts/accept-legacy-items.sh --with-deploy`。

## Alternatives considered

- **A（探针 400 归因 Host 头缺失，只补头不挪目标）**：被拒。CF Worker 对带 Host 头的裸
  CONNECT 依然回 400（Cloudflare 边缘层行为，与 worker.js 无关），补头后 plain doctor
  仍假阴性——必须同时修正探针目标语义。
- **B（改 `~/.pony/config.toml` 的 `data_plane` 为本机网关）**：被拒。`data_plane` 同时被
  客户端隧道配置生成消费（`pproxy tunnel` 持久化该字段），改值有跨功能副作用；代码层
  修正探针目标是最小闭包。且远程 worker 重部署后也不会回 CONNECT 200，归入
  deploy-deferred 是错误归类。
- **C（保留 VERCEL_HOSTS 回退等 Vercel 恢复）**：被拒。无恢复信号（DEPLOYMENT_DISABLED），
  保留死回退使"新路由无 override"静默落入死上游；显式 override 已提供恢复通道。
- **D（禁止测试直接改断言，改为重写 fixture）**：不采纳。断言值 `Vercel→Worker` 是
  pick_upstream 新语义的直接投影，属衍生更新（允许）；gateway.rs 夹具仅增补
  `Some("vercel")` 构造参数保持测试原意图（剥离路径而非默认可选性），经 L3 纯度审计 PASS。

## Consequences

- `pproxy doctor` 第 5 段真实可达：`[pass] CONNECT tunnel probe: oauth2.googleapis.com:443 → 200`
  （本机网关应答）；`pproxy status` 正确显示 `systemd (pproxy [system]): active`；
- 新建无 override 路由默认走 Worker（CF 池化），不再落入已下线 Vercel；
- worker 路由 test 的线上恢复依赖部署（deploy-deferred）：标记头 + `PROXY_SECRET` 对齐
  版本化上线后，`POST /api/routes/{anthropic,...}/test` 应绿；未上线前 doctor 仍报 4 条
  worker 路由 fail（**非链路故障，是探针端未部署**）；
- 残留风险：`route.rs` 文档注释与 `docs/product/specs/m7-frontend-ux/SPEC.md` 中
  "VERCEL_HOSTS 自动规则"表述已同步为"默认 Worker + 显式 override"；`Upstream::Vercel`
  变体保留供显式启用，不构成死代码。