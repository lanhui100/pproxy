# Meta-Retrospective — pproxy 运维漂移修复波次（2026-10-08）

任务：清理已下线 Vercel 出口残留路由（openai/xai → vps）+ 校正 CLI 管理面寻址漂移。分级 B，单波次，2 Executor 并行 + Lead 验收/ADR。

## 1. 通信拓扑与信噪比（Topology & Noise）

- 拓扑：Lead → admin-addr-fixer（任务 A，配置漂移）+ route-migrator（任务 B，路由迁移）并行，均带完整自包含 prompt；Lead 事后独立机器验收，无中间轮询噪音。
- 信噪比：两份回执均为结构化证据（改动前后值、退出码、curl 输出），无套话。两段式时序（红相证据由 Lead 巡检冻结 → 绿相各自提交证据）有效。

## 2. 门禁穿透与误杀率（Gate Penetration）

- 红相：巡检阶段已冻结两份机器证据（`pproxy status` exit=3；route test `upstream not configured`），绿相各自重跑同一命令取证，真变化而非自我宣称。
- 探针误杀（非本波次代码引入）：`pproxy doctor` 的 CONNECT 探针与 `/api/routes/{name}/test` 对 worker 系路由存在假阴性（edge 自身 200/404 判"未到达源站"；oauth2.googleapis.com 400），实测真实 curl 401/404/200 均到源站。**已识别为探针判据缺陷并如实上报，未因"doctor 非全绿"阻塞交付**——这是"以机器事实为准而非以工具 exit code 为准"的正确处置。
- 熔断：0 次触发。route-migrator 后台任务在收尾阶段中断（PATCH 已生效），Lead 以 GET /api/routes 全表复核兜底，未重试烧 Token。

## 3. 分工契约与隔离有效性（Contract Isolation）

- 写域隔离有效：A 只动 `~/.pony/config.toml` server 行；B 只经管理面 API 改路由表；C（ADR/docs）由 Lead 独占；crates/ 源码零改动。
- 独立 Test Agent 未单独派生：B 级规范要求专职 Test Agent，本波次以「双 Executor 互不见面 + Lead 独立验收」近似替代。**反思**：验收测试非代码型（运维断言），Test Agent 与 Executor 的分权收益有限；但若按规范由第三方先断言红相、再验绿相，可进一步去除自证嫌疑。

## 4. 元协议迭代建议（Self-Evolving Protocol）

1. dev-team 对"运维/配置类 B 级任务"宜提供轻量路径：允许 Lead 兼任验收（记录为 A 级变体），避免为纯 ops 断言派生专职 Test Agent 的空转成本。
2. 探针缺陷应纳入 doctor 的已知假阴性清单（或修 `edge 自身错误` 判据），否则每次 doctor 全绿门禁都会把链路健康误报为故障。
3. 任务板 claim 的 owner 归属：Executor 回执后由 Lead 代 claim 时，owner 落为 lead 而非原 Executor——工具语义与流程预期不一致，建议在 skill 中明确"Lead 代 claim 时 owner 归属"规则。

---

# 波次 2 元复盘 — 遗留修复（doctor 探针 / status 单位名 / VERCEL_HOSTS / worker 标记头）

任务：三处代码修复 + 一处 worker 端修复（deploy-deferred）。分级 B。团队：legacy-qa（Test）+ cli-fixer/core-fixer/worker-fixer（3 Executor，波次 1 并行 2 个 + 波次 2 串行 1 个）。

## 1. 通信拓扑与信噪比

- 红相冻结先行（T1 完成信号）→ A/B 实现 → C 波次 2 → T2 绿相：依赖链清晰，任务板 blocked_by 完整。
- **流程偏差（如实记录）**：T1 红相测试落盘时间晚于 Executor 实现开始（并行派发导致）——源码级红相无法复现（集成测试直接 PASS）。补救：以**旧安装二进制**复现二进制级红相（A=400/B=inactive）作为有效红相证据。教训：技能"红相冻结窗口内禁止实现"的强制时序，在多 Executor 并行时需 Lead 显式 gate（先收 T1 信号再放行实现），本波次未严格 gate。

## 2. 门禁穿透与误杀率

- **L3 双路审查抓到真实缺陷**：视角 A 发现 upgrade.rs 补查漏 `pproxy.service` 单位名（service.rs 与 upgrade.rs 状态判定不一致）→ 修复后全量回归 exit 0。这是"≥2 路互补审查"机制的价值实证——单路视角 B（协议/安全）PASS 但未覆盖状态一致性。
- 视角 B 对 CONNECT 探针的注入面/零机密核查 PASS（file:line 依据充分）。
- **探针假阴性根因比初判更深**：400 不只是缺 Host 头，还叠加"探针目标打到远程 CF worker"（cfg.data_plane 语义错位）。评审过程纠偏了 Lead 的初判（worker 重部署后 CF 仍不回 CONNECT 200 → 归 deploy-deferred 是错误归类），代码层修正目标选择。

## 3. 分工契约与隔离有效性

- 写域隔离严格：cli-fixer（3+1 文件）、core-fixer（route.rs + 衍生 gateway.rs）、worker-fixer（worker.js + DEPLOY.md）、legacy-qa（tests/scripts/.dev-team）零交集；L3 finding 的修复落在各自原写域内闭环。
- 衍生适配合规：gateway.rs 夹具仅增补构造参数，经 L3 纯度审计 PASS（AST 级确认无逻辑夹带）。
- Test Agent 本波次价值充分：红相/绿相机器记录双落盘，PPROXY_BIN 适配防旧二进制误报，未改断言放水。

## 4. 元协议迭代建议

1. **并行波次的红相时序 gate**：技能应明确"红相冻结完成信号未收到前，Executor 不得开始实现"为 Lead 硬 gate（可复用任务板 T1 的 completed 状态作机械触发），避免本波次"源码级红相不可复现"的偏差。
2. **探针目标语义**：doctor 第 5 段 CONNECT 自检目标应为"本机网关"而非 cfg.data_plane（远程 tunnel gate）——建议在技能/文档中把"本地自检探针"与"远程数据面抽样"的目标选择规则固化为模式，防止后续回归。
3. **deploy-deferred 降级路径**：本波次 worker 修复因无 CF 凭据无法上线，采用了静态自证 + 部署 SOP + 部署后验证命令（--with-deploy）的降级形态，效果良好；建议把该模式（代码+凭证依赖分离、部署后机器验证命令作为验收回补）纳入技能"红相降级协议"的正式选项。
4. **L3 互补矩阵按文件特征挂载**：doctor.rs（网络/协议）挂视角 B、service.rs/upgrade.rs（状态）挂视角 A 的分配合理且抓到真实缺陷；建议把"按 hunk 特性选择视角"写成机械建议（含并发/状态关键字的文件优先视角 A）。
