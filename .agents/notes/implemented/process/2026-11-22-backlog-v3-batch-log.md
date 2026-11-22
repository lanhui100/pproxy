# Agent Note: 待办账本从 v2 单文件 JSONL 升级为 v3 批次日志 + fold 现算 + 按批归档

Status: implemented

## Problem

2026-11-12 采纳的 pg-backlog v2（`backlog/tasks.jsonl` 单一活账本 + `backlog/.next-id` 全局计数器，
见 `implemented/process/2026-11-12-backlog-v2-jsonl-ledger.md`）解决了「markdown 看板无状态不变量」，
但把并发冲突集中到了两个单点上：

1. **写入撞车是结构性的**：任何两个会话/PR 记账都写同一文件的行尾（git 三方合并对同行尾追加敏感）
   且共享同一 `.next-id` 计数器——两个分支各自 `record` 后合并，冲突必然落在最后一行与计数器上，
   只能人肉重放。
2. **闭环归档无封存**：v2 的 `archive/tasks-<YYYY>.jsonl` 是滚动切片，「已完成」没有按批次边界，
   无法回答「这批任务是否随版本发布、能不能冻结」，也没有任何哈希证据防止归档被改。
3. **判据与模型耦合在单文件上**：`lint`/`doctor` 只能对「一个活跃池 + 一个归档文件」做对账，
   拿不到「批次」这一层，无法做按发布面（surface）/ 版本的归档时机判断。

同时本仓账本里 26 条活跃 + 14 条已归档记录需要无损搬迁，且`.agents/` 契约要求迁移是单提交、可回滚。

## Decision

升级到 pg-backlog v3，工具链从技能基线 `assets/` 复制入仓（`scripts/backlog.mjs`、
`scripts/backlog-migrate-v2-v3.mjs`、`.meta/gates/backlog-gate.sh`、`.meta/gates/backlog-gate.spec.sh`），
判据随仓、不跨仓调用。

- **写入单位 = 批次**：一次会话按发布面选出的任务写进 `backlog/log/<批次>.jsonl`（只追加、只写自己的
  批次，批次 id = `YYYYMMDDTHHMMSSZ-<surface>-<4hex>`）。不同会话 = 不同文件 ⇒ 合并零冲突，
  不再需要 `.next-id` 全局计数器。
- **读 = fold 现算**：视图由所有活跃批次 + 归档批次 + v2 遗留归档（只读）折叠得出，不落盘；
  `list` / `fold` 是唯一读法，禁止手改视图。
- **状态机与证据**：`todo → in_progress → done`（`done` 终态，回归开新任务并引用旧 ID）；
  `in_progress` 必填 `owner` + `touched_paths`，`done` 必填 `evidence` + `closed_at`。
- **ID 口径**：新任务 `B` + epoch 毫秒（跨会话不撞号）；**旧短 ID `B001`–`B071` 原样保留**，
  既有 `docs/`、`.agents/notes/` 引用继续有效，引用写作 `backlog:<ID>`。
- **归档时机**：批次内任务全部 `done` **且随版本发布后**（`archive --release <v>` 或发版 housekeeping）
  才搬入 `backlog/archive/batches/<year>/`，并向该年 `manifest.sha256` 追加 `sha256  <仓库相对路径>`；
  `verify-archive` 抓「sha256 不符」「归档未入清单」。归档文件只读。
- **根解析**：账本根 = **当前 worktree 顶层**（v2 用的是 git common dir）。多 worktree/分支各写各的
  批次并随各自 PR 提交，这正是零冲突的来源；`PONY_BACKLOG_ROOT` 仅用于夹具。
- **迁移**（本次执行）：`git tag backup/backlog-v2-20261122-2130` 为物理回滚点；
  `--surface legacy-migration` 整库迁移生成基线批次
  `backlog/log/20261122T133157Z-legacy-migration-afe4.jsonl`；对账收据 **迁移前活跃 26 / done 14 →
  迁移后 fold 活跃 26 / done 14**；随后 `git rm backlog/tasks.jsonl backlog/.next-id`，与
  `MODEL`/`log/`/文档/门禁改动同一个提交。

### 迁移中修正的判据冲突（随仓改动）

基线迁移脚本按 v2 文件行序发出 `record` 事件，用 `task.created_at` 作 `at`。本仓 v2 账本是
**按 ID 顺序**累积的（首次 legacy 迁移逐条抄自旧看板），`created_at` 不单调，于是 26 行里 12 行
触发 v3 lint 的「事件时间倒退」，脚本按设计自动回滚（无半成品残留）。
修正方式：**事件按 `at` 稳定排序后重放**（`resolve.at` 若早于其 `record.at` 则夹到 `record.at`），
只改事件排列顺序、不改任何 task 字段。这条改动写在仓内迁移脚本里并加了注释，属「按仓情调整模板」。

### 字段与功能的取舍

- v2 的 `.next-id`（编号高水位）**丢弃**：v3 由 epoch 毫秒 ID 取代；历史短号是遗留身份、不参与比较。
- 旧 CLI 的 `import`（把 `.agents/backlog/tasks.jsonl` 导入）**丢弃**：一次性命令，
  `grep -rn "backlog.mjs import"` 在 `docs/`、`.agents/notes/`、`.meta/` 无消费者。
