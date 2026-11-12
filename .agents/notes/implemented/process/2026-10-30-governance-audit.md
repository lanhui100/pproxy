# Agent Note: 治理审查 2026-10-30 的处置：L2 声明态与真实性缺口（A1–A9）

Status: implemented

## Problem

pproxy 的 `.meta/meta.yaml` 声明 `level: 2`（「承诺可验」），而 L2 当时的「承诺可验」只覆盖
**格式**层：Status↔目录自洽、零模板占位、proposed 状态真实、验收套件 31 PASS。它不覆盖四类
真实性问题——「记录与现实一致」「新旧决策接替」「绕过兜底」「凭据不进版本库」。`ponygo status`
当时自报 `2.4 本地层未接 ADR 门` 与 `级自洽验证：未通过`，即 `level: 2` 是**声明态**；采纳 ADR
本身也自认「当前自洽于 L1」（`.agents/notes/implemented/process/2026-09-25-adopt-ponygo-governance.md:14`）。

审查现场机械证据（cwd=`/home/dm/pproxy`，2026-10-30；「当时」列全部为现场值，**当前实测状态见
`## Consequences`**）：

| 命令 | 当时退出码 | 结论片段 |
|---|---|---|
| `ponygo status` | 1 | `治理级：L2` / `决策数：45` / `2.4 本地层未接 ADR 门` / `级自洽验证：未通过` |
| `ponygo audit`（M 档） | 1 | `命中 13 / 14`；`缺口与最小改进动作` 段为空 |
| `ponygo upgrade --check` | 1 | 4 处 drift + 3 处缺口（A1/A2/A4 门禁资产缺） |
| `ponygo observe status` | 0 | `声明: 缺失`（目标 ob1） |
| `bash .agents/skills/write-adr/verify-note.sh` | 0 | 1.1–1.9 全绿（但本仓副本是旧版，见 #5） |
| `bash tests/acceptance/run.sh` | 0 | `PASS=31 FAIL=0 SKIP=1`（G4 需生产令牌，已豁免） |

**水位一句话**：机械骨架是健康的（45/45 Status↔目录自洽、零模板占位、proposed 状态真实、
34/45 note 与代码同提交、验收套件 31 PASS），但 L2 的「承诺可验」只覆盖了**格式**，
不覆盖「记录与现实一致」「新旧决策接替」「绕过兜底」「凭据不进版本库」；`level: 2` 当时是
**声明态**。

### 问题清单（按严重度，2026-10-30 现场观测）

