# 工程治理元框架审核评估报告

> 审核日期: 2026-08-30 | 审核范围: pproxy 全库治理工件（specs/ADR/backlog/CI/文档体系），对照 deepseek-harness（DSH）治理基线
> 结论: **规划与审核层优秀，执行追踪与文档保鲜层存在系统性缺口** —— 治理仍停留在"约定"层，缺少 DSH 式的"机器强制"层

## 1. 总体评价

pproxy 已在"一次开发活动"的粒度上建立了相当完整的治理闭环：**spec 驱动 → 并行波次编排 → 双路对抗审核 → 裁决表 → 修订执行 → 交付报告**，这一链路的成熟度不亚于甚至局部超过 DSH（DSH 无同等结构的对抗审核裁决台账）。但作为"元框架"（即跨里程碑、跨时间可持续运转的治理体系），对照 DSH 基线存在三类系统性缺口：

1. **无机器强制的文档保鲜机制** —— README/CURRENT/ADR 已发生可观测的漂移（§3 发现 F1-F4），而 DSH 用 verify-md-links、doc-typecheck、doc 词数预算等 CI 门禁把漂移挡在合入前；
2. **无"站立规则"单一真源** —— 治理知识散落在各里程碑 spec 的历史裁决中，没有 AGENTS.md 式的常驻公约，新会话/新 agent 无法低成本继承；
3. **执行追踪双轨** —— 未入库的 IMPLEMENTATION_PLAN.md、编号冲突的 backlog、缺失的 ADR-008，说明"执行真源"纪律在里程碑间隙失效。

## 2. DSH 治理基线（对照参照系）

证据来源：D:\Documents\pony-agent\deepseek-harness 仓库实证。

| # | 基线 | DSH 实现 | 治理价值 |
|---|------|----------|----------|
| D1 | 站立规则单一真源 | 根 `AGENTS.md`：每规则 1-3 行 + 链接到归属文档；子树 AGENTS.md 只承载子树特有序 | agent/人每次会话零成本继承全部约束 |
| D2 | 文档分层"一事实一家" | `docs/AGENTS.md` 定义 tier 分类法：architecture/subsystems/Agent Notes/postmortem/cookbook/README 各司其职 | 消除重复表述导致的漂移源 |
| D3 | 机器可检的文档门禁 | `verify-md-links`（死链拒入）、`doc-typecheck`（文档内 ts 代码块必须编译）、`verify-doc-budgets`（词数上限）、`pnpm run doc-sync` 聚合 | 文档漂移在 CI 被拦截而非靠自觉 |
| D4 | 决策记录强制随同变更 | "Every non-trivial change includes at least one Agent Note in the same PR"；implemented/ 笔记用现在时描述既成事实 | 决策不随会话丢失 |
| D5 | 本地快速门禁 + CI 全量矩阵 | lefthook pre-commit（lint/i18n 配对/vendor 守卫）+ pre-push typecheck；CI 拥有全平台矩阵 | 快速反馈与穷尽验证分层 |
| D6 | 覆盖率硬门禁 | `test:coverage`：packages src 逐文件 100% | 测试债不可累积 |
| D7 | 卫生门禁聚合 | `pnpm run hygiene`（knip 死导出/publint/workspace 约束/NodeNext 消费方检查）、jscpd 重复检测 | 一致性腐烂自动化检出 |
| D8 | 变更即归档 | "Document current state, not change history"；状态标注禁止入文档（"status rots"） | 文档默认可信 |
| D9 | 发布治理 | tag 触发 release workflow + changesets 式版本纪律 + THIRD_PARTY_NOTICES 自动生成 | 发布可复现、合规自动 |
| D10 | 事后复盘独立分层 | `docs/postmortem/` 唯一允许"战争故事"叙事的层 | 经验沉淀不污染参考文档 |

## 3. 发现清单

### F1 [Critical] README 与现状严重漂移
- `README.md:15` 声称"非 CONNECT 隧道（CONNECT 一律 403）"，但 M6 已立项桌面 CONNECT 隧道（`docs/product/specs/pproxy-connect-tunnel/`、`crates/engine/`），产品形态已升维；
- `README.md:70-80` 代码结构只列 `crates/core`、`crates/server`，实际 workspace 含 `crates/{core,engine,server,cli}`（根 `Cargo.toml:2`）及完整 Tauri 桌面端 `desktop/`；
- `README.md:43` "初始 7 路由"静态表与 SQLite 热管理现实脱节；文档索引缺 specs/、backlog、dev-team/reviews。
- 影响：README 是治理体系的门面与入口，其失真意味着"文档默认可信"这一元框架前提已不成立。

### F2 [High] CURRENT.md 自相矛盾
- `docs/architecture/CURRENT.md:40` "CONNECT 请求：池空时直连目标（兜底，当前池已停用）" 与 M1 P0-1 裁决（CONNECT 一律 403，`docs/product/specs/m1/REVIEW-LOG.md:19`）直接冲突；
- 同文件 `:16` 管理面仍写 "/stats /refresh"，而该两端点在 M1 已移除（`docs/product/specs/m1/README.md:110`）。
- 该文件头标注 "更新: 2026-08-25"，说明最近更新时未做全文一致性校验。

