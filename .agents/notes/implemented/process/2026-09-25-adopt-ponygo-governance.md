# Agent Note: 采纳 ponygo 软件工程质量管理体系与决策规范统一

Status: implemented

## Problem

`pproxy` 历经快速演进，已具备正向 CONNECT 代理、反向 API 网关、多出口（CF/Vercel/RackNerd）和桌面端管理能力。但在工程治理与演进过程中存在以下管理痛点：
1. **决策体系双轨与断代**：早前在 `docs/architecture/decisions/` 中存在 8 篇架构决策（001~008），后在 `.agents/notes/` 中出现部分 note，缺乏统一的机械校验、时序约束与索引机制；
2. **缺乏机械化门禁与分层约束**：代码库仅有远端 CI（GitHub Actions），缺乏本地级验证门禁（pre-commit），提交时无法秒级拦截格式、语法与 ADR 规范缺口；
3. **文档标准与包职责不明确**：`crates/` 各模块缺少明确的职责边界文档（`README.md`），Agent 与人类协作者介入时无法快速定位各 crate 的契约。

## Decision

正式在 `pproxy` 项目中引入并落定 `ponygo` 软件工程治理框架，治理级别设为 L2（成熟度目标 L2，当前自洽于 L1 并建立 L2 门禁支持）：

1. **统一真相源与宪法**：
   - 在 `.meta/constitution/constitution.md` 固化项目元信息与四大常载命约（Standing Orders），通过 `ponygo sync` 投影至根目录 `AGENTS.md` 和 `CLAUDE.md`；
   - 规定后续所有非平凡变更必须遵循先 ADR 后代码原则，放入 `.agents/notes/`。

2. **决策体系收敛与旧决策建档**：
   - 确认 `.agents/notes/` 为活动决策的唯一有效真相源；
   - 保留 `docs/architecture/decisions/` 作为历史架构 ADR 归档参考，在架构地图及 `docs/AGENTS.md` 中指明双轨收敛策略，避免历史漂移。

3. **文档家标准与模块文档补齐**：
   - 以 `docs/AGENTS.md` 作为文档标准的自动加载家载体；
   - 为全部 6 个核心 crate（`cli`, `core`, `transport`, `engine`, `server`, `gate-server`）补齐模块契约 `README.md`。

4. **门禁分层与自动化验证**：
   - 启用 `.meta/gates/` 存放关键门禁校验，配置本地秒级验证，与 GitHub Actions 远端全量 CI 构成双层防御。

## Alternatives considered

- **维持现状（纯依赖 `docs/architecture/decisions/` 散文式记录）**：缺少程序化校验（`verify-note.sh`）与 Agent 认知闭环，容易因人而异产生格式漂移与时序倒置。
- **重型企业级管理规范（如完全引入外部复杂的 Issue 模板与流程体系）**：对于小团队或单人开发负荷过重，违背 P7 时间经济性与轻量自洽原则。选用 ponygo 能够在单文件、零依赖下实现纯机械检查与语义治理的平衡。

## Consequences

- 后续任何破坏性、架构性或行为变更均有程序化 ADR（`verify-note.sh` 守护）支撑。
- 代码库具备清晰的模块文档与分层门禁体系，降低协同与自动化 Agent 的理解门槛。
