# Agent Note: 待办账本从单文件 markdown 改为 JSONL 单一账本

Status: implemented

## Problem

旧 `backlog/backlog.md` 是单文件 markdown 看板，24 个事项全靠标题里的手写标记
（`✅ 已完成` / `🔄 进行中` / `📋 方案已出` / `⏸ 待决策`）表达状态。三个缺陷均已实测：

1. **编号会撞车**：`B002` / `B003` / `B010` 各出现两次且指不同事项
   （`:172` Vercel 出口 vs `:208` M6 收尾；`:161` vedge 橙云 vs `:211` 未来拓展池；
   `:7` gate 多出口 vs `:254` CF worker 标记头）。`docs/product/specs/distributed-commercialization/FOLLOW-UPS.md`
   用「B005~B009」指事项、「docs/dev-team/reviews/2026-08-governance-audit.md` 的 F4 已判定
   「ID 冲突使跨文档引用失去确定性」。
2. **状态没有不变量**：分区标题（`## 进行中` / `## 已完成`）与事项真实状态脱节——
   「已完成」区里躺着未来池、仅出方案的提案与待办；`✅ 已完成` 没有证据字段，
   「完成」与「声称完成」不可区分。
3. **没有并发语义**：同一事项两个会话同时改，无租约、无 owner，只能靠人肉避让；
   而 `.dev-team/` 里另有一份执行态账本，双轨并存。

## Decision

采纳 pg-backlog v2（JSONL 单一账本），判据随仓、门禁双接线：

- **账本**：`backlog/tasks.jsonl` 为唯一活账本（`todo` / `in_progress`）；
  `resolve` 把 `done` 追加入 `backlog/archive/tasks-<YYYY>.jsonl` 并释放租约；
  `backlog/.next-id` 管编号高水位；租约 `.locks/<ID>.lock` 不入仓（已 gitignore）。
- **判据随仓**：`scripts/backlog.mjs`（`validateRecord` / `lint` / `doctor` / `record` /
  `start` / `resolve` / `reap`）逐字节复制自框架基线仓（pony-agent），本仓不改编排；
  复制时的指纹：`scripts/backlog.mjs` = `902bf84ad5516009b0172260b71575eb`（758 行）、
  `backlog-gate.sh` = `0a5bbdb7e12e70ffc6934be70e3bbb6c`、`backlog-gate.spec.sh` = `8e7dd74323bb1c958c6f1194f88e4489`。
  `lint` 判记录不变量，`doctor` 判跨归档 / 高水位 / 孤儿租约。
- **门禁**：`.meta/gates/backlog-gate.sh`（= `lint` + `doctor`）挂 `.githooks/pre-commit`（门缺失即
  fail-closed）与 `.github/workflows/ci.yml` 的 `backlog-ledger` job；
  负样本自检 `.meta/gates/backlog-gate.spec.sh`（P1 + N1–N7 + H）随仓。
  登记落 `.meta/gates/INSTANCE-GATES.md`——框架 `README.md` 在 ponygo 的 `UPGRADE_UNIVERSE` 内，
  往里加实例门会让 `ponygo upgrade --check` 长期报漂移（实测 rc=1），故实例门一律登记在实例文件。
- **迁移**：旧看板原文冻结到 `backlog/legacy/backlog.md`（一字不改）；24 个事项逐条落为记录，
  17 条活跃 + 7 条归档；`source_ref` 指回旧文件锚点，`evidence` 指回各事项原验收命令。
  旧锚点与账本编号的对照（含三处拆号）落在 `backlog/legacy/README.md`。
- **编号口径**：保留旧身份 `B001`–`B024`，使 `docs/`、`.agents/notes/` 里既有引用继续成立；
  旧看板三处同号不同义拆号为 `B022`（M6 实现收尾）/ `B023`（未来拓展池）/ `B024`（CF worker 标记头）；
  `B025`–`B055` 保留不用（CLI 编号下限 56），新任务从 `B056` 起——本迁移的执行项即 `B056`。
- **状态口径**：按事项内容判，不按旧分区判。旧「已完成」区里 B003→B023（未来池）、B004（按需启动）、
  B005（仅 proposed ADR）、B006–B009/B016（待办）落 `todo`；只有原文带完成标记或明写「已完成」的
  B011 / B012 / B013 / B015 / B020 / B022 / B024 落 `done`。
- **文档的家**：账本契约 `backlog/README.md`；实例门登记 `.meta/gates/INSTANCE-GATES.md`；
  本记录是决策理由的家。`docs/AGENTS.md` 的 tier 表新增「待办与执行账本」一行（新文档先登记家）。

## Alternatives considered

- **继续用 markdown 看板 + 人工约定「先 grep 再改」**：零成本，但编号撞车与状态漂移已是实测事故，
  没有退出码的约定等于没有约定——F4 登记了近三个月仍未修，正是这个模式的证据。
- **改用 GitHub Issues 或外部看板**：天然有 ID 与状态机，但账本离开仓库后门禁与 CI 无法寻址、
  离线不可用，且打断「提交信息引用 `backlog:Bnnn`」的 same-commit 纪律。
- **SQLite 账本**（更强的并发与崩溃安全）：写入更稳，但账本是需要 diff 审阅的文本资产，
  二进制账本让 review 与 blame 失去可读性，CI 还要多一层依赖。
- **保留旧编号段并把 CLI 编号下限从 56 改成 22**：编号更连续，但要改判定真源并补负样本——
  等于给本仓分叉一份私有判据，之后框架侧修复无法回流（本技能明示的「编号下限」取舍点）。

## Consequences

- 新增 / 迁移：`backlog/{README.md,tasks.jsonl,.next-id,archive/tasks-2026.jsonl,legacy/}`、
  `scripts/backlog.mjs`、`.meta/gates/backlog-gate{,.spec}.sh`；`.githooks/pre-commit` 与
  `.github/workflows/ci.yml` 各一处接线；`.gitignore` 增 `backlog/.locks/`。
- 机械验证命令（本仓可复跑）：`node scripts/backlog.mjs lint`、`node scripts/backlog.mjs doctor`、
  `bash .meta/gates/backlog-gate.sh`、`bash .meta/gates/backlog-gate.spec.sh`；
  回归面 `bash tests/governance/run.sh` 与 `ponygo status` 必须保持绿。
- **诚实边界（本决策不承诺的部分）**：
  1. `done` 记录的 `evidence` 是**指针证据**（指回旧看板原文的验收命令），迁移会话未复跑历史验收——
     即本账本不证明历史事项当时真的达成验收，只证明「旧看板声称完成」这件事被如实搬运。
  2. 旧「进行中」事项 `B010` 迁移为 `in_progress` 但无租约心跳（`owner=legacy-backlog-migration`）：
     复做前必须 `start` 接管，否则 `lint` 只在 `.locks/` 存在时才可能提示。
  3. 租约只保护「同一任务」，不保护账本追加：并发 `record` 仍可能丢行，靠「record 后立即提交 +
     冲突人工合并」兜底。
  4. 迁移脚本是一次性工具，未入仓（避免留下无消费者的死代码）；可审计的产物是
     `backlog/legacy/README.md` 的映射表与每条记录的 `source_ref`。
- **回退路径**：若账本的仪式成本被证实高于收益，回退形态 = 把 `tasks.jsonl` 重新渲染为 markdown
  并把门禁从 pre-commit / CI 解绑；届时必须先落新决策，不允许两套账并存（双轨正是本次要消灭的问题）。
