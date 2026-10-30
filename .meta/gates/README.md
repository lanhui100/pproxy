# gates/ —— 门禁（把承诺转成非零退出命令）

**首版（L2 及以下）此目录为空。** 升到 L2 时放你的第一个门禁，并同步 meta.yaml 的 level 改为 2。
（ponygo 框架仓自身是例外：它在此目录 dogfood 自己的 ADR 门，见下节——实例仓照旧从空开始。）

## 本仓已接线的门禁（ADR 门，A1–A5）

「交付即接线」：`ponygo init` 幂等装配下列资产（`.meta` 已存在也装配，见 plan-freeze §6.2）；
repo 模板文件与 `ponygo` 内 heredoc 双写，EOL 归一后逐字节一致。

| 资产 | 路径 | 角色 | 退出语义 |
|---|---|---|---|
| A1 | `.meta/gates/adr-verify.sh` | 暂存区 ADR 格式门（无参=暂存 ADR；带参=指定路径） | 0 放行 / 非零拒绝 |
| A2 | `.meta/gates/adr-verify.spec.sh` | A1 的负样本执行判据（非法→拒、合法→放行、TOCTOU→拒） | 0 = 自检通过 |
| A3 | `.githooks/pre-commit`（或既有钩子内追加块） | 本地层挂载点 | 转发 A1 的退出码 |
| A4 | `.meta/gates/ci-verify.sh` | forge 无关全树门（verify-note 整树 + A2） | 0 放行 / 非零拒绝 |
| A5 | `.github/workflows/ponygo-governance.yml` | GitHub 薄适配器（checkout + A4） | 同 A4 |

**宇宙表只登记 A1/A2/A4**（不可变资产）：A3/A5 是实例可编辑文件（有人会追加自己的钩子逻辑、
改 CI 触发），塞进 `UPGRADE_UNIVERSE` 会让实例永久 drift 红。

### 承诺 → 非零退出命令

| 承诺（maturity-ladder §5） | 命令 | 命中判据 |
|---|---|---|
| 落了 ADR 就必须过机械格式门（暂存区） | `bash .meta/gates/adr-verify.sh` | 1.5–1.8 |
| 暂存字节 = 校验字节（不许"暂存后偷改"） | 同上（内建 R6 比对） | R6 TOCTOU |
| 重命名 ADR 不绕过门 | 同上（`--diff-filter=ACMR`） | R7 |
| 全树 ADR 格式常绿 | `bash .agents/skills/write-adr/verify-note.sh` | 1.1–1.10 |
| 门自己会拒绝非法样本 | `bash .meta/gates/adr-verify.spec.sh` | A2 |
| 远端层全树兜底 | `bash .meta/gates/ci-verify.sh` | A4（= `verify-note.sh` 整树 + A2） |

### 本地层与远端层：互补，**不等价**（B5 实测结论）

两层跑的不是同一套判据，**不得**用"本地绿"推断"CI 也绿"，反之亦然：

| | 本地层 `adr-verify.sh`（A3 → A1） | 远端层 `ci-verify.sh`（A4 → A5） |
|---|---|---|
| 输入面 | 暂存区**改动的 ADR 子集**（`--diff-filter=ACMR -- .agents/notes/*.md`） | `verify-note.sh` **整树**（`find .agents/notes -type f -name '*.md'`） |
| 判据范围 | 1.5–1.8 + 1.10（逐文件）+ R6 字节比对 + R7 覆盖面 | 1.1–1.10（整树），**含 1.9「文档有家」等只在整树模式判的项** |
| 定位 | 提交**前**拦截"这次改动" | 全仓**兜底**，抓历史债与整树不变量 |
| 通过时能推什么 | 只说明"本次改动的 ADR 是合法的" | 只说明"整树此刻合法"，不校验暂存态 |

**具体例子（实测）**：仓库缺 `docs/AGENTS.md`（1.9 违例），且这次提交没碰 `.agents/notes/`：
`bash .meta/gates/adr-verify.sh` → **exit 0**（无暂存 ADR 变更，直接跳过）；
`bash .meta/gates/ci-verify.sh` → **exit 1**（整树模式跑 1.9 报缺文档家）。
所以两层是**互补**关系（本地=窄而快，CI=全而慢），不是同一结论的两份拷贝。

**判定真源唯一**：格式判据只存在于 `.agents/skills/write-adr/verify-note.sh`；
A1 只做它做不到的三件事——TOCTOU 字节比对、`ACMR` 覆盖面、非普通文件（symlink/gitlink）拒绝——
不在此目录复刻 1.1–1.10（复刻必然漂移，故 `ponygo gate adr` 子命令被否决）。
A4 不是"再实现一遍"，它只是**编排**：真源整树 + 本目录的负样本 spec。

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
`implemented` 带提案时代标题、`Status` 与目录不一致 → 断言 A1 非零；
符号链接 ADR（`ln -s`，index 模式 `120000`）→ 断言 A1 非零**且**文案指向"必须是普通文件"；
staged 后又在工作区改动（两个方向：暂存合法→工作区改、暂存非法→工作区改好但未 `git add`）
→ 断言 A1 非零**且**文案含「ADR 有未暂存改动，请先 git add」；
无暂存 ADR / 无 `.agents/notes` → 断言放行。

`## Alternatives considered` 曾是 notes/README 的强制契约而判定真源 1.1–1.9 不判（覆盖缺口），
该 case 当时做成**探针**；**2026-10-29 起 1.10 落地为硬判据**，探针随之翻转为**硬断言**：
只有 `## Problem` + `## Decision` 的非法样本必须被拒，且文案须含判据号 `1.10`（补救可执行）。
本目录仍不私设第二真值（判据实现只在 verify-note.sh 一处）。执行判据只证明"门会响"，抽样无偏靠 review。

运行时机：`bash .meta/gates/ci-verify.sh`（CI/本地全树）、`ponygo audit --run-specs`（opt-in，
届时本 spec 因 `PONYGO_IN_SPEC=1` 报 SKIPPED、不计入 PASS）、pre-commit 不跑它（秒级预算）。

## 门禁分层的三个时间经济层（P7，dsh 实证）

| 层 | 触发时机 | 时间预算 | 该抓什么（示例） |
|---|---|---|---|
| 本地层 | pre-commit | 秒级 | 格式（fmt --check）、空白、快速 lint 子集、暂存内容检查 |
| 中继层 | pre-push | 10 秒级 | typecheck、全量 lint——推送前的次级过滤 |
| 远端层 | CI | 分钟级 | 全仓质量矩阵、构建、测试、聚合门（防 skipped 计为通过） |

**核心原则**：每层只抓该层性价比最高的缺陷；本地窄、CI 全，两层跑同样检查 = 白付延迟（P7）。
分层是「值不值」的价值判断（P6）——小项目只上 CI 也合法，别为分层而分层。

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

`*.spec.sh` 契约（违反即执行判据失败）：

- **自包含**：从自身路径解析仓库根（`ROOT=$(cd "$(dirname "$0")/../.." && pwd)`），不依赖调用者 cwd；
- **只读仓库**：样本一律建在 `mktemp -d` 临时目录、退出时清理；不得改动仓库文件；
- **秒级**：有 `timeout` 时施加 30 秒上限（挂死的 spec 不会拖死体检）；
- **exit 0 = 自检通过**（"门确实会拒绝非法样本"）；非零 → 报 FAIL 并回显该 spec 输出尾部；
- **防递归**：`PONYGO_IN_SPEC=1` 已在环境中时跳过并报 `SKIPPED`——**SKIPPED 不计入 PASS**。

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