### F3 [High] ADR 链路断裂
- ADR-001（`docs/architecture/decisions/001-gateway-mode-over-connect.md`）未按 m6 spec 声明的"修订 ADR-001 适用边界"（`docs/product/specs/m6/README.md:5`）更新状态——无 superseded/partially-amended 标注；
- ADR-008 被 13 处引用（`docs/product/TECH_DESIGN.md:98`、m6 spec §0.1/§7/§12、`backlog/backlog.md:47,51`），文件不存在。虽属 B001 待回填的显式状态，但"引用先行、文件缺失"使追溯链断裂，且回填依赖"凭据在审计会话手中"——单点知识孤岛。

### F4 [High] backlog 编号完整性损坏
- `backlog/backlog.md:11` B003 与 `:61` B003 是不同事项；`:22` B002 与 `:58` B002 同名不同义（前者"Vercel 出口排查"，后者"M6 实现收尾"）。ID 冲突使跨文档引用（spec ↔ backlog）失去确定性。

### F5 [Medium] 执行追踪双轨
- `IMPLEMENTATION_PLAN.md` 未入库（git 未跟踪），阶段状态停留在"进行中"，而工作区 `crates/engine/src/connect.rs` 等 5 个文件正有未提交改动——执行真源实际在工作区，plan 文件是残影。违反自身"执行真源=spec"的约定（backlog.md:56）。

### F6 [Medium] 本地门禁脚本未入 CI
- `scripts/m1_test.sh`–`m4_test.sh` 是里程碑级集成门禁，但 `.github/workflows/ci.yml` 只跑 `cargo test --workspace`（其文件头注释自承"补位"性质）；server workspace 无 clippy `-D warnings` / fmt 门禁（desktop-gate.yml 对 desktop 有，标准不对称）。m1 REVIEW-LOG 裁决的"离线可过门禁子集"（C-风险项）具备入 CI 条件但未接线。

### F7 [Medium] 无站立规则沉淀（元框架核心缺失）
- m1 spec README §3 的共享约定（错误处理/时间/并发/日志纪律）是高质量的工程公约，但只活在 m1 历史 spec 中；后续里程碑（engine/desktop）是否继承无声明。无 AGENTS.md/CONTRIBUTING，新开发会话无法自动加载这些纪律。

### F8 [Low] 文档无预算与分层约束
- `docs/ops/DESKTOP-TROUBLESHOOTING.md` 42KB 单文件日志式累加（git log 显示 rounds 3-6 持续追加），按 DSH D2/D8 基线应拆为 postmortem 条目 + 索引。

### F9 [Low] 过程规范无强制
- git log 显示事实上的 conventional commits（feat/fix/refactor/chore 前缀），但无 commit-msg 校验、无 PR/issue 模板（`.github/` 仅 workflows）。

## 4. "看着像债但其实合理"

- **docs 中未来年份日期（2026-02/2026-08）**：全库一致的时间线约定，非笔误。
- **ADR-008 引用悬空**：是显式登记的待回填状态（B001），有决策触发条件与回滚路径，治理动作正确；问题仅在无占位文件（见 F3）。
- **desktop-gate 仅 paths: desktop/\*\* 触发**：spec M5 §7 的显式设计，规避 paths×tags 组合歧义，注释自证（desktop-gate.yml:1-2）。
- **.pony/state.db、.workbuddy 在工作区出现**：`.gitignore` 已覆盖（`*.db`、`.workbuddy/`），git ls-files 确认未入库。

## 5. Top 5 优先级（按影响/工时比）

| 序 | 行动 | 工时 | 解决 |
|----|------|------|------|
| 1 | 建根 `AGENTS.md`：抽取 m1 §3 公约 + 历次裁决中仍然有效的纪律 + 文档索引 | 2h | F7，元框架从"历史"变"常驻" |
| 2 | 修 README + CURRENT.md 漂移；ADR-001 加 "M6 部分修订" 状态头；建 ADR-008 占位（状态: 待回填 B001） | 1.5h | F1/F2/F3 |
| 3 | backlog 重编号（已完成区 B002/B003 → B005/B006）+ 文件头加 ID 分配规则 | 0.5h | F4 |
| 4 | CI 加 doc-link 检查（markdown-link-check 或自写 20 行脚本）+ server 侧 clippy/fmt 门禁；m1_test 离线子集接 CI | 2h | F1-F3 的复发防护、F6 |
| 5 | IMPLEMENTATION_PLAN.md 要么入 spec 目录归档要么删除；立规"执行真源唯一" | 0.25h | F5 |

## 6. 与 DSH 的差距本质

DSH 治理的底层假设是"**任何约定都会腐烂，除非有门禁**"——所以它的每一份治理工件都配对了一个自动化检查（链接有 lint、预算有 verify、决策有同 PR 强制、覆盖率有硬门）。pproxy 目前的元框架是"**重前端（规划/审核）、轻后端（保鲜/强制）**"：对抗审核裁决台账是亮点资产，但裁决产出的纪律没有进入常驻规则与 CI，导致同一问题（文档漂移、CONNECT 语义矛盾）在里程碑间隙回潮。补齐顺序建议：先沉淀 AGENTS.md（D1），再上文档门禁（D3），最后按需引入卫生门禁（D7）。覆盖率 100%（D6）对个人项目过重，不建议照搬。
