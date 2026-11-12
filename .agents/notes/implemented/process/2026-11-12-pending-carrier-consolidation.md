# Agent Note: 待办载体收口——唯一活账本是 backlog/，其余载体定性归档

Status: implemented

## Problem

账本统一到 `backlog/tasks.jsonl` 之后，仓里仍散着若干"看起来像待办清单"的载体。它们本身不是账本，
但都能被误读成当前待办，正是双轨问题的剩余面：

1. `.dev-team/` 混放「运行账本（state / nfr-baseline / contract）」与「门禁收据（review / 相位记录 / 元复盘）」，
   其中 `state.json`、`review-join-*` 是已完成会话的看板快照，冷启动读它会把已收口的波次当成在飞状态。
2. `docs/ops/DESKTOP-TROUBLESHOOTING.md` 的「遗留与待决（审核后更新）」表是 2026-08-28 的时点快照，
   7 个数据行里 6 个已被同文件后续章节关闭或对象已被删除，只有 S8 仍成立，但没人知道这一点。
3. `.agents/notes/proposed/process/2026-10-30-governance-audit.md` 的 A1–A8 已全部落地，
   状态却仍写 `proposed`——状态轴失真：读的人会把已兑现的承诺当成待办。
4. `docs/dev-team/tasks/T004` 与 `specs/S004` 是已交付任务书；`.agents/tests/stability-2026-09-29/`
   是稳定性测试战役产物；`FOLLOW-UPS.md` 的 `#F-07` 是明确「不占 B 编号」的跟踪项。
5. `.agents/tests/stability-2026-09-29/test-token.txt` 留在磁盘（未追踪、内容已脱敏）属于纯残渣。

## Decision

**唯一活账本原则**：任何"还有没有活要干"的判断只读 `backlog/tasks.jsonl`。其余载体按下表定性，
且各自的事实只有一个家：

| 载体 | 定性 | 处置 |
|---|---|---|
| `.dev-team/` | dev-team skill 的执行态账本 + 门禁收据，**不是看板** | **保持平铺，不建 archive 子目录**；新增 `.dev-team/README.md` 记布局约束、文件归属、已知断链 |
| `docs/ops/DESKTOP-TROUBLESHOOTING.md` 遗留表 | 时点快照（事故年表的一部分） | 表头加「这是 2026-08-28 快照，当前待办只看账本」+ 逐项结局指路；**不删表**（删即篡改年表） |
| S8（`crates/engine` 与 `crates/server` 双份实现） | 唯一仍成立的待办 | 登记为 `backlog:B058`（P2/Debt），描述里带行数实测与 B016 第 5 项的部分重叠关系 |
| `proposed/process/2026-10-30-governance-audit.md` | 已全部落地（A9-3 由另一个决定收口） | 迁 `implemented/process/`，`## Proposal`→`## Decision`，`## Acceptance criteria`/`## Risks` 折叠进 `## Consequences` |
| `docs/dev-team/tasks/T004` + `specs/S004` | 已交付任务书，行为仍活在码里 | **留在原地**（`docs/AGENTS.md` 已把 `docs/dev-team/` 定为 spec/任务书的家） |
| `.agents/tests/stability-2026-09-29/` | 测试战役产物，被 `docs/ops/stability-report-2026-09-29.md` 与 ADR 引用 | 目录保留；仅删未追踪的 `test-token.txt` 残渣 |
| `FOLLOW-UPS.md` `#F-07` | 文档内跟踪项（原文声明不占 B 编号） | 不动、不登记（按用户裁决） |
| `docs/AGENTS.md` | 文档家索引 | 「团队评审与任务书」行补指 `.dev-team/README.md` |

**关键约束（机器证据，别再用直觉推翻）**：`.dev-team/` 下的 `review-*.json` 与
`meta-retrospective*.md` **不能移进子目录**——`~/.agents/skills/dev-team/scripts/dev-retro.py:247/:695`
与 `dev-meta-lint.py:35-37` 用非递归 flat glob 读它们。2026-11-12 临时克隆实测：把收据移入
`.dev-team/archive/` 后 `dev-retro.py receipt-gate` 的 `receipts_total` 由 9 归零并 exit 1、
`dev-meta-lint.py` 报缺阶段收据 exit 1；平铺时 `dev-meta-lint.py` exit 0 PASS。

