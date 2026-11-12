---
name: write-adr
description: 何时用：出现任何非平凡变更（行为变更 / 架构 / 跨文件或跨包契约 / 流程与工具链 / 测试策略 / 磁盘-线-配置格式 / 维护者可能再访的决定，命中任一）需要落决策记录时；或把记录迁移状态（proposed → implemented / rejected / archived）时。纯机械或局部编辑豁免。
---

# write-adr —— 把一次非平凡决策落成 .agents/notes/ 记录

## 真相源

- `.agents/notes/README.md` —— 路径规范、格式契约、触发规则（唯一真源，冲突时以它为准）
- `.meta/constitution/constitution.md` —— 常载命约第 1 条（决策必须入册且带 Alternatives）
- ponygo 框架仓库 `maturity-ladder.md` §5 —— L1 判据（status 机械校验的逐项定义）：<https://github.com/lanhui100/ponygo/blob/main/maturity-ladder.md>

## 非平凡判定边界（谁判、锁什么、不锁什么）

**判定权在模型，框架只锁"落了 ADR 之后的格式门"。** 框架不设行数/文件数/正则阈值来替你判
"这次改动重不重要"——阈值只会被绕过（拆小提交、改注释骗 grep），而误判成本不对称：多记一段
只是一段文字，漏记则是决策史永久缺口。所以：**判定是模型在动手前的一次判断，不是门禁的输入**。

| 判为**非平凡**（须写 ADR） | 为什么 |
|---|---|
| 破坏公共契约（改退出码语义、改 CLI 旗标、改函数/接口签名、改配置文件格式） | 下游全部消费者失配 |
| 换核心依赖 / 换运行时 / 换语言或引入新工具链 | 换的是长期维护面 |
| 改数据模型或协议（schema、序列化格式、线协议、迁移脚本） | 事后回滚成本高 |
| 改架构分层与并发模型（模块边界、单文件↔多文件、串行↔并行、锁策略） | 影响所有后续改动的地基 |
| 改门禁与发布语义（加/删门禁、改判据、改 release 与校验和链路） | 门禁是承诺的可验形态，改它即改契约 |

| 判为**平凡**（豁免，不写） | 为什么 |
|---|---|
| 纯格式（缩进、空行、排序、formatter 输出） | 无语义变更 |
| 纯文案（错别字、注释措辞、README 措辞不改规则） | 不改变承诺 |
| 纯重命名（局部符号/文件，无外部可见契约变化） | 等价变换 |
| 依赖 lock 更新（版本浮动的机械同步，不换核心依赖） | 噪声，重复记录会淹没真决策 |
| 生成物（构建产物、快照、自动生成的索引） | 可再生产 |

**灰区处理**：介于两者之间（"这算改契约吗？"）→ 写；写不出 `## Alternatives considered` 说明
根本没做决策，那才是唯一该重估的信号（缺备选 = 没权衡，见 `.agents/notes/README.md` 格式契约）。

**框架侧的锁在哪里**：只锁落盘之后——ADR 一旦进了暂存区，`.meta/gates/adr-verify.sh` 会先做
「暂存字节 = 工作区字节」比对（防"暂存后偷改"，R6），再交给 `.agents/skills/write-adr/verify-note.sh`
判 1.5–1.8 的机械影子；重命名（`git mv`）同样在覆盖面内（`--diff-filter=ACMR`）。
也就是说：**漏写 ADR 门禁抓不到（那是判断问题），写坏的 ADR 门禁一定抓得到（那是格式问题）**——
机制不做自己做不到的事，也不假装做了。

## 步骤

1. **判触发**：对照 description 的枚举，命中任一即写。拿不准 → 写（多记成本一段文字，漏记成本永久丢失）。
   **时序**：决策记录必须**先于对应代码变更、或与代码变更同一提交**——先 ADR 后代码；
   事后补记是债务，不是常态（时序本身是语义判断、靠 review，用 `git log` 对照 ADR 提交与代码提交校准）。
2. **写前检索**（必做，不是提示）：落笔前先查旧 ADR 是否覆盖同一决策——
   用主题词、模块路径、关键符号 `grep -r` 扫 `.agents/notes/` 活动树
   （proposed/implemented/rejected；archived 只读史不判），按下面判定表处理：

   | 检索结果 | 动作 |
   |---|---|
   | 无命中 | 直接写（本条即首案） |
   | 命中相关旧条 | 读全文；新 note 正文链入旧条路径；旧条若被部分取代，链回新条 |
   | 命中被取代的旧条 | 走 supersession：旧 implemented 条移入 archived/（冻结，原貌不动），或链入说明被哪条取代；**同一提交处理，不 deferred** |
   | 多阶段提案部分落地 | 落地部分拆条记 implemented，未落地部分留在 proposed（勿让 proposed 整条失准） |