| # | 问题 | 严重度 | 磁盘证据（路径 + 命令 → 关键输出） | 最小改进动作 |
|---|---|---|---|---|
| 1 | **明文凭据已入版本库，且全仓无 secret 扫描门**：管理面 admin token、网关 key、数据面 token 与 24h 网关日志被跟踪 | **P0** | `git grep -lE 'pony_admin_[0-9a-f]{8}\|sk-pony-[0-9a-f]{8}'` → 5 个已跟踪文件，均位于 `.agents/tests/stability-2026-09-29/`（`PLAN.md`、`ponyllm/inference.py`、`ponyllm/summary.md`、`ponyllm/logs/gateway-logs-24h.txt`、`reverse/control.sh`），另 `test-token.txt` 含数据面 token；引入提交 `55df3af`（2026-09-29）；`.githooks/pre-commit` 无扫描步骤，`.github/workflows` 5 个 workflow 亦无 | ①按轮换清单作废旧 token（先禁用后替换）②5 文件脱敏为 `<REDACTED>` 并裁剪日志，或整目录移出版本库 ③补 secret 扫描门（本地 pre-commit + CI 各一次，非零退出） |
| 2 | **`implemented` ADR 的代码从未进入任何提交**：两条 note 随 `68f364a` 入库，但其修复只在 11 天未提交的工作区 | **P0** | `git show HEAD:crates/server/src/gateway.rs \| grep -c absolute_form` → `0`；`git log -S'absolute_form' -- crates/server/src/gateway.rs` → 空；`git diff --stat -- crates/server/src/gateway.rs crates/server/tests/saturation_contract.rs` → `2 files changed, 120 insertions(+), 11 deletions(-)`；note：`implemented/bug-fix/2026-10-19-fix-absolute-form-masquerading-as-401.md`、`…-fix-saturation-contract-ansi-parsing.md` | 把工作区改动与两条 note 一并提交（或改 note 为 proposed）；提交前自检 `git log -S<Decision 关键符号>` 非空才允许 `Status: implemented` |
| 3 | **声明 L2 但 2.4 判据 FAIL，且有两个因由**（status 只报第一个）；`ponygo init` 从未在本仓跑过 | P1 | `sed -n '/>>> ponygo-gate: adr-verify v1 >>>/,/<<< ponygo-gate: adr-verify v1 <<</p' .githooks/pre-commit` → 0 行；`test -f .meta/gates/adr-verify.sh` → NO；`git log --all -- .meta/adaptation` → 空；A1/A2/A4/A5 同源缺失；`306c5dd`（09-25）后 35 天无治理文件第二次提交 | `bash ponygo init` 装配哨兵块 + A1/A2/A4/A5 + 适配任务书，再 `ponygo status` 复核 2.4 转绿；若两周内不补，则 `meta.yaml level: 1` 并留降级 decision |
| 4 | **远端层零治理门 + 无 pre-push 中继层，本地门可 `--no-verify` 绕过** | P1 | `grep -rn 'ponygo\|check-adr\|verify-note' .github/workflows/` → 4 命中全是 `*.ponygo.fun` 域名，0 处门禁调用；`ls .githooks` → 仅 pre-commit；同构沙盒实测：正常 `git commit` 被 GATE BLOCKED，`git commit --no-verify` 直接成功（`/tmp/pproxy-audit/hookdemo`） | 补 `.meta/gates/ci-verify.sh` + `.github/workflows/ponygo-governance.yml`（PR 触发，跑 `verify-note.sh` 与 `ponygo upgrade --check`）；pre-push 按需补 10 秒级门 |
| 5 | **宪法命约 1 的 `## Alternatives considered`（判据 1.10）无机械看守**：本仓 verify-note 是 1.10 之前的旧版 | P1 | `grep -c 'Alternatives' .agents/skills/write-adr/verify-note.sh` → `0`；行数 118 vs 框架模板 140；用模板真源跑整树 `exit 0`（存量合规、**未来无拦**） | 把 `.agents/skills/write-adr/{SKILL.md,verify-note.sh}` 升到当前模板（顺带消 2 处 drift），再让 A1/A4 经它判定 |
| 6 | **适配决策未登记，且 WARN 指向的任务书在磁盘上不存在**；`docs/architecture/decisions/` 8 篇旧 ADR 与 notes 双轨并行、无冻结口径 | P1 | `ls .meta/adaptation` → No such file；`find .agents/notes -name '*alt-governance*'` → 空；`ls docs/architecture/decisions` → 001–008（最新 2026-09-29）；双轨决定散在 `2026-09-25-adopt-ponygo-governance.md` | 落一条 `implemented/process/*alt-governance*.md`（拆双轨段 + 写明旧 ADR 冻结/迁移口径），WARN 即静默；`ponygo init` 生成任务书使指引可执行 |
| 7 | **文档分层双家且口径冲突**：`.meta/docs-tier/README.md` 与 `docs/AGENTS.md` 两份 tier 表结论相反；`desktop/` 被标 `✓` 却无任何 README/AGENTS；doc-sync/词数预算门 0 实现 | P1 | `grep -nE '待填\|待补' docs/AGENTS.md` → 5 行；`ls desktop/README.md desktop/AGENTS.md` → No such file；`grep -rniE 'doc-sync\|word.?budget' --include='*.sh' .` → 仅第三方包命中 | 留一个家（`docs/AGENTS.md` 为真源，docs-tier 只留激活状态+清单）；补 `desktop/README.md`；词数门要么落地（`scripts/check-docs-budget.sh` + 负样本 spec）要么改成「未实现，靠 review」 |
| 8 | **same-commit 欠账：近 5 提交 3 条**，全部落在无包文档目录 | P1 | `d0a661b` 新增 `PPROXY_YOUTUBE_PROXY`：`grep -rn 'PPROXY_YOUTUBE_PROXY' README.md docs/` → 0（只在源码）；`68f364a` 非-notes 文档只改了 `backlog/backlog.md` + `tests/acceptance/README.md`；`62e055b` 改 PAC 行为 0 文档；正例 `a4f9766`：README+API.md+DEPLOY.md 同提交 | 先补三个家（desktop / gate-server / scripts），再回填 #8 三处描述；此后改行为同提交改对应家 |
| 9 | **ADR 真实性/时序分叉**：admin 判定口径已改而 note 未更新；`5511e14` 携带文不对题的 proposed 条；该 proposed 已部分落地未拆条；10/45 note 日期早于落地日（最严重 +11 天） | P1 | note `implemented/feature/2026-10-03-admin-token-unmetered-gateway-quota.md:11` 写三选一；现行 `crates/gate-server/src/lib.rs:502-504` → `c.role == "admin"`（`5511e14` 改，未动 note）；`git show --stat 5511e14` 只带 `lib.rs` + `user.rs` + 该 proposed 条；`git diff` note 日期 vs `git log --diff-filter=A`：`delta==0: 34 / >0: 10` | 同提交改 note 为 role-only 并链入 proposed；`5511e14` 的 proposed 条拆出独立提交或标注「仅覆盖 admin 收紧」；10 条 note 日期改实际落地日 |
| 10 | **supersession 形同虚设**：同主题三代 ADR 各自现在时并存、互相零链入 | P1 | toast 三连（09-26 深灰 → 09-27 浅灰 → 10-07 毛玻璃）`grep -nE 'supersed\|取代\|参见'` → 全 0；全树交叉链入仅 2 条（45 条 note）；`446d557` 复制 + `26f358b` 次日删旧，中间出现 proposed/implemented 双份并存 | 10-07 条加 `Supersedes:`，旧条链回；迁移三件事（移动/改 Status/改写标题）必须同一次变更；写入 write-adr supersession 分支 |
| 11 | **流程资产缺元数据门禁；唯一有外部消费者的 `pproxy-ops` 欠五要素、触发词缺滚动升级/签名，且安装副本落后仓库 12 行（实体复制非壳引用）** | P1 | `find .meta/gates -type f` → 仅 check-adr*；`grep -c '^## 校准样例' .agents/skills/pproxy-ops/SKILL.md` → 0；`diff -r /home/dm/.agents/skills/pproxy-ops .agents/skills/pproxy-ops` → 仓库侧多 12 行（`### 2.4 集群零停机滚动升级`）；命令实况 `pproxy cluster --help` 有 upgrade | 补 `.meta/gates/skill-meta.sh`（负样本 spec）；pproxy-ops 按五要素重写并补触发词；安装副本改符号链接 |
| 12 | **负空间未启用**：`archived/` 零归档、无哈希封印；`rejected/` 目录不存在，audit 13/14 的缺项（被否选项留痕）无动作文案 | P2 | `find .agents/notes/archived -name '*.md'` → 仅 AGENTS.md；`grep -rln 'sha256sum.*archived' .meta/ .agents/skills/` → 空；`ls -d .agents/notes/rejected` → No such file；`ponygo audit` 缺口段为空 | 落 `process/*archive-seal*` note 写归档与封印命令；建 `rejected/` 并把已否决方案从 Alternatives 迁入 |
| 13 | **卫生与索引**：测试运行产物 4.3MB/67 文件入库（含 3.6MB 日志）；ROADMAP 过期（README 写 M1-M6，磁盘已有 m7）；6 份 crate README 是 9–15 行空壳；`meta.yaml` 未声明 `docs-tier: on`；宪法停止线仍是模板文案 | P2 | `git ls-files .agents/tests \| wc -l` → 67；最大 `gateway-logs-24h.txt` 3.6MB；`ls docs/product/specs` 含 m7；`wc -l crates/*/README.md` → 15/9/9/9/9/9；`cat .meta/meta.yaml` → 仅 `level: 2` | `git rm --cached` 日志/CSV/TXT 并加 `.gitignore`；ROADMAP 改当前态表 + 索引同步；crate README 补四段；`docs-tier: on`；停止线改写为已填态 |
| 14 | **工程门禁「两个家」**：真漂移门住在 `scripts/`（release-gate / check-egress-parity / check-update-feed），不在 `.meta/gates`、只在 tag 流跑；`upgrade --check` 的漂移信号无消费者 | P2 | `find .meta/gates -type f` → 仅 check-adr*；`grep -rn 'upgrade --check' .github/workflows/` → 0；`desktop-release.yml:51,160` 为唯一接线点 | 登记进 `.meta/gates/README.md`「承诺 → 命令」表；PR 触发流加一条轻量 invariant 步；`ponygo upgrade --check` 进 CI |

