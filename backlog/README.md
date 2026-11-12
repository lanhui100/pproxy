# backlog/ —— 本仓待办与执行账本（唯一活账本）

> **判定真源 = `scripts/backlog.mjs`**（`validateRecord` / `lint` / `doctor`）。本文是它的可读契约：
> 冲突时以代码与命令输出为准，本文只描述模型，不复刻判据细节。

## 布局

| 路径 | 角色 | 入仓 |
|---|---|---|
| `backlog/tasks.jsonl` | 唯一活账本，一行一任务（`todo` / `in_progress`） | ✅ |
| `backlog/archive/tasks-<YYYY>.jsonl` | 闭环滚动归档（`resolve` 幂等追加，只存 `done`） | ✅ |
| `backlog/.next-id` | ID 高水位（下一个候选编号，单调不减） | ✅ |
| `backlog/.locks/<ID>.lock` | 任务级租约（含 pid / 会话 / 心跳，**本地运行时**） | ❌ 已 gitignore |
| `backlog/legacy/` | 冻结历史（旧 markdown 看板与迁移映射），只读、不参与校验 | ✅ |
| `backlog/README.md` | 本契约 | ✅ |

## 记录模型

- 必填：`id`（`B\d{3,}`）/ `category`（`Defect\|Task\|Debt`）/ `priority`（`P0..P3`）/
  `status`（`todo\|in_progress\|done`）/ `title` / `created_at`（ISO）/ `metadata.fingerprint`。
- 条件必填：`in_progress` → `owner` + `touched_paths`（≥1 个仓内相对路径）；
  `done` → `evidence` + `closed_at`。
- 其他可用字段：`description`（≤2000 字符）/ `source_ref`（来源指针）/ `updated_at` /
  `leased_at` / `heartbeat_at` / `archived_at`；`metadata` 只接受 `fingerprint` /
  `occurred_count` / `triage_context`（≤8KiB）。未知字段一律拒绝。
- `evidence` 要能被**人复跑**（命令 + 退出码 + 关键输出），不是「改好了」。

## 状态机

`todo → in_progress → done`，允许 `in_progress → todo`（`release`）。**`done` 只能由 `resolve` 写入**
（写入证据、追加归档并释放租约；从 `todo` 直接闭环也合法）。活跃池残留 `done` 由 `lint` 判违规。

手改 JSONL 的状态跳变 CLI 不拦，但「记录非 `in_progress` 却仍持锁」会被 `lint`/`doctor` 判违规。
会话中断后用 `owner` / `touched_paths` / `description` 与 `git diff --stat` 交叉确认真实进度——
本仓不维护任何 plan 文件。

## 租约（并发控制）

- `start` 以 `O_EXCL` 原子取 `<ID>.lock`：同一任务同时只允许一个会话；`heartbeat` 刷新，
  `release` 释放，`resolve` 归档同时清锁。
- `reap --id <ID> --minutes N [--dry-run]` 只回收指定任务的超龄租约；参数是白名单，
  `--id=B056` / `-id` / 位置参数一律 exit 2，不会回落为全局清理。
- `.locks/` 是本地运行时状态，不入仓：干净克隆 / CI 缺锁只出 `NOTE`（不阻断）；
  只有本工作区确实存在 `.locks/` 时，「锁在而记录非 `in_progress`」才判违规。
- 租约只保护「同一任务」，不保护账本追加：并发 `record` 仍可能丢行，靠「record 后立即提交 +
  冲突人工合并」兜底。

## 编号口径

- 编号永不复用：ID 由 `.next-id` 高水位分配，归档与删除都不回退。
- 历史遗留编号（`B001`–`B024`）沿用旧看板的事项身份，跨文档既有引用（`docs/`、`.agents/notes/`）因此保持有效；
  旧看板内三处**同号不同义**的冲突已在迁移时拆号，映射表见 `backlog/legacy/README.md`。
- `B025`–`B055` 保留不用：CLI 的编号下限是 56（与判定真源一致，本仓不改判据）；
  **新任务从 `B056` 起递增**。
- 提交信息与文档引用活跃任务写 `backlog:B056`；裸编号视为历史遗留编号。

## 命令

```bash
node scripts/backlog.mjs list                 # 看板（状态/优先级排序）
node scripts/backlog.mjs record --category Defect --title "..." --fingerprint "core::x::y" --priority P1
node scripts/backlog.mjs start B056 --owner lane-1 --paths "crates/x/src/y.rs"
node scripts/backlog.mjs heartbeat B056 --owner lane-1
node scripts/backlog.mjs release B056         # 回退 todo 并清锁
node scripts/backlog.mjs resolve B056 --evidence "cargo test --workspace 全绿；rc=0"
node scripts/backlog.mjs lint                 # 记录不变量（非零 = 违规）
node scripts/backlog.mjs doctor               # 跨归档/高水位/孤儿租约对账
node scripts/backlog.mjs reap --id B056 --minutes 0   # 只清该任务的锁
node scripts/backlog.mjs annotate B056 --description "..." --priority P2   # 改注释/优先级（不动状态）
```

## 门禁与接线

- 门禁：`bash .meta/gates/backlog-gate.sh`（= `lint` + `doctor`）；负样本自检：
  `bash .meta/gates/backlog-gate.spec.sh`（P1 + N1–N7 + H）。登记见 `.meta/gates/INSTANCE-GATES.md`。
- 本地层：`.githooks/pre-commit` 调门禁（门脚本缺失即 fail-closed）。
- 远端层：`.github/workflows/ci.yml` 的 `backlog-ledger` job 跑门 + 自检。
- 前提：`git config core.hooksPath .githooks`——git **不克隆**该配置，新 clone 需自行装配一次；
  未装配时本地门禁静默失效，仅 CI 兜底。
- 改判据（`scripts/backlog.mjs`）或改本文属非平凡改动：须同步改 CLI + 本文 + spec，并按
  `.agents/notes/` 写决策记录。
