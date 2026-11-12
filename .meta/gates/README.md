# gates/ —— 门禁（把承诺转成非零退出命令）

**框架交付资产 A1/A2/A4/A6/A7/A8/A9 常驻；实例自有门禁从 L2 起按需添加。**
升到 L2 时放你的第一个实例门禁，并同步 meta.yaml 的 level 改为 2——框架资产不需要你手动放，
`ponygo init` 已经落盘（见下表）。

## 本仓已接线的门禁（ADR 门 + 归档封印门 + 零残留门，A1–A9）

「交付即接线」：`ponygo init` 幂等装配下列资产（`.meta` 已存在也装配，见 plan-freeze §6.2）；
repo 模板文件与 `ponygo` 内 heredoc 双写，EOL 归一后逐字节一致。

| 资产 | 路径 | 角色 | 退出语义 |
|---|---|---|---|
| A1 | `.meta/gates/adr-verify.sh` | 暂存区 ADR 格式门（无参=暂存 ADR；带参=指定路径）；A6 `check` 在**任何跳过分支之前**早期 probe（见下「A6 的时序」） | 0 放行 / 非零拒绝 |
| A2 | `.meta/gates/adr-verify.spec.sh` | A1 的负样本执行判据（非法→拒、合法→放行、TOCTOU→拒） | 0 = 自检通过 |
| A3 | `.githooks/pre-commit`（或既有钩子内追加块） | 本地层挂载点 | 转发 A1 的退出码 |
| A4 | `.meta/gates/ci-verify.sh` | forge 无关全树门（verify-note 整树 + A2 + A6/A7 + A8/A9，四段） | 0 放行 / 非零拒绝 |
| A5 | `.github/workflows/ponygo-governance.yml` | GitHub 薄适配器（checkout + A4）；等价形态是主 `ci.yml` 内 `governance` job 直接跑 A4——只有同 workflow 的 job 才能被聚合门 `needs` 消费 | 同 A4 |
| A6 | `.meta/gates/archive-verify.sh` | 归档 SHA-256 封印门：`check`（默认，只读）/ `seal`（追加封印，互斥 + 原子替换），零旗标 | 0 通过 / 1 拒绝 |
| A7 | `.meta/gates/archive-verify.spec.sh` | A6 的负样本执行判据（篡改/缺条目/孤儿/重复/非追加 → 必须拒；合法样本 → 放行） | 0 = 自检通过 |
| A8 | `.meta/gates/no-residue.sh` | 零残留门（删除/收窄后的符号检索验收）：读清单 `.meta/gates/no-residue.manifest`（每行 `<pattern><TAB><scope>`），`rg --fixed-strings` 按字面检索（无 `rg` 退化 `grep -rIlF`）；缺清单/空清单 → 放行并打印跳过 | 0 放行（无命中 / 跳过）/ 1 命中、清单格式非法、未知参数 |
| A9 | `.meta/gates/no-residue.spec.sh` | A8 的负样本执行判据（命中→拒 / 无命中→放行 / 空清单→放行 / 未知参数→拒 / 特殊字符按字面） | 0 = 自检通过 |

**宇宙表只登记 A1/A2/A4/A6/A7/A8/A9**（不可变资产）：A3/A5 是实例可编辑文件（有人会追加自己的钩子逻辑、
改 CI 触发），塞进 `UPGRADE_UNIVERSE` 会让实例永久 drift 红。`scratch-hygiene.sh` 也不进宇宙表——它是
**本仓实例门**，不由 `init` 播种（见下节）。

### 实例卫生门 scratch-hygiene.sh（init 不播种，CI 单点接线 `ci.yml:47`）

常载命约 7「临时产物统一下沉」的机械看守，只拦三类可判违例（`.scratch/` 被 Git 跟踪 / 根 `.gitignore` 缺
`.scratch/` 声明 / 发布安装链引用 `.scratch`）；「平铺在源码旁」是 review 项，门不覆盖。接线 = **CI 单点**：
`.github/workflows/ci.yml:47` 的 `governance` job 跑 `bash .meta/gates/scratch-hygiene.sh && bash .meta/gates/scratch-hygiene.spec.sh`；
本地 pre-commit 不跑它，秒级预算留给 A1。

