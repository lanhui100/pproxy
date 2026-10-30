# tests/governance — 治理修复独立黑盒验收（L2-T）

> 归属：独立 Test Agent（L2-T）。写域仅 `tests/governance/**`。
> 本套件与 `tests/acceptance/`（业务出海验收）**相互独立**：后者验产品行为，本套件验治理层（密钥、决策记录门禁、
> 本地/远端接线、文档落地、仓库卫生）。
> 合同唯一依据：Lead 冻结的契约矩阵 —— Wave-1 `G0..G9`（§1，冻结于 `e0acfe3`，**断言不可变更**）
> 与 Wave-2 `G10..G16` 追加（§1.1，Lead 授权契约演进，**只追加不改 Wave-1**）。
> 冻结后本目录只读，任何改动须 Lead 明确指令。

---

## 1. 契约矩阵（冻结）

| # | 用例 id | 判定命令（原文即契约） | 用例函数 | 红相(HEAD b7f1387) |
|:-:|:--|:--|:--|:-:|
| 1 | `G0-SELFTEST` | 构造假"违规样本"调用探测函数，**必须输出非零**（防空转假绿） | `case_G0_SELFTEST` | PASS |
| 2 | `G1-NO-TRACKED-SECRETS` | `git grep -lE 'pony_admin_[0-9a-f]{16,}\|sk-pony-[0-9a-f]{16,}'` 无输出 → PASS | `case_G1_NO_TRACKED_SECRETS` | FAIL |
| 3 | `G2-SECRET-GATE` | `.meta/gates/secret-scan.sh` 存在且 `bash .meta/gates/secret-scan.sh` 在干净仓库 exit 0；`bash .meta/gates/secret-scan.spec.sh` exit 0（负样本必须被拒） | `case_G2_SECRET_GATE` | FAIL |
| 4 | `G3-NOTE-1.10` | 临时 note 缺 `## Alternatives considered` → `bash .agents/skills/write-adr/verify-note.sh <tmp>` exit 非零；合法 note exit 0 | `case_G3_NOTE_110` | FAIL |
| 5 | `G4-SENTINEL-LOCAL` | `.githooks/pre-commit` 同时含 `>>> ponygo-gate: adr-verify v1 >>>` 与 `<<< ponygo-gate: adr-verify v1 <<<`；`.meta/gates/adr-verify.sh` 存在；`ponygo status` exit 0 | `case_G4_SENTINEL_LOCAL` | FAIL |
| 6 | `G5-REMOTE-GATE` | `.github/workflows/ponygo-governance.yml` 存在且调用 `.meta/gates/ci-verify.sh`；且 `.meta/gates/ci-verify.sh` 内含 `verify-note.sh` 调用（**传递闭包**） | `case_G5_REMOTE_GATE` | FAIL |
| 7 | `G6-ADAPTATION` | 存在 `.agents/notes/**/process/*alt-governance*.md`，且 `ponygo status` 输出不含「适配决策未登记」 | `case_G6_ADAPTATION` | FAIL |
| 8 | `G7-IMPLEMENTED-REPRODUCIBLE` | `git log -S'absolute_form' -- crates/server/src/gateway.rs` 非空，且 `git show HEAD:crates/server/src/gateway.rs \| grep -q forward_proxy_unsupported` | `case_G7_IMPLEMENTED_REPRODUCIBLE` | PASS |
| 9 | `G8-DOCS` | `desktop/README.md` 存在；`grep -q PPROXY_YOUTUBE_PROXY README.md`；`grep -nE '待填\|待补' docs/AGENTS.md` 为空 | `case_G8_DOCS` | FAIL |
| 10 | `G9-HYGIENE` | `git ls-files '.agents/tests/**' \| grep -E '\.(log\|txt\|csv)$'` 为空（`PLAN.md` 除外可豁免） | `case_G9_HYGIENE` | FAIL |