**正例（保留，不改造）**：家载体选对了（`docs/AGENTS.md` 是文档标准家，根 AGENTS.md/CLAUDE.md
第 4 条 standing order 链到，`diff` 为 0）；`a4f9766`/`d0a661b` 是同提交正例；敏感配置
（`config.json`/`.secrets.env`/`.pproxy.env`/*.pid/*.log）全部 ignored 且未跟踪；`.rgignore`
已隔离 `archived/`；验收套件 31 PASS 且含负控制/变异灵敏度自检（非假绿）。

## Decision

采纳 A1–A9 路线图（每条小到一次提交可完成、按杠杆排序、P0 两种不与门禁升级混提交），并按
「每条一个判定本体 + 一条非零退出命令」落地。下表是**现行落点**（判定本体在仓库里的位置）与
**今日可复跑的判据**——承诺不落在散文里，落在命令里：

| 动作 | 内容 | 实际落点（判定本体） | 复跑判据（期望） |
|---|---|---|---|
| A1 | 凭据不进版本库：脱敏 + secret 扫描门（本地 + CI） | `.meta/gates/secret-scan.sh`（唯一判定本体）+ 负样本 `.meta/gates/secret-scan.spec.sh`；调用点 `.githooks/pre-commit:24-25`、`.github/workflows/ponygo-governance.yml:22` | `bash .meta/gates/secret-scan.sh` → rc 0；`git grep -lE 'pony_admin_[0-9a-f]{8}\|sk-pony-[0-9a-f]{8}'` → 零命中（rc 1） |
| A2 | 修复与 note 同提交，`implemented` 可复现 | `b7f1387`（`fix(server): 显式拒绝 absolute-form 并统一 CONNECT 凭据校验`）同时带代码与两条 note；admin 判定口径在 note 内改为 role-only（`crates/gate-server/src/lib.rs:641-645`） | `git log -S'absolute_form' -- crates/server/src/gateway.rs` → 命中 `b7f1387`；治理套件 G7 PASS |
| A3 | `ponygo init` 装配哨兵块 + 门禁资产 + 适配任务书，2.4 转绿 | `.githooks/pre-commit:2-6`（哨兵对）+ `.meta/gates/{adr-verify,ci-verify,archive-seal,archive-verify,no-residue,backlog-gate}.sh` + `.meta/adaptation/alt-governance.md` | `ponygo status` → rc 0、`治理门接线：已接线`、`级自洽验证：通过`；`ponygo upgrade --check` → rc 0、`零漂移` |
| A4 | write-adr 升到含 1.10 的当前模板，并接进 CI + 负样本 spec | `.agents/skills/write-adr/verify-note.sh`（1.10 硬判据实现，见其头部 13-17 行）；CI 经 `.meta/gates/ci-verify.sh:49-50` 调用；负样本 `.meta/gates/adr-verify.spec.sh`（缺 `## Alternatives considered` 必须被拒，夹具见 spec §8/§16） | `grep -c Alternatives .agents/skills/write-adr/verify-note.sh` → 5；`bash .meta/gates/adr-verify.spec.sh` → rc 0 |
| A5 | 适配决策入册（双轨 + 旧 ADR 冻结口径），WARN 静默 | `.agents/notes/implemented/process/2026-10-30-alt-governance-dual-track.md` + `.meta/adaptation/alt-governance.md` | `ponygo status` 不再打印「适配决策未登记」；治理套件 G6 PASS |
| A6 | 文档收敛到单一 tier 家 + 补缺的家 + 环境变量契约回填 | `docs/AGENTS.md`（tier 真源，零「待填/待补」）、`desktop/README.md`、`README.md:206`（`PPROXY_YOUTUBE_PROXY`）、`.meta/meta.yaml` 显式 `docs-tier: on` | 治理套件 G8 PASS；`grep -n PPROXY_YOUTUBE_PROXY README.md` → 命中 |
| A7 | supersession 互链 + 建 `rejected/` + 归档封印 | `implemented/architecture/2026-10-07-desktop-toast-borderless-frosted.md` 的 `Supersedes:`；`.agents/notes/rejected/process/2026-10-30-history-rewrite-rejected.md`；`.meta/gates/archive-seal.sh` + `.agents/notes/archived/SHA256SUMS` | 治理套件 G10 / G14 PASS |
| A8 | 卫生：产物移出版本库 + `.gitignore` + tier 开关 | `git rm --cached` + `.gitignore`；`meta.yaml` 的 `docs-tier: on` | `git ls-files .agents/tests` → 25 条、`.log/.txt/.csv` 0 条；治理套件 G9 PASS |
| A9 | 承诺表 + PR 流轻量 invariant 步 | `.meta/gates/instance-commitments.md`（实例承诺 → 命令，**不是** `.meta/gates/README.md`，理由见 `## Consequences` 偏移②）+ `.github/workflows/ponygo-governance.yml:28`（`scripts/check-egress-parity.sh`）；`upgrade --check` 进 CI 一项以 G5 契约改口径收口（偏移①） | 治理套件 G12 / G5 PASS |

### 档位判断（现行）

`level: 2` 现在**自洽**：2.4 已接线、1.1–1.8 PASS（1.9b/2.6 因未声明 `ai-surface` 为 SKIPPED，
不计 PASS）。**不升 L3**：L3 要求 skill 元数据门禁与触发路由，现状仍缺（见
`## Consequences`「未纳入本路线图的建议」）。L4（归档冻结/哈希封印）与 L5（运行时不变量）已有
零件（`.meta/gates/archive-seal.sh`、`scripts/release-gate.sh`）但未装配，按 maturity-ladder §3
的触发条件（语料腐烂速度、多服务不变量）再选装——不抢跑下一档。

## Alternatives considered

- **维持现状（合法终点）**：不成立——#1 是凭据事实错误、#2 是 `implemented` 不可复现，
  都不属于「机制已在工作、框架不产生新收益」，不能按「维持现状」放行。
- **只降级声明（`level: 1`）不补门禁**：成本最低、口径诚实（与采纳 ADR 自认一致）；
  但放弃 2.4 的收益，且 #1/#2 仍须单独修。若两周内不排 A3/A4，选此路并留降级 decision。
- **升 L3（流程资产）**：否决——L2 未自洽时升档，等于用新形状掩盖旧缺口。
- **只补文档不动门禁**：否决——same-commit 欠账已有实证（#8），没有非零退出命令看守就会复发。
- **立即重写全部门禁（自建 CI 网关）**：否决——P7 时间经济性，秒级用本地门、分钟级交给
  A4/A5 薄适配器，不重复跑同一检查。

## Consequences

**迁移来源**：本条原为 `.agents/notes/proposed/process/2026-10-30-governance-audit.md`（提案态），
A1–A9 落地后按 write-adr 步骤 8 的四件事同一次变更迁移：移动路径 / 改 `Status:` / `## Proposal`
改写为现在时 `## Decision` / 原「验收标准」与「风险」折叠进本节。正文重写幅度超过 git 的重命名
相似度阈值（`git diff -M` 报 add+delete 而非 rename），故在文内显式登记迁移来源，保住可追溯性。

### 机械判据的当前实测状态（2026-11-12）

原 `## 验收标准` 的 8 条判据，逐条换成今天可复跑的命令与实测输出：

| # | 原判据 | 实测（2026-11-12） |
|---|---|---|
| 1 | `ponygo status` exit 0（含 2.4 PASS），`upgrade --check` 缺口 0 | `ponygo status` **rc 0**（`治理门接线：已接线`、`级自洽验证：通过`）；`ponygo upgrade --check` **rc 0**（`宇宙内十七文件与模板基准一致，零漂移`） |
| 2 | 泄漏凭据零命中 + token 已轮换 | `git grep -lE …` **rc 1（零命中）**；`bash .meta/gates/secret-scan.sh` **rc 0**（`索引 542 个文件 + HEAD 树`未见凭据形状）。**轮换部分不可机械验证**，见下「残余风险」 |
| 3 | `git log -S'absolute_form'` 非空，两条 implemented note 在 HEAD 可复现 | 命中 `b7f1387`；治理套件 `G7-IMPLEMENTED-REPRODUCIBLE` PASS |
| 4 | PR 流跑 `verify-note.sh` + `upgrade --check` | 闭包成立：workflow → `.meta/gates/ci-verify.sh:49-50` → `verify-note.sh`；`upgrade --check` 一项**改口径**（偏移①）。`G5-REMOTE-GATE` PASS |
| 5 | verify-note 含 1.10；缺备选的 note 被判非零 | `grep -c Alternatives .agents/skills/write-adr/verify-note.sh` → **5**；`bash .meta/gates/adr-verify.spec.sh` **rc 0**（含「缺 `## Alternatives considered` 拒绝」断言，spec §8 第 295 行）；`G3-NOTE-1.10` PASS |
| 6 | 适配 WARN 消失 + `.meta/adaptation/alt-governance.md` 在场 | `ponygo status` 无「适配决策未登记」；任务书在场（1847 B）；`G6-ADAPTATION` PASS |
| 7 | `docs/AGENTS.md` 无「待填/待补」；`desktop/README.md` 在场；README 含 `PPROXY_YOUTUBE_PROXY` | `grep -cE '待填\|待补' docs/AGENTS.md` → **0**；`desktop/README.md` 在场；`README.md:206` 命中；`G8-DOCS` PASS |
| 8 | `.agents/tests` 不含 logs/txt/csv；`rejected/` 存在；archived 有封印 | `git ls-files .agents/tests \| wc -l` → **25**，后缀 `.log/.txt/.csv` → **0**；`.agents/notes/rejected/process/` 有 note；`.agents/notes/archived/SHA256SUMS` + `archive-seal.sh`（`G9-HYGIENE` / `G14-ARCHIVE-SEAL` PASS） |

治理套件整体：`bash tests/governance/run.sh` → **rc 0，`PASS=17 FAIL=0`**。

### 两处口径偏移（如实登记，勿当作"已按原文落地"）

① **A9-3「`ponygo upgrade --check` 进 CI」未按字面实施，由 G5 契约改口径收口**：G5 判定从
"workflow 字面含 `upgrade --check`"改为**传递闭包** workflow → `ci-verify.sh` → `verify-note.sh`
（冻结契约见 `tests/governance/cases/g5_remote_gate.sh:4-7`，修订记录见 `tests/governance/README.md:28-29`：
「薄适配器设计：ponygo 二进制不进 CI，故不再要求 workflow 字面出现 `upgrade --check`」）。
实测 `grep -rn 'upgrade --check' .github/workflows/` → **0 命中**。**限定条件（诚实条款）**：CI 里
并非完全没有 ponygo 进程——workflow 第 32-42 行按**钉死 commit** `7055ced1` 装一个 oracle，只服务
`tests/governance/run.sh`（G4/G6 需要 `ponygo status` 文案），不跑漂移检查。故准确说法是
「PR 流不以 `upgrade --check` 作为门禁」，而非「CI 不装 ponygo 二进制」。

② **A9-1「登记进 `.meta/gates/README.md` 承诺表」载体内改**：该 README 是**框架模板文件**
（与框架仓逐字节一致、进 `ponygo upgrade --check` 漂移比对），实例承诺写进去会立刻制造漂移，
且 `ponygo init` 重跑会覆盖。故实例承诺 → 命令落在 `.meta/gates/instance-commitments.md`
（自述见该文件第 3-5 行），并由 `G12-COMMITMENTS` 冻结判据：四条 `secret-scan / release-gate /
check-egress-parity / check-update-feed` 承诺各须配一条命令，且 alt-governance note 须链到该表
（`tests/governance/cases/g12_commitments.sh:10-11`）。

### 残余风险

- **泄漏 token 是否真已轮换无法机械验证**：`secret-scan` 只证明「仓内不再有该形状的字符串」，
  不等于「旧 token 已失效」。轮换须先禁用旧值、再替换、留回滚窗口——这是本 note **唯一 100% 靠
  review** 的验收项，机器到此为止。
- **本地门可被 `--no-verify` 跳过**：pre-commit 是秒级便捷层，真兜底在 CI（
  `ponygo-governance.yml` 跑 `secret-scan` / `archive-seal --check` / `check-egress-parity` /
  `ci-verify.sh` / 治理套件）。云端全树门与本地门判据同源（P9 一份判据一份实现），但本地
  `--no-verify` 仍能跳过 → 绕过者的唯一代价是 CI 变红。
- **doc-sync / 词数预算仍未实现**：tier 放置语义靠 review（`.meta/gates/instance-commitments.md`
  §3 已把这条登记为「靠 review」而非门禁，避免 review 项伪装成门禁项）。
- **新增 CI 步骤有分钟级成本（P7）**：只放「全树一次」的检查，不复制本地秒级检查。
- **审查口径外（原文复述，禁止外推）**：文档分层漂移全量（tier 放置语义 / slop 清单 / 词数预算）、
  供应链安全（audit/SBOM/许可证）、可观测性、发布与版本治理、开发环境可复现、API 契约演进治理；
  语义真实性终审归 review（P6 形意分离）。附带盲区：分支保护是否强制 CI 未查 API 校验。

### 未纳入本路线图的建议（保持未落地——不是"已完成"）

- **#11 skill 元数据门**：`.meta/gates/skill-meta.sh` 仍不存在，`.agents/skills/pproxy-ops/SKILL.md`
  的 `## 校准样例` 计数仍为 0。这正是「不升 L3」继续成立的理由，不是待办清单上的勾。
- **#12 负空间**已由另一条决策落地（`.agents/notes/implemented/process/2026-10-30-archive-seal.md`
  + `.meta/gates/archive-seal.sh`），本条不重复声明。
- **#9 的 10 条 note 日期回填、#8 的三处 same-commit 描述回填**属逐条文案工作，不在 A1–A9 内；
  同类欠账仍可能复发（门禁只管格式与接线，管不到"这次改动该不该有文档"）。