### 承诺 → 非零退出命令

| 承诺（maturity-ladder §5） | 命令 | 命中判据 |
|---|---|---|
| 落了 ADR 就必须过机械格式门（暂存区） | `bash .meta/gates/adr-verify.sh` | 1.5–1.8 + 1.10（逐文件；1.9 只在整树模式判，带参模式不判） |
| 暂存字节 = 校验字节（不许"暂存后偷改"） | 同上（内建 R6 比对） | R6 TOCTOU |
| 重命名 ADR 不绕过门 | 同上（`--diff-filter=ACMR`） | R7 |
| 全树 ADR 格式常绿 | `bash .agents/skills/write-adr/verify-note.sh` | 1.1–1.9a + 1.10（1.9b/1.9c 不在本门面，见「1.9 的分工」） |
| 门自己会拒绝非法样本 | `bash .meta/gates/adr-verify.spec.sh` | A2 |
| 归档件冻结（改一字节即拒） | `bash .meta/gates/archive-verify.sh check`（无参等价） | A6（manifest ↔ `archived/**/*.md` 双向一致） |
| 归档必须显式封印 | `bash .meta/gates/archive-verify.sh seal` | A6（append-only 追加；新条目要求 `Status:` 首词 ∈ {`implemented`, `archived`}） |
| 封印门自己会拒绝非法样本 | `bash .meta/gates/archive-verify.spec.sh` | A7（篡改/缺条目/孤儿/重复/非追加） |
| 删除/收窄后无残留符号 | `bash .meta/gates/no-residue.sh` | A8（清单 pattern 零命中；缺/空清单放行） |
| 零残留门自己会拒绝非法样本 | `bash .meta/gates/no-residue.spec.sh` | A9（命中→拒 / 无命中→放行 / 空清单→放行 / 未知参数→拒） |
| 远端层全树兜底 | `bash .meta/gates/ci-verify.sh` | A4（= `verify-note.sh` 整树 + A2 + A6/A7 + A8/A9，四段） |

### 归档时先 seal（A6 的写路径）

把一个决策移进 `.agents/notes/archived/` 之后、提交之前，跑一次
`bash .meta/gates/archive-verify.sh seal`：它只为**尚未登记**的归档件追加
`.agents/notes/archived/manifest.sha256` 行（`<64hex><空格><空格><仓库相对路径>`），
既有行一个字节都不改（内容层 append-only；写入是「临时文件 + 同目录原子替换」，中断也不会留半截行）。
忘了跑 → A1（暂存区，**在任何跳过分支之前**评估）与 A4（全树）都会以「未封印」拒绝，CI 不会放行。

**封印的边界（诚实条款）**：归档件改一个字节而 manifest 未同步 → `check` 报「哈希不符」；manifest 自身
的 append-only（既有行不被重写）由 `seal` 保证，`check` 另拒格式非法 / 重复 / 孤儿条目 / 归档区内任何
**符号链接**（含目录链接）与归档根自身是链接的情形。`git ls-tree` 显示 HEAD 里确有归档内容、而工作区
`.agents/notes/archived/` 整个消失 → 报「归档目录被删除」并拒绝（整块删归档必须显式裁决：确有裁决时用
`git commit --no-verify` 并同步删除 manifest；非 git 仓 / 新仓无此记录，仍按「无归档 → 放行」）。
但若有人**同时**改归档件与其 manifest 行，纯内容校验分辨不出来（没有 git 历史真值可对）——那道防线属
review 与 CI 的可追溯性，本门不冒充。
**seal 是唯一写路径**，pre-commit 只 `check`——钩子不在提交窗口里偷偷改工作区/暂存区。

### A6 的时序（为什么不是"末尾调 A6"）