### 契约修订记录（Lead，冻结前）
1. **G5** 由"workflow 字面含 `upgrade --check`"改为**传递闭包**：workflow → `.meta/gates/ci-verify.sh` → `verify-note.sh`。
   （薄适配器设计：ponygo 二进制不进 CI，故不再要求 workflow 字面出现 `upgrade --check`。）
2. **G7 基线前移**：Lead 已提交 `b7f1387`，HEAD 的 `gateway.rs` 应含 `forward_proxy_unsupported`；判定式不变。
3. **G3 夹具位置**：临时 note 必须落在仓库外（`$TMPROOT`，如 `/tmp`）并**显式传相对路径参数**调用 `verify-note.sh`
   —— 带参模式只校验给定文件；无参即整树模式，会额外判 1.9 并扫描仓库全部决策记录，与本用例目标不符。

---

## 1.1 Wave-2 追加矩阵（G10..G16，Lead 授权契约演进）

> 纪律：**只追加**。Wave-1（G0..G9）断言一行未改；`REQUIRED_CASES` 与 `CASE_TABLE` 仅在尾部追加。
> Wave-2 的每个检测器都做过「合成绿相夹具 + 负相突变」双向验证（见 §3.2），确保既能变绿、又不会假绿。

| # | 用例 id | 判定命令（原文即契约） | 用例函数 | W2 红相(HEAD 464852b) |
|:-:|:--|:--|:--|:-:|
| 11 | `G10-SUPERSESSION` | `implemented/architecture/2026-10-07-desktop-toast-borderless-frosted.md` 含 `Supersedes:` 且指向 2026-09-26 / 2026-09-27 两条；两条旧 note 各含一行「被 …2026-10-07… 取代」 | `case_G10_SUPERSESSION` | FAIL |
| 12 | `G11-ROADMAP-INDEX` | `README.md` 文档索引行含 `M1-M7`（不再是 `M1-M6`）；`docs/product/ROADMAP.md` 含 `## M7` | `case_G11_ROADMAP_INDEX` | FAIL |
| 13 | `G12-COMMITMENTS` | `.meta/gates/instance-commitments.md` 存在，且含 `secret-scan`、`release-gate`、`check-egress-parity`、`check-update-feed` 四条「承诺→命令」行；alt-governance note 链到它 | `case_G12_COMMITMENTS` | FAIL |
| 14 | `G13-PREPUSH` | `.githooks/pre-push` 存在、可执行、`bash -n` 通过，且调用 `tests/governance/run.sh`（或等价 `bash .meta/gates/secret-scan.sh` + `verify-note`） | `case_G13_PREPUSH` | FAIL |
| 15 | `G14-ARCHIVE-SEAL` | `.meta/gates/archive-seal.sh` 支持 `--check`（存在 manifest 时 exit 0）与 `--write`；在临时副本里篡改 `archived/*.md` 后 `--check` 必须非零；manifest 落 `.agents/notes/archived/SHA256SUMS` | `case_G14_ARCHIVE_SEAL` | FAIL |
| 16 | `G15-HOOK-TIMEOUT` | `.githooks/pre-commit` 中 `cargo check` 被 `timeout` 包裹（`grep -q 'timeout'`） | `case_G15_HOOK_TIMEOUT` | FAIL |
| 17 | `G16-OBSERVE-DECISION` | 存在 `.agents/notes/implemented/process/*observe*` note，明确「可观测性四维暂不接」+ 理由 + 重估触发条件；`ponygo observe status` 仍为缺失（不强制 init） | `case_G16_OBSERVE_DECISION` | FAIL |

