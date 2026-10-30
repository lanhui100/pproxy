# Agent Note: 替代治理形态适配：决策双轨收敛与远端流水线薄壳

Status: implemented

## Problem

`ponygo init` 在本仓检出两种「替代治理形态」（检测清单真源 = CLI 的 `detect_alt_gov_hits`，
任务书 `.meta/adaptation/alt-governance.md` 由 init 幂等生成）：

1. **决策载体双轨**：`docs/architecture/decisions/` 下已有 001–008 共 8 篇架构 ADR（2026-08~09 陆续落盘，
   最新 2026-09-29），而 2026-09-25 起活动决策统一改落 `.agents/notes/{lifecycle}/{class}/`。
   两处并存导致三件事说不清：读者该信哪边、旧条目还算不算"当前态"、新决策若触及旧主题要不要回改旧条目。
   机械面同样受影响——`verify-note.sh`（判据 1.1–1.10）只覆盖 `.agents/notes/`，
   `docs/architecture/decisions/` 的 8 篇落在判据之外（无 `Status:` 头、无 `## Alternatives considered`），
   每次新增决策都可能与其中某篇形成"无声分歧"。
2. **远端流水线无门禁**：`.github/workflows/` 已有 5 个 workflow（ci / cli-release / deploy-vercel /
   desktop-gate / desktop-release），全部承载具体构建与发布逻辑，**没有一个是治理门**。
   治理判据只活在本地 `.githooks/pre-commit`（`--no-verify` 可绕过）与人的记忆里，
   PR 上没有非零退出的兜底——这正是 2026-10-30 治理审查（`.agents/notes/proposed/process/2026-10-30-governance-audit.md`）
   记的 P1「远端层零治理门」。

两条都属于"框架要的是唯一真源 + 可验台阶，实例已有既成形态"——不处置就会让 `ponygo status`
长期 WARN（适配决策未登记），处置过头又会重写历史。本条即该 WARN 的完成判据
（`.agents/notes/**/process/*alt-governance*.md`）。

## Decision

**一、决策载体：notes 唯一真源，旧目录冻结只读、不迁移。**

- `.agents/notes/{proposed,implemented,rejected,archived}/{class}/` 是**唯一活动决策载体**：
  lifecycle 由目录承载、`Status:` 头与目录一致、格式由 `verify-note.sh` 1.1–1.10 机械看守。
- `docs/architecture/decisions/001–008` **冻结为历史存档**：原样保留、只读、**不迁移到 notes、不补 `Status:` 头、
  不改写正文**。理由：迁移必然改写原始时序证据（文件创建日、提交 sha），而"决策史真实"正是 L2 要守的东西；
  8 篇的正文已由 `docs/architecture/CURRENT.md` 的架构地图引用，移动会断链。
- **旧条目的效力口径**：凡与 notes 冲突，**以 notes 为准**（notes 是当前态，旧目录是快照）。
  旧 ADR 记录的是当时的取舍，不是当前承诺；引用它的地方必须标明"历史存档，当前态见 notes/<对应条目>"。
- **新增/改写决策一律落 notes**；若新决策实质取代某篇旧 ADR，在**新条目正文里**链入
  `docs/architecture/decisions/00X-*.md` 并写明取代关系（旧文件仍不动——冻结不等于失联）。
- 双轨分工的**索引家**是 `docs/AGENTS.md` 的 tier 表（决策记录行），不在本 note 与
  `.meta/docs-tier/README.md` 里各写一份（同一条规则两个家 = 迟早漂移）。

**二、远端流水线：只加薄壳，不承载判定逻辑。**

- 新增 `.github/workflows/ponygo-governance.yml`，唯一 `run:` 步是 `bash .meta/gates/ci-verify.sh`；
  该脚本内编排两道判据——全树 ADR 格式（`verify-note.sh` 无参模式，1.1–1.10）与暂存区门的负样本自检
  （`adr-verify.spec.sh`）。判定逻辑本体留在仓内，**可本地复现**（`bash .meta/gates/ci-verify.sh` 与 CI 同结论）。
- **既有 5 个 workflow 保持不动**：它们是构建/发布流水线，不是门禁；不往里面塞治理判定
  （改一次构建流程就要重验门禁，是反方向耦合）。换 forge（GitLab/Jenkins）时只需重写这 10 行壳。
- 本地层与远端层**互补不等价**：本地 `adr-verify.sh` 只判暂存区改动的 ADR 子集（秒级、含 R6 字节比对），
  远端 `ci-verify.sh` 判整树（含只在整树模式生效的 1.9）。本地绿不能推远端绿，反之亦然。
- 凭据扫描（`.meta/gates/secret-scan.sh`）在两处生效，判定本体只有一份：
  ①本地 pre-commit 第 0 段（秒级）；②`ponygo-governance.yml` 里追加的一步 `bash .meta/gates/secret-scan.sh`。
  远端那步是**实例追加**（A5 薄壳不进宇宙表，故不产生 drift），仍不安装 ponygo 二进制。
  放在远端而不是只靠本地，是因为 `git commit --no-verify` 一次就能跳过本地两层——凭据泄漏比格式违例更贵。