A1 里 A6 是**早期 probe**，不是末尾收口：判定在任何 `skip` 分支之前完成（`archive_probe`），
输出与拒绝留在 ADR 段之后打印（`archive_finish`）。顺序语义没变（ADR 段在前、归档段在后），
变的是**判定时机**——把 A6 留到末尾会把"本次没有暂存 ADR"变成绕过封印的门（实测缺陷：
删除或移出归档件不产生暂存 ADR 变更，末尾调用的 A6 永远跑不到）。故：
`archive_probe` 先算出 A6 的 rc 与输出 → ADR 段照常判 → `archive_finish` 打印归档段并按 rc
fail-closed 收口。门缺失时优雅回退（不影响 ADR 判定）。

### 本地层与远端层：互补，**不等价**（B5 实测结论）

两层跑的不是同一套判据，**不得**用"本地绿"推断"CI 也绿"，反之亦然：

| | 本地层 `adr-verify.sh`（A3 → A1） | 远端层 `ci-verify.sh`（A4 → A5） |
|---|---|---|
| 输入面 | 暂存区**改动的 ADR 子集**（`--diff-filter=ACMR -- .agents/notes/*.md`） | `verify-note.sh` **整树**（`find .agents/notes -type f -name '*.md'`） |
| 判据范围 | 1.5–1.8 + 1.10（逐文件）+ R6 字节比对 + R7 覆盖面 | 1.1–1.9a + 1.10（整树；1.9a = 文档标准家 `docs/AGENTS.md` 存在）|
| 定位 | 提交**前**拦截"这次改动" | 全仓**兜底**，抓历史债与整树不变量 |
| 通过时能推什么 | 只说明"本次改动的 ADR 是合法的" | 只说明"整树此刻合法"，不校验暂存态 |

**具体例子（实测）**：仓库缺 `docs/AGENTS.md`（1.9a 违例），且这次提交没碰 `.agents/notes/`：
`bash .meta/gates/adr-verify.sh` → **exit 0**（无暂存 ADR 变更，直接跳过）；
`bash .meta/gates/ci-verify.sh` → **exit 1**（整树模式跑 1.9a 报缺文档家）。
所以两层是**互补**关系（本地=窄而快，CI=全而慢），不是同一结论的两份拷贝。

**判定真源唯一**：格式判据只存在于 `.agents/skills/write-adr/verify-note.sh`；
A1 只做它做不到的三件事——TOCTOU 字节比对、`ACMR` 覆盖面、非普通文件（symlink/gitlink）拒绝——
不在此目录复刻 1.1–1.10（复刻必然漂移，故 `ponygo gate adr` 子命令被否决）。
A4 不是"再实现一遍"，它只是**编排**：真源整树 + 本目录的负样本 spec。

### 1.9 的分工：A1/A4 绿 ≠ 1.9b PASS（诚实边界）

判据 1.9 在实现上分三段，看守面**不同**，不得混着推断：

| 子项 | 内容 | 谁判 |
|---|---|---|
| 1.9a | 文档标准家 `docs/AGENTS.md` 存在 | `verify-note.sh`（整树模式）→ 经 A1/A4；`ponygo status` 亦判 |
| 1.9b | 根 `AGENTS.md` 链到 `docs/AGENTS.md`（**仅当** `meta.yaml` 声明 `ai-surface: on`；否则 SKIPPED 不计入 PASS） | **由 CLI 的 `verify_ladder` 判**：`ponygo status` 与 `ponygo audit` 共用同一函数，**两者都会报 1.9 FAIL**（勿低估判定面） |
| 1.9c | 包/模块文档覆盖（`packages/` `crates/` `src/` 下每包有 `AGENTS.md`/`README.md`，或在 `.meta/docs-tier/exemptions` 豁免） | 同上（`verify_ladder`，status 与 audit 共用） |

原因：1.9b 要读 `meta.yaml` 开关、1.9c 要扫包目录，二者是**实例状态**而非单条 ADR 的格式。
所以：`bash .meta/gates/adr-verify.sh` 与 `bash .meta/gates/ci-verify.sh` 全绿，只证明
「ADR 格式与归档封印没问题」，**不证明 AI 常载入口完好**——要证 1.9b，跑 `ponygo status`
（或 `ponygo audit`，同一函数；两者的 fail 列表都会出现 1.9）。
机器面（`status --json` / `audit --json`）的 `coverage` 块**同源**且**默认保守**：只有
`verify_ladder` 真的判过 1.9b（`ai-surface: on`）才给 `verified:["1.1-1.9"]`；其余情形——
含 off/未声明/值非法，以及**骨架缺口等根本没跑 ladder 的提前退出分支**——一律给
`verified:["1.1-1.8"]` + `skipped:["1.9b"]`（未判定 = 未验证，不得宣称）。