### Wave-2 判据的容错边界（只放宽书写形态，不放宽语义）
- **G10**：`supersedes` 大小写不敏感、允许列表/加粗/frontmatter 写法；但「新声明指向两条日期」与「旧 note 同行含日期+取代」两向都必须成立（单向断链即 FAIL）。
- **G12**：命令载体允许折到命中行的**下一行**（markdown 表格/列表常见写法）；但"只有承诺名、全表无 `bash`/`*.sh`/`gates/`"仍判 FAIL（口号不是门禁）。
- **G14**：篡改/删除/重建全部在仓库外镜像副本（`$TMPROOT/g14`，含 `git init` + 基线提交）内进行，绝不写被验收仓库；断言链为 基线 `--check=0` → 篡改后 `≠0` → 删 manifest 后 `--write=0` → 重建后 `--check=0`（回环一致）。
- **G15**：`timeout` 允许同行、上一行续行、`timeout … cargo` 变量化写法；但 `cargo check` 裸调用（无任何 timeout 包裹证据）判 FAIL；且"删除 `cargo check` 段落"同样判 FAIL（契约以"包裹 cargo check"为前提）。
- **G16**：文档语义判据只能做**关键词组**判定（维度/暂不接/理由/重估触发条件四组，同义写法在 `cases/g16_observe_decision.sh` 头部枚举）；四组任一缺失即 FAIL 并回报命中情况（fail-closed，不做"看起来有道理"的自由裁量）。

---

## 2. 运行方式

```bash
# 全部命令 cwd=/home/dm/pproxy（脚本自身会 cd 到仓库根，可从任意目录调用）
bash tests/governance/run.sh                # 全量 + 必需用例完备校验（验收唯一合法用法）
bash tests/governance/run.sh '^G3'          # 仅跑匹配正则的用例（调试；不做完备校验，结果不可用于验收）
bash tests/governance/run.sh --list         # 列出冻结用例 id
GOV_KEEP=1 bash tests/governance/run.sh     # 保留临时夹具目录排障
```

环境变量：`GOV_CMD_T`（单命令超时，缺省 90s）、`GOV_TOTAL_T`（全局预算，缺省 600s）。
零外部网络依赖；`ponygo` 若装在 `~/.local/bin` 由脚本自行补 PATH（G4/G6 依赖它）。

### 输出协议
每行一条判定，机器可解析：

```
[PASS] <case-id> <实测 detail>
[FAIL] <case-id> <实测 detail>
[INFO] <纯观测，不参与判定>
末行：ACCEPT_EXIT=<0|1>    ← 进程退出码与之一致
```

---

## 3. 反假绿设计（本套件为何可信）

1. **G0-SELFTEST 是前置锚**：它把「假违规样本」喂给**本套件真实使用的探测器**
   （G1 的 git 密钥探测、G4 的哨兵配对探测、G3 的 note 判据探测），断言每个都返回**非零**；
   同时用「合规样本必须放行」做反向控制，防止探测器沦为"恒拒绝"空转。
   它还自检**记账层**（注入 rc=1 必须真的变成 `[FAIL]`）与 **MISSING-CASE 兜底**（未执行 id 必须被判未执行）。
   G0 红了 = 验收套件本身不可信，必须先修 harness 再谈被验收仓库。
2. **MISSING-CASE 兜底**：`REQUIRED_CASES` 中任何 id 未被执行 → 追加 `[FAIL] MISSING-CASE …` 并 `ACCEPT_EXIT=1`。
   漏跑、改名、`continue` 跳过都不得放行。
3. **无 SKIP**：套件不提供 skip 原语。无法验证（缺文件、命令超时、git 异常）一律 FAIL（fail-closed）。
4. **退出码分档**：如 `git grep` 的 `0/1/其它` 三档分别映射为 命中(违规)/干净(合规)/无法判定(违规)，
   禁止"一律当违规"或"一律当合规"的偷懒映射。
5. **禁止管道断言**：所有外部命令经 `run_cmd`/`run_cmd_in` 捕获退出码（管道会让 rc 变成下游命令的 rc，
   是历史假绿陷阱）；统计用 `grep -c` + here-string，避免 `grep -q` 触发 SIGPIPE 污染 `set -o pipefail`。
6. **零仓库污染**：全部夹具（假密钥仓库、影子决策记录树、坏哨兵样本、stub 校验器）落在 `mktemp -d` 的
   仓库外目录，`trap cleanup` 回收；假密钥在运行期拼接生成，源码中不存在可被 G1/G2 命中的完整密钥。