- 扫描面 = **索引 + HEAD 树**（`git grep --cached` ∪ `git grep HEAD`），不是工作树：pre-commit 的输入是
  暂存区，"暂存了含凭据的版本、随后把工作区改净"是常见的修订动作，读工作树会把它判成干净并放行
  （首版真实缺陷，2026-10-30 审查 F1）。HEAD 面的残留默认也 FAIL，但留 `SKIP_HISTORY=1` 逃生口：
  否则"修历史"那次提交本身会被自己的门挡住（死锁）；降级时如实打 WARN，不静默。
  边界（二进制不扫、历史提交不扫、只做模式匹配）写在脚本头部，不在本 note 复述。

**三、本仓自写门与框架资产 A1/A4 的分工（同一承诺不许两个实现）。**

| 门 | 归属 | 判什么 | 为什么在它手里 |
|---|---|---|---|
| `adr-verify.sh`（A1） | 框架资产（进宇宙表，勿改） | 暂存区 ADR 子集：R6「暂存字节 = 工作区字节」、R7 `ACMR` 覆盖面（`git mv` 绕不过）、非普通文件（symlink/gitlink）拒绝 | 这三件事 `verify-note.sh` 结构上做不到（它读工作区、无暂存概念） |
| `ci-verify.sh`（A4） | 框架资产（进宇宙表，勿改） | 编排：整树 `verify-note.sh` + A2 负样本自检 | 远端层要一次「全而慢」的兜底，且结论可本地复现 |
| `check-adr.sh` | 本仓自写（L1 时期遗留） | 转调 `verify-note.sh`（带参/无参透传） | 与 A1 不重复判据，只做「本地手工自查」的入口 |
| `cargo check --tests` / doc-freshness WARN | 本仓自写（pre-commit 第 1–3 段） | Rust 编译面 / 源码改动无（非-notes）文档更新的启发式提醒 | 前者是构建门（框架语料外）；后者是语义判断，只能提醒不能 FAIL（P6） |
| `secret-scan.sh` | 本仓自写（pre-commit 第 0 段 + governance workflow 一步） | 索引 + HEAD 树里的凭据形状 | 框架门禁语料不含凭据扫描，属实例自建；判定真源只此一份，远端只调一行不复制判据 |

分工原则不变：**格式判据只有 `verify-note.sh` 一处实现**（A1/A4/`check-adr.sh` 都经它），
A1 只补它做不到的三件事，本仓自写门不得复刻 1.1–1.10（复刻必然漂移）。

**四、登记与收口。**

- 本 note 落盘后 `ponygo status` 的「适配决策未登记」WARN 静默；`.meta/adaptation/alt-governance.md`
  是 init 生成的任务书（幂等重写，勿手改），与本 note 是"待办 → 处置结论"的关系，不是两处真源。
- 冻结决定不设截止日：旧目录永不再写；若将来确需把某篇旧 ADR 提升为活动决策，**重写一条 notes 条目**
  并在其正文链回旧文件，而不是把旧文件搬过来。

## Alternatives considered

- **全量迁移 8 篇旧 ADR 进 `.agents/notes/implemented/architecture/`**：否决。迁移要补 `Status:` 头、
  改写标题时态、对齐 1.10 备选节，等于**重写历史文件**——提交时间与文件创建日错位，可回放性反而变差；
  且旧条文里若有已被取代的取舍，搬进活动树会让"当前态"立刻失真。冻结 + 链入的成本更低、信息不丢。
- **两处并存但不声明（维持 WARN）**：否决。WARN 是"框架认为你没处置"的诚实信号；长期挂着等于训练所有人
  忽略 WARN。声明成本是一篇 note，收益是"哪边权威"这个问题从此有唯一答案。
- **把 `ci-verify.sh` 的判据直接写进 workflow YAML**：否决。YAML 里的判定逻辑无法本地复现（要开 PR 才知道红绿），
  且换 forge 就得重写一遍判据（P9：同一判据两份实现必然漂移）。
- **只保留本地 pre-commit，不补远端门**：否决。`git commit --no-verify` 一次即可绕过，
  历史上有实证；远端层是唯一无法被本地跳过的兜底。
- **删除 `docs/architecture/decisions/` 只留 notes**：否决。那是 2026-08~09 的真实决策证据，
  删掉等于把"为什么这么设计"从仓库里抹掉；`CURRENT.md` 的架构地图还在引用它们。

## Consequences

- 读者路径变短：当前态 → `.agents/notes/`（格式受门禁看守）；历史意图 → `docs/architecture/decisions/`（只读快照）。
  两处冲突时的裁决规则（notes 优先）写进了本 note，不再依赖口口相传。
- 冻结的代价：旧 ADR 不会随代码演进更新，可能与现状不符——这是**明示**的（"历史存档"），不是缺陷。
- 门禁面扩大后，新决策必须先过 `verify-note.sh` 1.1–1.10 才能进主干；对应地，
  `.agents/skills/write-adr/` 已升到含 1.10 的框架模板（缺 `## Alternatives considered` 直接 FAIL），
  写 ADR 的人不必猜判据落在哪一版。
- 遗留：`docs/architecture/decisions/` 8 篇仍在 `verify-note.sh` 判据之外（冻结即豁免，不与活动树同判）；
  若将来要求"旧条目也机械可验"，须另立 decision 并接受历史改写成本。