### 挂载方式（非破坏性）

- `core.hooksPath` 未设 → 设为 `.githooks` 并**回读校验**；已设（含 husky 绝对路径）→ **一律不动**，
  直接对该目录下的 `pre-commit` 动手（改 hooksPath 会静默停用 husky v9 的 `.husky/_` 链）。
- `pre-commit` 不存在 → 新建 A3 并 `chmod +x`；
- 已接线（块逐字节合规）→ 什么都不做；存在但未接线**且可执行** → 把哨兵块**插在 shebang 首行之后**
  （无 shebang 时插在文件最前），既有字节的**顺序与内容零改动**、不动权限：

```bash
# >>> ponygo-gate: adr-verify v1 >>>
if [ -f "$(git rev-parse --show-toplevel)/.meta/gates/adr-verify.sh" ]; then
  bash "$(git rev-parse --show-toplevel)/.meta/gates/adr-verify.sh" || exit 1
fi
# <<< ponygo-gate: adr-verify v1 <<<
```

  为什么是"前置"而不是"末尾追加"：既有钩子常以 `exit 0` 结尾（或含 `exec 他脚本` / `trap 'exit 0' EXIT`），
  末尾追加的块**永不执行**，而 2.4 仍报"已接线" → 假绿。块位置在首个顶层 `exit`/`exec` 之后 = 视为未接线并迁移
  （删旧块 + 前置新块，一次原子替换）。写入一律「临时文件 + `chmod` + `mv -f` 原子替换」，禁止 `>>` 追加；
  发现 BEGIN 无对应 END 的半截块 → 整块重建（不得再 append）。
- 存在但**不可执行** → 不改权限（激活沉睡钩子属破坏性副作用），2.4 FAIL 并打印
  「`chmod +x <hook>`（init 不会替你激活沉睡钩子）或删除该文件后重跑 init」。
- 判据 2.4 的机器识别 = `hooksPath` 归一化目录内 `pre-commit`：①存在 ②可执行 ③哨兵块
  （BEGIN..END 区间）**逐字节等于**规范块（EOL 归一；含 marker 行
  `# ponygo-gate: adr-verify v1` 与非注释调用行） ④ `<root>/.meta/gates/adr-verify.sh` 存在。
  ①–④全真才 PASS——逐字节比较一举封死 `echo adr-verify.sh`、`|| true`、`if false; then … fi`、
  `: adr-verify.sh`、注释掉调用行等全部变体。
- 前置条件：git 会**静默忽略**不可执行的钩子——生成/前置后务必确认 `test -x`。

### A2 的负样本怎么读

`adr-verify.spec.sh` 在 `mktemp -d` 临时仓里造样本（`trap … EXIT` 清理，仓库只读）：
合法 ADR（带参/无参两路）→ 断言 A1 为 0；时态错位（`proposed` 带 `## Decision`）、
`implemented` 带提案时代标题、`implemented` 含中文提案时代标题（`## 验收`，以及**复合词**
`## 验收清单`——中文按前缀匹配，防"无空格复合标题"绕过）、`Status` 与目录不一致
→ 断言 A1 非零；`rejected` 缺一行理由（判据 1.7）→ 断言 A1 非零**且**文案含「缺一行理由」，
补上 `— <理由>` → 断言放行；`implemented` 缺 `## Decision`（判据 1.8）→ 断言 A1 非零**且**文案含
「缺 ## Decision」，补 `## Decision` → 断言放行；中文提案标题用例的拒绝文案须含「提案时代标题」；
符号链接 ADR（`ln -s`，index 模式 `120000`）→ 断言 A1 非零**且**文案指向"必须是普通文件"；
staged 后又在工作区改动（两个方向：暂存合法→工作区改、暂存非法→工作区改好但未 `git add`）
→ 断言 A1 非零**且**文案含「ADR 有未暂存改动，请先 git add」；
无暂存 ADR / 无 `.agents/notes` → 断言放行。
每个"必须被拒"的样本都配一个相邻的正向对照（补一行理由 / 补 `## Decision` → 放行），
证明判据不是"一律拒绝"——只有负样本的 spec 分不清"门会响"和"门坏了"。