### 3.2 Wave-2 检测器的双向验证（只追加也要可信）

新增 7 条检测器在冻结前做过两轮实证（仓库外 `/tmp` 合成夹具，**不进仓库**）：

1. **变绿能力**（防"永久红"）：按契约实现一份参考版本（含 archive-seal 的 `--check/--write` 参考实现、
   承诺表、alt-governance 链接、pre-push、`timeout 180 cargo check`、observe 决策 note），
   以 `bash tests/governance/run.sh '^G1[0-6]'` 运行 → **7/7 PASS、ACCEPT_EXIT=0**。
2. **假绿能力**（防"永远绿"）：对参考版本逐条注入突变 → 全部如实 FAIL：
   - seal 换成 `exit 0` 空转 → G14「篡改后 --check 仍 exit 0（封条形同虚设）」
   - pre-push 去可执行位 → G13「不可执行（mode=664）」
   - pre-commit 去掉 `timeout` → G15「未出现 timeout」
   - 承诺表删 `check-update-feed` 行 → G12「承诺→命令行不全」
   - 新 note 删 `Supersedes` → G10「缺 Supersedes 字段」
   - 旧 note 删取代行 → G10「缺『被 …2026-10-07… 取代』行（链断一半）」
   - observe note 删「重估触发条件」段 → G16「缺重估触发条件」

---

## 4. 文件清单

| 文件 | 职责 |
|:--|:--|
| `run.sh` | 编排：REQUIRED_CASES / CASE_TABLE、预算与超时、fail-closed 循环、summary + `ACCEPT_EXIT` |
| `lib.sh` | 公共原语：`record`/`executed`/`run_cmd`/`run_cmd_in`/`run_probe`/文本折叠 |
| `cases/g0_selftest.sh` | G0 自身可信性锚（探测器敏感性 + 记账层 + MISSING-CASE 自检） |
| `cases/g1_no_tracked_secrets.sh` | G1（含 `probe_git_no_tracked_secret`，被 G0 复用） |
| `cases/g2_secret_gate.sh` | G2 |
| `cases/g3_note_110.sh` | G3（含 `probe_note_section_enforced`，被 G0 复用） |
| `cases/g4_sentinel_local.sh` | G4（含 `probe_sentinel_pair`，被 G0 复用） |
| `cases/g5_remote_gate.sh` | G5 传递闭包 |
| `cases/g6_adaptation.sh` | G6 |
| `cases/g7_implemented_reproducible.sh` | G7 |
| `cases/g8_docs.sh` | G8 |
| `cases/g9_hygiene.sh` | G9 |
| `cases/g10_supersession.sh` | G10 决策取代双向链 |
| `cases/g11_roadmap_index.sh` | G11 索引与里程碑真源一致 |
| `cases/g12_commitments.sh` | G12 承诺→命令映射表 + 决策链接 |
| `cases/g13_prepush.sh` | G13 push 前本地拦截层 |
| `cases/g14_archive_seal.sh` | G14 归档封条（镜像副本内篡改/回环） |
| `cases/g15_hook_timeout.sh` | G15 pre-commit 的 cargo check 超时保护 |
| `cases/g16_observe_decision.sh` | G16 可观测性四维「暂不接」决策 |

---

## 5. 红相快照（修复前基线，原始输出）

命令：`cd /home/dm/pproxy && bash tests/governance/run.sh`
仓库状态：`HEAD=b7f1387`（Lead 在途批次已提交）、branch=`master`、`ELAPSED=1s`、进程退出码 `1`。

