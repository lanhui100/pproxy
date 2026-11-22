# backlog/ —— 本仓待办与执行账本（v3：批次日志 + fold 现算 + 按批归档）

> **判定真源 = `scripts/backlog.mjs`**（`validateTask` / `lintV3` / `fold` / `doctor` / `verifyArchive`）。
> 本文是它的可读契约：冲突时以代码与命令输出为准，本文只描述模型，不复刻判据细节。

## 布局

| 路径 | 角色 | 入仓 |
|---|---|---|
| `backlog/MODEL` | 模型标记（内容 `v3`） | ✅ |
| `backlog/log/<批次>.jsonl` | 活跃批次日志（**写入单位**，只追加；只写本会话自己的批次） | ✅ |
| `backlog/archive/batches/<year>/<批次>.jsonl` | 已归档批次（只读，按批封存） | ✅ |
| `backlog/archive/batches/<year>/manifest.sha256` | 归档清单（`sha256  <仓库相对路径>`，追加） | ✅ |
| `backlog/archive/tasks-<YYYY>.jsonl` | v2 遗留滚动归档（只读，fold 时并入历史） | ✅ |
| `backlog/.locks/<ID>.lock` | 任务级租约（pid / 会话 / 心跳，**本地运行时**） | ❌ 已 gitignore |
| `backlog/.state/batch` | 当前工作区的活动批次 id（**本地运行时**） | ❌ 已 gitignore |
| `backlog/legacy/` | 冻结历史（旧 markdown 看板与迁移映射），只读、不参与校验 | ✅ |
| `backlog/README.md` | 本契约 | ✅ |

**账本根 = 当前 worktree 顶层**（不是 git common dir）：每个分支/工作树写自己的批次日志并随 PR 提交，
这是「并发记账零冲突」的来源；`PONY_BACKLOG_ROOT` 仅供夹具/测试覆盖。

## 事件模型（每行一个 JSON）

| 事件 | 载荷 | 语义 |
|---|---|---|
| `batch.start` | `model` / `batch_id` / `surface` / `release?` / `created_at` | 批次首行，一文件一个批次 |
| `record` | `at` / `task:<完整任务行>` | 新建任务；同 ID 只能出现一次 |
| `start` | `at` / `id` / `owner` / `paths[]` | 取租约 + 声明 touched_paths |
| `release` | `at` / `id` / `owner?` | 回退 `todo`，清锁 |
| `annotate` | `at` / `id` / `description?` / `priority?` | 改注释/优先级，不动状态 |
| `resolve` | `at` / `id` / `evidence` | 闭环 `done`，清锁 |
| `dedup` | `at` / `id` / `context?` | 同 fingerprint 去重命中（不新建任务） |

事件 `at` 必须**非递减**（lint 抓「事件时间倒退」）；写入只经 CLI，手写事件会被 lint 拒绝。

## 任务行（record 载荷）

- 必填：`id`（`B\d{3,}`，新 ID = `B`+epoch 毫秒）/ `category`（`Defect|Task|Debt`）/
  `priority`（`P0..P3`）/ `status`（`todo|in_progress|done`）/ `title` / `created_at`（ISO）/
  `metadata.fingerprint`。
- 条件必填：`in_progress` → `owner` + `touched_paths`（≥1 个仓内相对路径）；
  `done` → `evidence` + `closed_at`。
- 其他可用字段：`description`（≤2000 字符）/ `source_ref`（来源指针）/ `updated_at` /
  `leased_at` / `heartbeat_at` / `archived_at`；`metadata` 只接受 `fingerprint` /
  `occurred_count` / `triage_context`（≤8KiB）。未知字段一律拒绝。
- `evidence` 要能被**人复跑**（命令 + 退出码 + 关键输出），不是「改好了」。

## 状态机

`todo → in_progress → done`，允许 `in_progress → todo`（`release`）。**`done` 是终态**：
回归开新任务并用 `backlog:<旧 ID>` 引用旧任务，不复用已闭环 ID。

会话中断后用 `owner` / `touched_paths` / `description` 与 `git diff --stat` 交叉确认真实进度——
本仓不维护任何 plan 文件。

## 租约（并发控制）

- `start` 以 `O_EXCL` 原子取 `<ID>.lock`：同一任务同时只允许一个会话；`heartbeat` 刷新，
  `release` 释放，`resolve` 清锁。