**围栏感知（Wave-2 §D2）**：1.8/1.10 的标题判定只认**围栏外**的 `## 标题`——``` 围栏内的标题是示例文本，
不算在场；未闭合围栏 → 其后全部视为围栏内（fail-closed）。故"把 `## Decision` 塞进示例代码块"不再能骗过
A1/A2/A4，spec 用「围栏内 Decision / 围栏内 Alternatives / 未闭合围栏」三负样本 + 一条"围栏外真标题"正向对照看守。
**CJK 标点（Wave-2 §D1）**：1.7 的理由正文口径先剥分隔符、空白、ASCII 标点与中文标点（，。、；：！？「」
『』（）【】《》…～· 等），剩余 ≥2 字节才算理由——`Status: rejected — 。` 必拒。

`## Alternatives considered` 曾是 notes/README 的强制契约而判定真源 1.1–1.9 不判（覆盖缺口），
该 case 当时做成**探针**；**2026-10-29 起 1.10 落地为硬判据**，探针随之翻转为**硬断言**：
只有 `## Problem` + `## Decision` 的非法样本必须被拒，且文案须含判据号 `1.10`（补救可执行）。
本目录仍不私设第二真值（判据实现只在 verify-note.sh 一处）。执行判据只证明"门会响"，抽样无偏靠 review。

### A6/A7 的负样本怎么读

`archive-verify.spec.sh` 用 `mktemp -d` 造**独立**样本仓（只放被测门禁 + 归档目录，不需要 git；
`trap … EXIT` 清理，被检仓库只读），以自身所在目录寻址 A6（不看 `cwd`、不从 `PATH` 取同名脚本），
逐类断言 A6 会拒绝，并配一个合法样本断言"门不得一律拒绝"：

| 样本类 | 造法 | 期望 |
|---|---|---|
| 篡改 | 封印后改归档件一字节（长度不变） | `check` exit 1 |
| 缺条目 | 归档件存在但 manifest 无该条目 | `check` exit 1 |
| 孤儿 | manifest 条目指向已删除的归档件 | `check` exit 1 |
| 重复 | manifest 同一路径登记两行 | `check` exit 1 |
| 非追加 | 既有条目被改动后再 `seal` | `seal` exit 1 **且** manifest 零写入 |
| 合法样本 | 两件 `Status: implemented` 归档件 | `seal` 0 且 `check` 0 |

`PONYGO_IN_SPEC` 只是信息变量，**不是**本 spec 的跳过开关：被 `audit --run-specs` 或 CI 调用时
同样完整执行全套样本（跳过 = 未验证，不得计为通过）。

运行时机：`bash .meta/gates/ci-verify.sh`（CI/本地全树）、`ponygo audit --run-specs`（opt-in，
本 spec 会被**真跑**并断言 exit 0）、pre-commit 不跑它（秒级预算）。

若 `audit --run-specs` 存在**未被真正执行**的 spec（0 个 / 自报 SKIPPED（含部分）/ 递归护栏拦截），
`audit` 以 **exit 3（证据不足）** 结束，且结论行不得出现「机械判据全部 PASS」。
无有效内容（去掉空白行与 `#` 注释行后为空）的 spec 判 **FAIL**（exit 1，优先于 3）。

### A8/A9 的负样本怎么读