```
=== GOVERNANCE ACCEPT RUN ===
[INFO] REPO_ROOT=/home/dm/pproxy (cwd=/home/dm/pproxy)
[INFO] HEAD=b7f1387 branch=master
[INFO] TMPROOT=/tmp/pproxy-gov.OWKwnh CMD_T=90s TOTAL_T=600s bash=5.2.21(1)-release
[INFO] ponygo=/home/dm/.local/bin/ponygo
[PASS] G0-SELFTEST 自检 8/8 通过：探测器对假违规样本均返回非零、对合规样本返回 0；记账层与 MISSING-CASE 兜底无假绿
[FAIL] G1-NO-TRACKED-SECRETS git grep -lE 命中 5 个追踪文件（期望 0）：.agents/tests/stability-2026-09-29/PLAN.md | .agents/tests/stability-2026-09-29/ponyllm/inference.py | .agents/tests/stability-2026-09-29/ponyllm/logs/gateway-logs-24h.txt | .agents/tests/stability-2026-09-29/ponyllm/summary.md | .agents/tests/stability-2026-09-29/reverse/control.sh
[FAIL] G2-SECRET-GATE 缺 .meta/gates/secret-scan.sh：密钥扫描门禁不存在（矩阵 G2 前置件）
[FAIL] G3-NOTE-1.10 缺「## Alternatives considered」的 note 未被拒（rc=0，1.10 判据缺失）；校验器输出：verify-note: 全部通过（机械影子 1.1–1.9；真实性/分类靠 review）
[FAIL] G4-SENTINEL-LOCAL /home/dm/pproxy/.githooks/pre-commit 哨兵缺失：begin=缺 end=缺（期望两行都在）
[FAIL] G5-REMOTE-GATE 缺 .github/workflows/ponygo-governance.yml（远端层适配器不存在）
[FAIL] G6-ADAPTATION 缺 .agents/notes/**/process/*alt-governance*.md（适配决策未落盘）
[PASS] G7-IMPLEMENTED-REPRODUCIBLE git log -S 命中 1 个提交（最新 b7f1387 fix(server): 显式拒绝 absolute-form 并统一 CONNECT 凭据校验）；HEAD:crates/server/src/gateway.rs 含 forward_proxy_unsupported 5 处
[FAIL] G8-DOCS 缺 desktop/README.md（桌面端文档无家）
[FAIL] G9-HYGIENE 追踪的测试产物 42 个（期望 0；豁免 PLAN.md）：.agents/tests/stability-2026-09-29/forward/START_TIME.txt | .agents/tests/stability-2026-09-29/forward/journal-errors.txt | .agents/tests/stability-2026-09-29/forward/probe_cloudcode.csv | .agents/tests/stability-2026-09-29/forward/probe_github.csv | .agents/tests/stability-2026-09-29/forward/probe_google204.csv …(+37)

=== GOVERNANCE ACCEPT SUMMARY ===
REPO=/home/dm/pproxy
HEAD=b7f1387
ELAPSED=1s CMD_T=90s TOTAL_T=600s
PASS=2 FAIL=8
FAILED_IDS=G1-NO-TRACKED-SECRETS G2-SECRET-GATE G3-NOTE-1.10 G4-SENTINEL-LOCAL G5-REMOTE-GATE G6-ADAPTATION G8-DOCS G9-HYGIENE
ACCEPT_EXIT=1
```

### 红相失败清单与人话注记（给修复方）

