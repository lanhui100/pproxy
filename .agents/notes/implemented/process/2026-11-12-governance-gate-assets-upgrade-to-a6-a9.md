# Agent Note: 治理门资产升级至 A6/A7/A8/A9 并恢复 pre-push 免绕过

Status: implemented

## Problem

`git push` 被 `.githooks/pre-push` 拦下（B020）：`bash tests/governance/run.sh` 16/17，唯一红项
`G4-SENTINEL-LOCAL`，根因是 `ponygo status` rc=1。该红项与业务改动无关，已在 `HEAD~1` 复现，
因此 2026-11-12 的会员开通发布按钩子声明的逃生口用了 `git push --no-verify`。

拆开看是两层缺口，不是一层：

1. **骨架缺件（4 个）**：`.meta/gates/{archive-verify,archive-verify.spec,no-residue,no-residue.spec}.sh`
   在本仓从未被追踪。缺件的后果不是"少跑一个检查"——`archive-verify.sh` 缺失时
   `ci-verify.sh` 的归档封印段无从执行，`archived/` 冻结就退回成礼貌约定。
2. **门资产内容漂移（3 个）**：`.meta/gates/{adr-verify.sh,adr-verify.spec.sh,ci-verify.sh}`
   与 ponygo 内嵌模板不一致，触发级自洽判据 2.1 FAIL。漂移的真实内容是框架把 A6 归档封印
   接进了本地门（`archive_probe` 在任何 `skip` 之前评估）——即"本次没有暂存 ADR"曾被当作
   绕过归档封印的路径，旧门资产带着这个缺陷。

`--no-verify` 是逃生口而不是修复手段：它让"红着推"成为可复用习惯，而钩子拦住的东西
（归档封印、1.11 负空间判据）恰恰是无人复核时会静默失效的那一类。

## Decision

**一次性补齐骨架 + 对齐门资产 + 重封归档，并从此禁止 `--no-verify` 推送。**

1. `ponygo init --repair-skeleton` 补 4 个缺失门资产（append-only：不覆盖既有文件）。
2. `ponygo upgrade apply` 把宇宙内 10 个漂移文件刷新为内嵌模板，含三个门资产与
   `.agents/skills/write-adr/verify-note.sh`（新增 1.11：`implemented/simplification/` 必须含
   `## Reintroduction conditions`；围栏内标题不算在场）。**门资产是框架不可变资产，实例侧不编辑**——
   这是本次选择的处置方向：不与模板对抗，跟随模板升级。
3. 模板刷新触及 `.agents/notes/archived/AGENTS.md`（归档区使用说明，属框架文档而非归档决策），
   同步 `bash .meta/gates/archive-seal.sh --write` 重封，保持 `SHA256SUMS` 与内容一致。
   归档区**决策条目**一字未动，封印语义（冲突改即红）不变。
4. 两套归档门分工固定，不再新增第三套：`archive-verify.sh` 用 `manifest.sha256`（框架 A6，
   按 append-only seal 索引归档件）；`archive-seal.sh` 用 `SHA256SUMS`（本仓实例门，G14 冻结契约）。
   两者都只覆盖 `archived/`，判定口径一致（内容与封印时不同即拒），差异仅在清单文件名与
   追加方式——共存不产生"同一文件两个真相"，因为两者读的是同一批字节。

## Alternatives considered

- **只补 4 个缺失骨架，不动漂移门资产**：`ponygo status` 仍 rc=1（2.1 判据不通过），G4 继续红，
  问题不回绿。而且缺 A6 接线的旧 `adr-verify.sh` 恰好保留了"无暂存 ADR 时归档封印可绕过"的缺陷，
  补了新资产却不接线，等于把门放在抽屉里。
- **改 G4 判据为"骨架缺口仅告警"**：能让 17/17 立刻变绿，但绿的是判据而不是仓库——缺口仍在，
  下一次有人删归档件仍无人拦。Wave-1 断言冻结后为过门而放宽判据，是把治理改造成记分牌。
- **继续用 `git push --no-verify`**：本次实测 17/17 全绿，说明钩子拦的是真实缺口而非误报；
  逃生口一旦常态化，"钩子红了先绕过"就成为默认动作，后续红项会被当作噪声。
- **保留 `.agents/notes/archived/AGENTS.md` 漂移不刷新**：`ponygo upgrade --check` 会长期红，
  且归档说明缺失"`git mv` + `Status:` 保持 + seal 同提交"这条纪律——正是下一次归档最需要看的一段。
  选择刷新 + 重封，代价是归档区多一次内容变更，收益是纪律文档与门资产同版本。
- **手工逐条合入 10 处差异（不跑 apply）**：`apply` 写入是原子替换且幂等，先在 `/tmp` 的
  `git worktree` 副本里预演过完整升级（`ponygo status` rc=0、`verify-note.sh` 全树 rc=0、
  `ci-verify.sh` 四段 rc=0），再落到真实仓库；手工合入反而引入"逐字对齐失败"的风险。

## Consequences

- `tests/governance/run.sh` **17/17 PASS**（`ACCEPT_EXIT=0`），G4 由 `ponygo status rc=0` 支撑；
  `ponygo upgrade --check` 零漂移。`git push` 恢复可用，**不再允许 `--no-verify`**。
- 提交面变严两处，属于预期收紧：
  - 暂存/全树门会跑 `archive-verify.sh check`：`archived/` 条目被改一字节即拒提交；
  - `verify-note.sh` 1.11：`implemented/simplification/` 缺 `## Reintroduction conditions` 即拒。
- `.meta/skills-library/` 9 个可选资产随 `--repair-skeleton` 入场（框架附带，本仓不主动使用）。
- 已知局限（靠 review）：`ponygo upgrade` 只比对宇宙内十七文件，宇宙外文件（根 README、
  `meta.yaml`、已填 constitution）无基线、漂移不判定；`archive-seal.sh --write` 会按当前内容重封，
  重封动作本身不可自证"内容是对的"——本次是框架文档升级，判定靠上述 diff。

## Verification

```bash
ponygo status >/dev/null; echo $?                        # 0
ponygo upgrade --check; echo $?                          # 0（零漂移）
bash .meta/gates/ci-verify.sh; echo $?                    # 0（四段：ADR 格式/门自检/归档封印/引用残留）
bash tests/governance/run.sh | tail -3                    # PASS=17 FAIL=0 / ACCEPT_EXIT=0
bash .meta/gates/archive-seal.sh --check; echo $?         # 0
```