`no-residue.sh`（A8）是**零残留门**：读清单 `.meta/gates/no-residue.manifest`（每行 `<pattern><TAB><scope>`；
`#` 起注释、空行忽略；非空且无 TAB = 格式非法 → exit 1），用 `rg --fixed-strings --no-messages -l` 在 scope
内按**字面**（不解释正则）检索——任一 pattern 命中 → 列出命中并 exit 1。缺清单 / 清单为空（去掉空白与注释行
后为空）→ exit 0 并打印「跳过」：没有删除面就没有残留面，不制造一道永远空转的门。参数只认 `--help` 与一个
可选清单路径，其余（未知 / 多余参数）→ exit 1。**运行时不承诺 `rg` 存在**（本框架只承诺 bash）：无 `rg` 时
退化为 `grep -rIlF`（同样字面匹配）并在 stderr 声明退化，不假装有 `rg` 的精确度。它服务的承诺是常载命约 3
「删代码前先搜消费者」的**收尾验收**：删除那次提交把被删符号写进清单，门证明"同一组检索词在生产树里零残留"。

`no-residue.spec.sh`（A9）在 `mktemp -d` 临时目录造夹具仓，逐类断言：命中 → 拒（并列命中）；
无命中 → 放行；空清单 / 缺清单 → 放行；未知参数 → 拒；pattern 含 `.` / `*` 等特殊字符 → **按字面**匹配
（当正则解释就会误报或漏报）。与 A2/A7 同规格：**执行判据**，由 A4 第 4 段与 `ponygo audit --run-specs` 真跑。

**A4 的四段**：① `verify-note.sh` 整树（1.1–1.11）→ ② A2 负样本 → ③ A6 `check` + A7 负样本 →
④ A8 零残留 + A9 负样本。第 ④ 段在文件缺失时打印提示但不失败（尚未分发 A8/A9 的旧实例不因此恒红），
存在即执行并按退出码判；未知参数一律 exit 1（fail-closed，不猜意图）。

> 诚实标注（P6）：门只能证明"清单里登记的那些 pattern 不再命中"，证明不了"清单本身完整"——
> 该把哪些删除面写进清单是 review/评审时的判断，不是门能替的部分。

## 门禁分层的三个时间经济层（P7，dsh 实证）

| 层 | 触发时机 | 时间预算 | 该抓什么（示例） |
|---|---|---|---|
| 本地层 | pre-commit | 秒级 | 格式（fmt --check）、空白、快速 lint 子集、暂存内容检查 |
| 中继层 | pre-push | 10 秒级 | typecheck、全量 lint——推送前的次级过滤 |
| 远端层 | CI | 分钟级 | 全仓质量矩阵、构建、测试、聚合门（防 skipped 计为通过） |

**核心原则**：每层只抓该层性价比最高的缺陷；本地窄、CI 全，两层跑同样检查 = 白付延迟（P7）。
分层是「值不值」的价值判断（P6）——小项目只上 CI 也合法，别为分层而分层。

**中继层 pre-push 是可选动作，框架不生成该 hook、也不设开关**：想让推送前再挡一道，
可在自己的 `.githooks/pre-push` 里加一行（按需自加，不装配就没有）：

```bash
bash .meta/gates/ci-verify.sh
```

> 可靠性 review 程序（掩盖式修法：加超时/加重试/全串行/吞错/弱断言/加 sleep 均不算根因修复）见 methodology §4.7——**本行非门禁、无执行语义**，checklist 2.A–2.C 判据链到该节，不链本行。

## 每个门禁的落地形态

- 脚本放本目录（`<name>.sh` 或等价），必须**非零退出 = 拒绝**语义；
- 配一个负样本 spec（`*.spec.*`，构造非法样例证明门会拒绝）；
- 挂载：`git config core.hooksPath .githooks` + 在 .githooks/ 放 pre-commit / pre-push；
- 每门一条"承诺 → 命令"的对应（凡机械可查的承诺必配命令，P2）。

## 负样本 spec 的两种形态（判据 2.3）

| 形态 | 判据强度 | 写法 |
|---|---|---|
| `*.spec.sh` | **执行判据**——由 `ponygo audit --run-specs`（opt-in）执行，exit 0 = 自检通过 | 自造非法样本 → 断言门非零拒绝；自造合法样本 → 断言门放行；全过才 exit 0 |
| `*.spec.md` 等其它后缀 | 形状判据（存在性，靠 review） | 只作说明/占位；机器不执行，**不得冒充真判据** |