| 用例 | 根因（实测） | 修复方向（不构成契约，仅定位） |
|:--|:--|:--|
| G1 | 5 个**已追踪**文件含真实密钥：`.agents/tests/stability-2026-09-29/{PLAN.md,ponyllm/inference.py,ponyllm/logs/gateway-logs-24h.txt,ponyllm/summary.md,reverse/control.sh}` | 移出版本库或脱敏后重新入库；脱敏后 G1 自然转绿 |
| G2 | `.meta/gates/secret-scan.sh`、`secret-scan.spec.sh` 均不存在 | 新建门禁 + 负样本 spec（spec 必须真的调用门禁并有拒绝分支） |
| G3 | `verify-note.sh` 尚无 1.10 判据：缺 `## Alternatives considered` 的样本仍 `exit 0` | 在带参/整树两模式都补 1.10（本用例只验行为，不约束实现） |
| G4 | `.githooks/pre-commit` 无 `ponygo-gate: adr-verify v1` 哨兵对；`.meta/gates/adr-verify.sh` 不存在；`ponygo status` rc=1（`2.4 本地层未接 ADR 门`） | 按 `ponygo init` 规范块接线；`ponygo status` 需回到自洽（exit 0） |
| G5 | `.github/workflows/ponygo-governance.yml` 不存在（`ci-verify.sh` 亦不存在） | 建薄适配器 workflow + `.meta/gates/ci-verify.sh`（内含 `verify-note.sh`） |
| G6 | 仅有 `.agents/notes/proposed/process/2026-10-30-governance-audit.md`，**文件名不含 `alt-governance`**；`ponygo status` 仍告警「适配决策未登记」 | 落一条命名/位置合规的适配决策，并让 CLI 该告警消失 |
| G8 | `desktop/README.md` 缺失；`README.md` 无 `PPROXY_YOUTUBE_PROXY`；`docs/AGENTS.md` 仍有 8 处「待填/待补」 | 补桌面端文档、补环境变量契约、清零占位 |
| G9 | `.agents/tests/**` 下 **42 个** `.log/.txt/.csv` 被追踪 | `git rm --cached` + `.gitignore`，产物移出版本库 |
| G7 | 已绿（`b7f1387` 同时满足"可回放提交"与"HEAD 含实现"） | — |
| G0 | 套件自身健康（8/8 自检通过） | — |

---

## 5.2 Wave-2 红相快照（G10..G16 追加后，原始输出）

命令：`cd /home/dm/pproxy && bash tests/governance/run.sh`
仓库状态：`HEAD=464852b`（Wave-1 收口后）、branch=`master`、`ELAPSED=3s`、进程退出码 `1`。
要点：Wave-1 十条**全部维持 PASS**（追加未引入回归），Wave-2 七条如实报红。

```
[PASS] G0-SELFTEST 自检 8/8 通过：探测器对假违规样本均返回非零、对合规样本返回 0；记账层与 MISSING-CASE 兜底无假绿
[PASS] G1-NO-TRACKED-SECRETS git grep -lE 无输出：追踪文件 0 命中（repo=/home/dm/pproxy）
[PASS] G2-SECRET-GATE 门禁 rc=0（干净仓库放行）；spec rc=0（负样本被拒；spec 引用门禁 3 处/拒绝分支 5 处）
[PASS] G3-NOTE-1.10 1.10 生效：缺节 note rc=1（被拒）；合法 note rc=0（放行）
[PASS] G4-SENTINEL-LOCAL 哨兵配对（.githooks/pre-commit）+ .meta/gates/adr-verify.sh 存在 + ponygo status rc=0（关键行：治理级：L2 级自洽验证：通过（L0-L2 机械判据全部 PASS） ）
[PASS] G5-REMOTE-GATE 传递闭包成立：.github/workflows/ponygo-governance.yml → .meta/gates/ci-verify.sh → verify-note.sh（引用行数 3 / 3）
[PASS] G6-ADAPTATION 适配决策已落盘（.agents/notes/implemented/process/2026-10-30-alt-governance-dual-track.md）；ponygo status 无「适配决策未登记」（status rc=0）
[PASS] G7-IMPLEMENTED-REPRODUCIBLE git log -S 命中 1 个提交（最新 b7f1387 …）；HEAD:crates/server/src/gateway.rs 含 forward_proxy_unsupported 5 处
[PASS] G8-DOCS desktop/README.md 存在；README.md 含 PPROXY_YOUTUBE_PROXY 1 处；docs/AGENTS.md 占位 0 处
[PASS] G9-HYGIENE .agents/tests 追踪文件 25 个，其中 .log/.txt/.csv 产物 0 个（豁免 PLAN.md）
[FAIL] G10-SUPERSESSION .agents/notes/implemented/architecture/2026-10-07-desktop-toast-borderless-frosted.md 缺 Supersedes 字段（未声明它取代了哪些旧决策）
[FAIL] G11-ROADMAP-INDEX README.md 索引行未升级到 M1-M7：232:| [docs/product/ROADMAP.md](docs/product/ROADMAP.md) | 里程碑 M1-M6 |
[FAIL] G12-COMMITMENTS 缺 .meta/gates/instance-commitments.md（承诺→命令映射表不存在）
[FAIL] G13-PREPUSH 缺 .githooks/pre-push（push 前无本地拦截层）
[FAIL] G14-ARCHIVE-SEAL 缺 .meta/gates/archive-seal.sh（归档封条门不存在，archived 决策可被静默改写）
[FAIL] G15-HOOK-TIMEOUT .githooks/pre-commit 未出现 timeout（cargo check 无超时保护，钩子可无限挂起）
[FAIL] G16-OBSERVE-DECISION 缺 .agents/notes/implemented/process/*observe* 决策 note（可观测性四维接不接没有记录）
PASS=10 FAIL=7
ACCEPT_EXIT=1
```