## Alternatives considered

- **把已完成会话的收据整体移进 `.dev-team/archive/<session>/`（最初的倾向）**：目录最清爽，
  但被打断的是 Phase 5 收口门（`dev-retro.py receipt-gate`、`dev-meta-lint.py` 双双 exit 1，已实测）——
  用"整洁"换掉一台真实的门禁，不成立。
- **删掉 `DESKTOP-TROUBLESHOOTING.md` 的遗留表**：能立刻消掉误读面，但该文档是事故年表，
  删历史表等于篡改记录；改为"表头标注快照 + 结局指路"既保真又消歧。
- **把 `.dev-team` 整个加进 `.gitignore`**：dev-team skill 明确要求该目录纳入版本库
  （SKILL.md「`.dev-team/` 目录纳入版本库，不 gitignore」），且 `contract.md` 是 `tests/acceptance/`
  的合同唯一依据——忽略即切断测试契约的引用面。
- **保留 `proposed` 只加"落地现状"段**：成本最低，但状态轴继续失真（proposed 里躺着已兑现的承诺），
  且 write-adr 明确要求部分/全部落地后拆条或迁移，故按迁移处理。
- **给 S8 只留一张文档表、不登记账本**：省一次登记，但 S8 会重新变成一个没有机器可查状态的看板项，
  正是本次要消灭的形态。
- **顺手删掉零引用的 `NeedSetupGuide.vue`**：代码侧确为零引用（`grep -rn NeedSetupGuide desktop/` 零命中），
  但 `docs/product/specs/m7-frontend-ux/SPEC.md` 把它写成"五个页面统一渲染的共享组件"，
  删组件等于替产品改口径——**保留，交 review 决定删或接回壳层**。

## Consequences

- 新文件：`.dev-team/README.md`（布局约束 + 文件归属表 + 已知断链）、本记录。
- 状态迁移：`.agents/notes/{proposed→implemented}/process/2026-10-30-governance-audit.md`（内容按 implemented 骨架重写）。
- 账本：新增 `backlog:B058`（S8 双份实现未收敛）。落地时快照：活跃池 18 条、归档 9 条、`.next-id` 59
  （此后账本由后续任务继续推进，条数以 `node scripts/backlog.mjs list` 为准）。
- 文档：`docs/ops/DESKTOP-TROUBLESHOOTING.md` 遗留表加快照声明；`docs/AGENTS.md` 团队评审行补指 `.dev-team/README.md`。
- 清理：`.agents/tests/stability-2026-09-29/test-token.txt`（未追踪、内容已脱敏）删除。
- **诚实边界**：
  1. `NeedSetupGuide.vue` 与 m7 SPEC 的口径不一致**未修**——属产品决定（删组件 vs 接回壳层），
     本记录只把它显式化，代码与文档都按现状保留。
  2. `.dev-team/{contracts/2026-10-09-pproxy-accept-loop.md,nfr-baseline-pproxy-accept-loop.json}`
     被 4 处（3 个 `.rs` 注释 + `deploy/watchdog/README.md`）引用但从未入库（ADR 2026-10-19 已自述），
     **未修**：补文件还是改注释需要重构归属的结论，与本记录的清理动作无关。
  3. `proposed/process/2026-10-30-governance-audit.md` 的 A9-3（`ponygo upgrade --check` 进 CI）
     不是"完成"而是"被另一个决定取代"：G5 契约改为传递闭包（workflow → `ci-verify.sh` → `verify-note.sh`），
     即 **PR 流不以 `upgrade --check` 作为门禁**——注意 CI 里仍按钉死 commit 安装 ponygo oracle
     （`.github/workflows/ponygo-governance.yml:32-42`）只为 `tests/governance/run.sh` 的 G4/G6 提供
     `ponygo status` 文案，不跑漂移检查。迁移后的 note 按这个精确口径写明偏移，不冒充已落地。
  4. 泄漏 token 的**轮换**属外部动作，机器不可验，仍标「靠 review」。
  5. `.agents/tests/stability-2026-09-29/reverse/probe.sh:6` 仍 `cat` 本次删掉的 `test-token.txt`：
     该令牌文件从未入库，干净克隆本来就没有它，脚本复现前须自行重建（这不是新引入的依赖，
     而是把「删了本地残渣」与「脚本的输入前提」两件事显式分开）。