- `reap --id <ID> --minutes N [--dry-run]` 只回收指定任务的超龄租约；参数是白名单，
  `--id=B056` / `-id` / 位置参数一律 exit 2，不会回落为全局清理。
- `.locks/`、`.state/` 是本地运行时状态，不入仓：干净克隆 / CI 缺锁只出 `NOTE`（不阻断）；
  只有本工作区确实存在 `.locks/` 时，「锁在而记录非 `in_progress`」才判违规。
- 租约只保护「同一任务」，不保护跨会话并发：不同会话写**不同批次文件**，合并零冲突。

## 归档（按批封存）

- 条件：**该批次引用到的任务全部 `done`**，且批次随版本发布（`--release <v>` 或发布后 housekeeping）。
- 动作：批次文件搬入 `archive/batches/<year>/`，并向该年 `manifest.sha256` **追加**哈希行。
- 归档只读：改归档文件会被 `verify-archive` 抓「sha256 不符」；未入清单的归档文件会被抓「未入清单」。
- 未闭环批次一律不归档（否则要在归档后开新批次补正）。

## 编号口径

- 新任务 ID = `B` + epoch 毫秒（跨会话不撞号，无需全局计数器；`.next-id` 已随 v2 退役）。
- 历史短编号（`B001`–`B071`）**原样保留**：跨文档既有引用（`docs/`、`.agents/notes/`）因此持续有效；
  旧看板内三处同号不同义的冲突已在首次迁移时拆号，映射表见 `backlog/legacy/README.md`。
- 提交信息与文档引用写 `backlog:<ID>`；裸短编号视为历史遗留编号。

## 命令

```bash
node scripts/backlog.mjs list [--surface tui]   # fold 现算看板（唯一读法）
node scripts/backlog.mjs fold                   # JSON：活跃/批次/问题（排障用）
node scripts/backlog.mjs record --category Defect --title "..." --fingerprint "core::x::y" --priority P1 --surface tui
node scripts/backlog.mjs start <ID> --owner lane-1 --paths "crates/x/src/y.rs"
node scripts/backlog.mjs heartbeat <ID> --owner lane-1
node scripts/backlog.mjs release <ID>           # 回退 todo 并清锁
node scripts/backlog.mjs resolve <ID> --evidence "cargo test --workspace 全绿；rc=0"
node scripts/backlog.mjs annotate <ID> --description "..." --priority P2   # 不动状态
node scripts/backlog.mjs lint && node scripts/backlog.mjs doctor && node scripts/backlog.mjs verify-archive
node scripts/backlog.mjs archive [--release <v>] [--before <ISO>] [--dry-run]
node scripts/backlog.mjs batch new --surface tui --release <v>   # 显式开新批次（可选）
node scripts/backlog.mjs reap --id <ID> --minutes 0              # 只清该任务的锁
```

`<ID>` 是占位符：真实编号用 `list` 或 `fold` 的输出（历史编号不连续，ID 不复用）。

## 门禁与接线

- 门禁：`bash .meta/gates/backlog-gate.sh`（v3 = `lint` + `doctor` + `verify-archive`）；负样本自检：
  `bash .meta/gates/backlog-gate.spec.sh`（P1–P3 放行 + N1–N8 拒绝 + H 接线）。登记见
  `.meta/gates/INSTANCE-GATES.md`。
- 本地层：`.githooks/pre-commit` 调门禁（门脚本缺失即 fail-closed）。
- 远端层：`.github/workflows/ci.yml` 的 `backlog-ledger` job 跑门 + 自检。
- 前提：`git config core.hooksPath .githooks`——git **不克隆**该配置，新 clone 需自行装配一次；
  未装配时本地门禁静默失效，仅 CI 兜底。
- 改判据（`scripts/backlog.mjs`）或改本文属非平凡改动：须同步改 CLI + 本文 + spec，并按
  `.agents/notes/` 写决策记录。

## 迁移与决策

账本当前为 v3（批次日志 + fold 现算 + 按批归档）。v2 单文件账本（`tasks.jsonl` + `.next-id`）的
迁移依据、ID 映射、字段取舍与回滚点见
`.agents/notes/implemented/process/2026-11-22-backlog-v3-batch-log.md`；旧看板身份映射见
`backlog/legacy/README.md`。`backlog/archive/tasks-<YYYY>.jsonl` 是 v2 遗留归档，只读。