### Wave-2 红相失败清单（给修复方）

| 用例 | 根因（实测） | 修复方向（不构成契约，仅定位） |
|:--|:--|:--|
| G10 | `2026-10-07-desktop-toast-borderless-frosted.md` 全文无 `Supersedes`；两条旧 note（`2026-09-26-desktop-toast-style-redesign.md`、`2026-09-27-desktop-toast-light-gray-redesign.md`）无「被 …2026-10-07… 取代」行 | 新 note 加 `Supersedes: 2026-09-26-…, 2026-09-27-…`；两条旧 note 各加一行回指 |
| G11 | `README.md:232` 仍为「里程碑 M1-M6」（`docs/product/ROADMAP.md` 的 `## M7` 已在位） | 索引行升级为 `M1-M7`，勿留 `M1-M6` 残句 |
| G12 | `.meta/gates/instance-commitments.md` 不存在 | 建表：四条承诺各配可执行命令；并在 alt-governance note 中链接该表 |
| G13 | `.githooks/pre-push` 不存在 | 新增可执行 pre-push，调用 `tests/governance/run.sh`（或 secret-scan.sh + verify-note） |
| G14 | `.meta/gates/archive-seal.sh` 与 `.agents/notes/archived/SHA256SUMS` 均不存在 | 实现 `--check/--write` 并落 manifest；篡改须被检出 |
| G15 | `pre-commit` 第 25 行 `cargo check --tests --quiet --workspace` 为裸调用 | 用 `timeout <N> cargo check …` 包裹（保留 `cargo check` 段落） |
| G16 | 无 `implemented/process/*observe*` 决策 note（`ponygo observe status` 当前确为「声明: 缺失」，该半条已满足） | 落一条决策：四维暂不接 + 理由 + 重估触发条件；**不要**执行 `observe init` |

---

## 6. 冻结与只读纪律

- 冻结后本目录**只读**：实施者（Executor）不得修改 `run.sh` / `lib.sh` / `cases/**` / 本 README。
- 本项目采用**两段冻结**：
  - Wave-1：`e0acfe3 test(wave-1-governance): freeze red-phase governance acceptance tests`（G0..G9，内容指纹 `406f77b9…`）。
  - Wave-2：G10..G16 追加后由 Lead 重新锚定提交（`test(wave-2-governance): extend acceptance (G10-G16)`），
    锚点前本目录处于"追加中"状态；锚点后即只读。
- 冻结指纹（逐文件 `sha256sum` + 汇总）与冻结时 `HEAD` 由 Test Agent 只读记录后**随消息上报 Lead**，
  不写入本目录（避免"改文件即改指纹"的自证循环）。
- 绿相验收：仅当 Lead 通知修复完成，Test Agent 才运行全量 `bash tests/governance/run.sh`，
  回报**原始输出与进程退出码**，不做二次加工。
