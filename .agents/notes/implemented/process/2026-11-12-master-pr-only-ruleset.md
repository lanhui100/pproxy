# Agent Note: master 合入通道改为服务端强制 PR（ruleset 空 bypass）

Status: implemented

## Problem

远端的 required check 从来没对管理员生效过。经典分支保护 `branches/master/protection` 里
`enforce_admins.enabled = false`、且没有 required pull request——仓库 owner 直推时 GitHub
只提示 `Bypassed rule violations: Required status check "governance" is expected`，推送照成。
于是"governance 必绿"是对非管理员说的，而本仓只有一个管理员：承诺没有对象。

本地层同样不够：`.githooks/pre-push` 跑 `tests/governance/run.sh` 全树验收（B020 已修好），
但它是本地钩子，`git push --no-verify` 一次即可整体跳过。把门禁可信度押在"操作者不跳"
上，与 B020 之前的状态同构——逃生口一旦存在就被用。

两层缺口同一个根因：**远端层没有一条对仓库管理员也生效的 ref 更新规则**。

## Decision

在 `refs/heads/master` 上挂仓库规则集 `master-protection-pr`（id `27096701`），
`bypass_actors` 为空数组，API 回 `current_user_can_bypass: never`：

- `pull_request`（`required_approving_review_count = 0`）：必经 PR，solo 维护可自合并；
- `required_status_checks`（context `governance`）：检查未绿不可合并；
- `deletion` + `non_fast_forward`：禁止删分支与强推；
- 经典分支保护同步开 `enforce_admins = true`：两层语义同向，任一失效另一层仍在。

规则集只覆盖 `refs/heads/master`：tag（`desktop-v*` / `cli-v*`）照旧可直推触发发布工作流。
本地 pre-push 保留（跑得快、反馈早），但它不再是唯一防线。
操作口径与取证命令落在 `docs/ops/DEPLOY.md`「合入 master 的通道（服务端强制 PR）」。

## Alternatives considered

- **只把经典保护的 `enforce_admins` 打开，不建 ruleset**：经典保护对仓库 owner 的
  bypass 语义不闭环，且 owner 在 settings 里一键就能关掉；ruleset 空 bypass 是唯一
  "连我自己也得走 PR" 的形态（实测 `current_user_can_bypass: never`）。
- **`required_approving_review_count = 1`**：solo 仓库无法自审，PR 会永久待审，
  门禁退化成僵尸流程，最后一定有人去放宽规则——比没有规则更糟（规则被证明可绕）。
  取 0 保留"PR + 必过 CI"这条真实约束，评审质量交给 review 流程与决策记录，不交给机械计数。
- **用 Actions 自建"提交方式检查"job 代替服务端规则**：job 只能事后把提交标红，
  不能阻止 ref 前移；能阻止的只有服务端规则。事后红 + 已进主干的代码 = 无约束。
- **继续只靠"禁止 `--no-verify`"的纪律（B020 结论）**：B020 的教训就是纪律层会漏。
  纪律保留（钩子照跑、照旧禁 `--no-verify`），但不再假设它是防线。
- **把 release tag 也纳入规则集**：会让 `desktop-v0.3.x` / `cli-v*` 发布链路失效，
  PR 无法替代"打 tag 即构建"，收益为负。

## Consequences

- 直推 master 被服务端拒绝：`GH013: Repository rule violations found for refs/heads/master`
  → `Changes must be made through a pull request.` + `Required status check "governance" is
  expected.`，`git push` 退出码 1（实测收据见 `backlog` B060）。
- 每次变更多一次分支 push、开 PR、等检查、合并；换来的是"governance 绿"从承诺变成合入前提。
- 规则只活在仓库设置里，不随代码分发：`docs/ops/DEPLOY.md` 给出取证命令，
  避免"文档写强制、实际没开"的形意分离（P6）。
- 诚实边界（靠 review）：ruleset 可被仓库管理员用 API/设置删除，本条防的是"无意绕过"，
  不防"有管理权的人主动拆门"。要防后者需外部审计/双管理员审批，不在本次范围。
- 远端 required check 只列 `governance`：`ci` 与 `desktop-gate` 仍会跑但不阻断合并，
  这是刻意保留的——把无谓红的 job 设成必过会让 PR 卡在无关环境问题上。