- 任务行字段**零丢弃**：26 条记录的 `id`/`status`/`title`/`created_at`/`owner`/`touched_paths`/
  `source_ref`/`description`/`metadata.fingerprint` 全部原样进入 `record` 事件；
  14 条 v2 归档继续以只读历史并入 fold（`legacyArchived` 计数）。
- 门禁语义升级：`lint + doctor` → `lint + doctor + verify-archive`；负样本从 P1 + N1–N7 扩到
  P1–P3 + N1–N8 + H。spec 的 H 段原本假定本仓有 `scripts/git-guard.mjs`（框架基线仓的前提），
  本仓 `pre-commit` 自包含，故改 H fixture 只拷门——**这是修「用别人的前提判假红」，不是放松门禁**。

## Alternatives considered

- **继续用 v2，靠「record 后立即提交 + 人工合并」兜底**：零迁移成本，但冲突面是文件行尾 + 计数器，
  冲突频率随并发会话数线性上升；本仓已有 `.agents/notes` 记录多次账本合并手工重放，属于已知成本。
- **v3 但保留 `.next-id` 作编号分配器**：编号可读（`B072` 而非 `B1763...`），但恢复了跨分支共享
  可变单点——正是要消除的冲突源；且 epoch ID 让 ID 自带时间序，省掉「高水位越界」这条判据。
- **改用 GitHub Issues / 外部看板**：天然 ID 与状态机、并发由服务端管，但账本离开仓库后门禁与 CI
  无法寻址、离线不可用，且打断「提交信息引用 `backlog:<ID>`」的 same-commit 纪律（与 v2 决策同因被否）。
- **迁移时给历史短 ID 重编号（`B001`→`B<epoch>`）**：ID 口径统一，但 `docs/`、`.agents/notes/` 里
  大量既有引用立刻失效，需全仓批量改写且对外部协作者不可追踪——identity 稳定性优先于口径统一。
- **只把 `MODEL` 与批次日志落盘，`tasks.jsonl` 留一版过渡**：可以少一个删除动作，但会同时存在两个
  「活跃池」真源，`fold` 与 `tasks.jsonl` 一旦漂移就无法判定谁对——正是 §6 常见坑里的半迁移。
- **迁移拆成两个提交（先落 v3 文件、再删 v2 文件）**：diff 更小，但中间态会让 `doctor` 报布局不一致，
  且回滚只能回滚一半；单提交 + `backup/backlog-v2-*` tag 的回滚成本更低。

## Consequences

- 新增：`backlog/MODEL`、`backlog/log/20261122T133157Z-legacy-migration-afe4.jsonl`、
  `scripts/backlog-migrate-v2-v3.mjs`；`scripts/backlog.mjs` 换成 v3 版本（v2 只读兼容）；
  `.meta/gates/backlog-gate{,.spec}.sh` 升级到 v3 语义。
- 删除：`backlog/tasks.jsonl`、`backlog/.next-id`；`.gitignore` 增 `backlog/.state/`（本地运行时）。
- 文档 same-commit：`backlog/README.md` 重写为 v3 契约；`.meta/gates/INSTANCE-GATES.md` 换判据描述；
  `.githooks/pre-commit` 与 `.github/workflows/ci.yml` 的注释/step 名同步 v3。
- 机械验证（本仓可复跑，退出码即收据）：
  ```bash
  node scripts/backlog.mjs lint && node scripts/backlog.mjs doctor && node scripts/backlog.mjs verify-archive
  bash .meta/gates/backlog-gate.sh && bash .meta/gates/backlog-gate.spec.sh
  cat backlog/MODEL && git ls-files backlog/tasks.jsonl backlog/.next-id   # v3 / 空
  node scripts/backlog.mjs fold | head -40                                 # 活跃 26 / done 14
  ```
- **诚实边界**：
  1. 历史 14 条 `done` 的 `evidence` 仍是指针证据（指回旧看板/历史验收命令），本次迁移未复跑它们；
     迁移证明的是「旧账本被如实搬运」，不是「历史事项当年真的达成验收」。
  2. `B010` 迁移后仍是 `in_progress`（`owner=legacy-backlog-migration`，无租约心跳）：复做前需
     `start` 接管；干净克隆下 `lint` 只出 `NOTE`（`.locks/` 不入仓）。
  3. 本次迁移基线批次**未归档**（活跃任务未全 done），归档时机按上面的发布条件由后续 housekeeping 履行。
  4. 判据的语义真实性靠 review：`lint` 能抓结构/时间倒退/重复 record，抓不到「事件内容编造」。
- **回退路径**：`git reset --hard backup/backlog-v2-20261122-2130`（或 `git revert` 迁移提交）回到 v2
  单文件账本；回滚后若要再迁，先删残留 `backlog/log/` 再重跑脚本（脚本拒绝重复迁移）。
- 取代关系：本条取代 `implemented/process/2026-11-12-backlog-v2-jsonl-ledger.md` 的账本载体决策，
  该条的「编号 identity 保留」「实例门登记在实例文件」「evidence 必须是可复跑命令」三条继续有效。