3. **选 lifecycle**：`proposed/`（未实现）或 `implemented/`（随本次变更落地）。
4. **选 class**：`feature`（新能力）/ `bug-fix`（修缺陷）/ `simplification`（减复杂度）/ `architecture`（交付源码的结构）/ `process`（代码周围的工具与流程）/ `testing`（测试策略）。都不够 → 考虑在 `notes/classes.local` 登记领域扩展（扩展即一次治理决策，须评审）。
5. **建文件**：`.agents/notes/{lifecycle}/{class}/$(date +%F)-<topic-title>.md`，topic 用 kebab-case。
6. **头部**：第一行 `# Agent Note: <title>`，空行，`Status: <与所在目录一致>`。`rejected` 必须带一行理由：`Status: rejected — <为何否决>`（分隔符 `—` / `-` / `:` 任一，且**剥掉分隔符、空白、ASCII 与中文标点**后正文 ≥2 字节；判据 1.7 机械校验：`Status: rejected — 。` 这种「标点充理由」同样 FAIL；`*.zh.md` 豁免）。
7. **正文**：`## Problem` 开头（先写动机，脱离方案也成立），随后**按 lifecycle 选骨架**（判据 1.8 机械校验标题是否与 lifecycle 匹配，选错即 FAIL：`implemented/` 缺 `## Decision` 即 FAIL，含提案时代标题——英文 `## Proposal`/`## Plan`/`## Migration plan`/`## Acceptance criteria` 或中文 `## 提案`/`## 计划`/`## 迁移计划`/`## 验收标准`/`## 验收`/`## 风险`/`## 待办`——同样 FAIL；判据 1.10 机械校验 `## Alternatives considered` 必须在场；删除/收窄类决策另受判据 1.11 约束：`implemented/simplification/` 须含围栏外 `## Reintroduction conditions`，若含 `## Removed surface` 还须有一行检索收据（`rg` / `grep` / `git grep`））。**围栏感知**：``` 围栏内的 `## 标题` 是示例文本，不算在场（未闭合围栏 → 其后全部视为围栏内）；要证明「真有这一节」，标题写在围栏外：

   | lifecycle | 骨架 |
   |---|---|
   | `proposed/` | `## Proposal`（将来时）→ `## Alternatives considered` → `## Acceptance criteria` → `## Risks` |
   | `implemented/` | `## Decision`（现在时，必须在场）→ `## Alternatives considered`（必填）→ `## Consequences`（可选） |
   | `rejected/` | 提案原文冻结（保留 `## Proposal`），verdict 只写 `Status:` 行（含一行理由） |
   | `archived/` | 冻结豁免：保持归档时原貌，一字不改（走第 8 步的归档动作） |
8. **迁移/归档状态 = 四件事**，同一次变更内完成：
   ① 移动文件到目标 lifecycle（`git mv`；落 `archived/` 时同样用 `git mv` 保血缘）；
   ② 改 `Status:` 行（`archived/` 下保持原态 `implemented` 或写 `archived`，两种都合规）；
   ③ 把提案时代标题改写为现在时（proposed → implemented 时；`## Proposal` → `## Decision`，
   `## Acceptance criteria`/`## Risks` 折叠进 `## Consequences`）；
   ④ **归档时追加封印**：`bash .meta/gates/archive-verify.sh seal`（append-only，只为**尚未登记**
   的归档件追加 manifest 行；manifest 是归档件的唯一索引）。漏 ④ → 本地层 A1（暂存区）与远端层 A4（全树）
   都以「未封印」拒绝提交，CI 不放行。正文**一字不改**（冻结即法律）。

## 校准样例

