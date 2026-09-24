# notes/ —— 决策记录（ADR / Agent Note）

这是治理根的**记忆**：任何"为什么这样 / 为什么不那样"的决定，都落到这里，
而不是散落在人脑、聊天记录或 PR（拉取请求）线程里。

## 路径规范

```
notes/{lifecycle}/{class}/yyyy-mm-dd-topic-title.md
```

两个轴都编码进**路径**（文件夹即标签，内容里不必重复声明，二者永不漂移）：

- **lifecycle（状态轴，顶层）** 随状态迁移：
  - `proposed/` —— 提案，未实现或部分实现
  - `implemented/` —— 已落地，记录当前态（事实随代码更新）
  - `rejected/` —— 已否决，判定保留在路径 + Status 行
  - `archived/` —— 冻结历史快照（只有 implemented 能进入；原理见 ponygo 框架仓库
    `docs/methodology.md` §2.3：<https://github.com/lanhui100/ponygo/blob/main/docs/methodology.md>）
- **class（类别轴，嵌套）** 来自封闭集合：
  - `feature`（新能力）/ `bug-fix`（修缺陷）/ `simplification`（减复杂度）/
    `architecture`（交付源码的结构）/ `process`（代码周围的工具与流程）/ `testing`（测试策略）
  - class 目录由**写入时创建**，不预建空目录——空目录既不承载事实、又制造"有决策"的假象。
  - **lifecycle 四态目录由 `ponygo init` 预建**（`proposed/ implemented/ rejected/ archived/`）：
    状态轴的空态是合法的——它代表治理根已播种；而 class 是类别轴，空目录不承载事实，故不预建。
  - **领域扩展（L5 开放口）**：六类不够用（领域定制的第一个诉求常是加一个 class）时，
    在本目录放 `classes.local`，每行一个额外 class 名（小写字母/数字/连字符；`#` 起注释）。
    封闭集本身不放开——扩展必须显式落盘成文件，留有可审计的载体，而非随手 mkdir。

## 文件格式契约

头部固定前两行：

```markdown
# Agent Note: <title>

Status: <status>
```

- `Status:` 三选一，必须与所在文件夹一致：`proposed` / `implemented` / `rejected — <一行理由>`。
- 正文从 `## Problem` 开始（先写动机，脱离方案也成立）。
- **正文骨架按 lifecycle 分态**（时态与状态一致，判据 1.8 机械校验）：

  | lifecycle | 正文骨架 |
  |---|---|
  | `proposed/` | `## Proposal`（可将来时）→ `## Alternatives considered` → `## Acceptance criteria` → `## Risks`；禁止现在时 `## Decision` |
  | `implemented/` | `## Decision`（现在时）→ `## Alternatives considered` → `## Consequences`（可选）；禁止提案时代标题（`## Proposal` / `## Plan` / `## Migration plan` / `## Acceptance criteria`） |
  | `rejected/` | 提案原文冻结（保留 `## Proposal`），verdict 只在 `Status:` 行；禁止现在时 `## Decision` |
  | `archived/` | 冻结豁免（保持归档时原貌，不改一字） |

- 每条必含 `## Alternatives considered`（候选方案与落选原因）——记录无备选的决策是在邀请重开争执。
- **迁移即改写**：proposed → implemented 时，移动文件的那次变更必须把 `## Proposal`
  改写为现在时 `## Decision`（Acceptance criteria / Risks 折叠进 `## Consequences`）；
  提案用现在时等于"未批准的方案伪装成已落地的决定"，状态轴在正文层失真。

## 触发规则（谁在什么时候必须写）

非平凡变更 = 命中任一：行为变更 / 架构 / 跨文件或跨包契约 / 流程与工具链 /
测试策略 / 磁盘-线-配置格式 / 维护者可能再访的决定。
命中即同一次变更内新增或更新记录。纯机械或局部编辑豁免。

**时序**：先 ADR 后代码（先于或同一提交）；事后补记是债务，不是常态。

**多类拆条**：一次变更命中多个类别（如"采纳治理 + 架构设计"）→ **拆多条记录，
不合并**——混装会把未实施的方案伪装成已落地（状态轴失真），并污染 class 轴的检索面。

**计划文档的家**：实施计划 = `proposed/` note（`## Proposal` + `## Acceptance criteria`）。
禁止根目录游离 `*plan*.md` / `ROADMAP.md`——计划烂在根目录永远不会被迁移，
且是 spec-speak 的滋生地（status/audit 对游离计划文件输出 WARN）。

## 读触发规则（谁在什么时候必须读旧 ADR，与写触发对称）

只写不读，决策记录就是只写存档。以下时刻必须先读相关旧 ADR 再动手：

- **写新 ADR 前**：按主题词/模块路径/关键符号 `grep` 检索活动树（proposed/implemented/
  rejected；archived 只读史不判）——命中则读全文、链入、判 supersession（见 write-adr
  写前检索程序）；
- **评审变更时**：用 change-review 技能——先检索相关 ADR 再逐项对照（对应性/same-commit/
  Status 迁移），无对应 ADR 的非平凡变更先补 ADR 再评审；
- **找简化/归档时**：读 note 树理解架构意图，按"未来价值"分类（能指导未来工作的理由/
  备选/负面保证保留，其余归档）；
- **治理审查时**：governance-review 决策面抽查（implemented 是否真落地、proposed 是否
  已实施未迁移）；
- **接手陌生模块时**：先读相关 implemented 再读代码——决策史是比代码更便宜的地图。

读的结论只进两处：新 note 的链入/拆条，或评审报告的问题清单——不落盘等于没读。

## 与下一级的关系

有决策记录是 L1 的判据之一；是否"承诺可验"（L2）取决于 `gates/` 是否有非零退出门禁。