**为什么执行是 opt-in**：`status` / `audit` 的默认语义是"判定优先只读"——不执行仓库里的脚本
（对陌生遗留仓库执行脚本 = 任意代码执行）。执行判据由显式开关承担，或在 CI / pre-push 层直接跑
（那两层本来就允许执行代码）：

```bash
ponygo audit --run-specs          # 跑 gates/*.spec.sh，断言各自 exit 0
bash .meta/gates/<name>.spec.sh   # CI / pre-push 层的等价写法
```

### 2.3 门与负样本配对（实例门）

- 每个实例门（`*.sh` / `*.ps1`）必须有**同 Stem、同语言后缀**的负样本 spec：
  `foo.sh ↔ foo.spec.sh`、`foo.ps1 ↔ foo.spec.ps1`；空文件不算配对。
- `ci-verify.sh` 是唯一编排器豁免；框架内置 A1/A2/A6/A7 不冒充实例门计分。
- 默认只读 `audit` 即核对配对，缺口以 `gate_spec_missing` 非零；`--run-specs` 仍只执行
  `*.spec.sh`，`.ps1` 只作结构配对，其执行语义由对应平台 CI 或 review 承担。

`*.spec.sh` 契约（违反即执行判据失败）：

- **自包含**：从自身路径解析仓库根（`ROOT=$(cd "$(dirname "$0")/../.." && pwd)`），不依赖调用者 cwd；
- **只读仓库**：样本一律建在 `mktemp -d` 临时目录、退出时清理；不得改动仓库文件；
- **秒级**：有 `timeout` 时施加 **软 25s + KILL 兜底 5s**（最坏 30s；无视 TERM 的挂死 spec 会被强杀，不会拖死体检）；
- **exit 0 = 自检通过**（"门确实会拒绝非法样本"）；非零 → 报 FAIL 并回显该 spec 输出尾部；
- **防递归**：嵌套由调起方判定——`ponygo audit` 在 `PONYGO_AUDIT_SPEC_DEPTH ≥ 1` 时拒绝再跑 spec
  （该层不执行任何 spec，按 `SKIPPED` 处理并 **exit 3**）；第一层 spec **必须真跑**。
  spec 内若仍见 `PONYGO_IN_SPEC=1`，那是旧版 `audit` 的遗留标记，**不再**表示"可跳过"；
  自检脚本保留该判断只作向后兼容，不得据此把"未验证"当"通过"。

> 诚实标注（P6）：执行判据只证明"脚本自检通过"，证明不了"脚本真的构造了非法样本"——后者靠 review；
> 非 `.sh` 后缀的 spec 连执行都没有，只是形状判据。"放个空 spec 文件"过得了存在性，证明不了任何拒绝语义。

## 提醒档（v2.1，非门禁）：doc-freshness WARN 片段

新鲜度/对应性是语义判断，做不成 FAIL 门禁（P6）——但可以进 pre-commit 做**启发式提醒**
（只 echo，不改退出码）。把下面这段追加到你的 pre-commit 末尾（栈无关示例，`.rs`
换成你栈的源码后缀；ponyllm 实例已验证）：

```bash
# [doc-freshness WARN] 暂存区改了源码却没带非-notes 文档更新时提醒。
# 计数排除 `.agents/notes/`——ADR 不算"文档更新"，否则"代码+ADR、无用户文档"永远静默
# （实证：ponyllm 8f37827 类提交）。纯重构/内部改动误报可忽略；噪声过大按 L2 退场 disable。
staged_src=$(git diff --cached --name-only --diff-filter=ACM 2>/dev/null | grep -E '\.(rs|ts|py|go)$' || true)
staged_doc=$(git diff --cached --name-only --diff-filter=ACM 2>/dev/null | grep -E '(\.md$|AGENTS\.md$)' | grep -v '^\.agents/notes/' || true)
if [ -n "$staged_src" ] && [ -z "$staged_doc" ]; then
  echo "=== [doc-freshness WARN] 暂存区有源码变更但无（非-notes）文档更新 ===" >&2
  echo "  改了用户可见行为/契约请同步对应文档的家（same-commit）；纯重构忽略。" >&2
fi
```