- 正例（该写）：把某命令的退出码语义从 0 改为 1 → 行为变更，写 `implemented/feature/…`。
- 正例（该写）：评审后否决"用 yaml 多字段记版本" → 写 `rejected/process/…`，Status 行带一行理由。
- 正例（该写）：引入 lefthook 管理本地钩子 → 流程与工具链变更，写 `implemented/process/…`。
- 反例（豁免）：改 README 错别字、调整测试断言文案 → 纯机械/局部编辑，不写。
- 反例（豁免）：格式化重排、重命名局部变量 → 无行为/契约/结构/流程变化，不写。
- 边界（靠 review）：重构归哪个 class？判别器是"可观察行为是否改变"——改变 → feature/bug-fix；只为减复杂度 → simplification（`refactor` 刻意缺席，与之重叠）。
- 反例（时态错位，判据 1.8）：proposed/ 下用现在时 `## Decision` → 未批准的提案伪装成已落地的决定；proposed 必须用 `## Proposal`，迁移到 implemented 时再改写为现在时。
- 反例（标点充理由，判据 1.7）：`Status: rejected — 。`（理由只有一个中文句号）→ 剥掉标点后正文为 0 字节，FAIL（`1.7 <path>：rejected 缺一行理由`）；正确写法是写出**为什么**否决（如「与 X 冲突」「成本高于收益」）。
- 反例（围栏内标题，判据 1.8/1.10）：把 `## Decision` 放进 ```` ```text ```` 示例块 → 围栏内标题不算在场，仍报「缺 ## Decision」；示例代码块里的标题只是**样例**，不是本文档的节。
- 反例（围栏吞标题，判据 1.8/1.10）：prose 里想引用三反引号时，别让它出现在行首（含缩进行首）——会被当成围栏开启；改为行内引用或加前缀字符。
- 反例（删除无回程票，判据 1.11）：`implemented/simplification/…` 只写"删了什么、为什么删"、不给 `## Reintroduction conditions` → 被删能力永久失去复活支点；补法是把"什么条件下接回来"写成可判定的条件（"若出现外部消费者 X 或性能回归 Y"），而不是"以后有需要再说"。含 `## Removed surface` 时还要贴一行 `rg --fixed-strings '<符号>' <scope>` 的收据，"没人用"才有证据。
- 反例（缺备选节，判据 1.10，硬判据）：只写 `## Problem` + `## Decision` 就收尾、整条没有 `## Alternatives considered` → 决策史只剩结论没有权衡，读者无法判断"为什么不选另一条"，ADR 退化成事后记录；`verify-note.sh` 直接 FAIL（`1.10 <path>：缺 ## Alternatives considered`）。补法：把当时真正比较过的 2–3 个备选各写一行 + 各自落选理由（"没考虑过备选"本身不是备选——说明该决定还不该落条）。
- 反例（混合 ADR）：把"治理已采纳"（implemented/process）与"架构尚在设计中、零代码"（proposed/architecture）写进同一条 implemented note → 状态轴失真（未实施伪装成已落地）+ class 污染；**多类拆条**（notes/README 触发规则）：已落地的记 implemented，未实施的记 proposed。
- 反例（文不对题，判据管不到、靠 review）：TUI 定价表单功能（11 个 `.rs` 含大文件重写）携带一篇多协议声明 proposed note——标题与代码无关，ADR 成了"有记录无对应"的伪证据；正确做法是为 TUI 定价单独立条，把多协议提案放到它自己的分支/commit。
- 正例（对应性）：ADR 的 Problem/Decision 引用它治理的代码路径（`crates/…/src/….rs`）或提交 sha；多阶段提案部分落地时，落地部分拆条或在原文链入 implemented（勿让 proposed 整条失准，如 error-observability 四层中已落地的 `AttemptObserver` 构造器）。

## 验证与报告

- 跑 **`bash .agents/skills/write-adr/verify-note.sh`**（无参 = 整树；或跟单个相对路径）——机械影子 1.1–1.11（1.1–1.8：路径两轴/文件名/日期/Status（含 rejected 一行理由）/骨架标题（围栏感知）；1.9a：整树文档有家 docs/AGENTS.md 存在；1.10：必含 `## Alternatives considered`，围栏感知；1.11：`implemented/simplification/` 必含围栏外 `## Reintroduction conditions`，含 `## Removed surface` 时另需一行 `rg`/`grep`/`git grep` 检索收据）全部通过，exit 0 才算完成；FAIL 项逐条修复重跑。
- **门绿 ≠ 内容成立**：1.7 理由是否成立、1.8 标题是否真是决定、1.10 备选是否真权衡、1.11 复活条件是否真成立属**非机械项**（见 `.agents/notes/README.md` 同名小节），由 `governance-review` / `change-review` 抽查。
- 归档/迁移落 `archived/` 时，再跑 **`bash .meta/gates/archive-verify.sh seal`**（追加封印，append-only）与 **`bash .meta/gates/archive-verify.sh check`**（只读校验：逐条比对归档件哈希，应为 exit 0；报「未封印」/「哈希不符」即欠账）。
- 再跑 `ponygo status`：决策数 +1、L1 判据全绿、无卫生 WARN。
- 报告格式：新记录路径 + lifecycle/class + 一句话 Problem + verify-note.sh 的通过输出。
