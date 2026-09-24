---
name: write-adr
description: 何时用：出现任何非平凡变更（行为变更 / 架构 / 跨文件或跨包契约 / 流程与工具链 / 测试策略 / 磁盘-线-配置格式 / 维护者可能再访的决定，命中任一）需要落决策记录时；或把记录迁移状态（proposed → implemented / rejected / archived）时。纯机械或局部编辑豁免。
---

# write-adr —— 把一次非平凡决策落成 .agents/notes/ 记录

## 真相源

- `.agents/notes/README.md` —— 路径规范、格式契约、触发规则（唯一真源，冲突时以它为准）
- `.meta/constitution/constitution.md` —— 常载命约第 1 条（决策必须入册且带 Alternatives）
- ponygo 框架仓库 `maturity-ladder.md` §5 —— L1 判据（status 机械校验的逐项定义）：<https://github.com/lanhui100/ponygo/blob/main/maturity-ladder.md>

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
6. **头部**：第一行 `# Agent Note: <title>`，空行，`Status: <与所在目录一致>`（rejected 可带一行理由：`rejected — <理由>`）。
7. **正文**：`## Problem` 开头（先写动机，脱离方案也成立），随后**按 lifecycle 选骨架**（判据 1.8 机械校验，选错即 FAIL）：

   | lifecycle | 骨架 |
   |---|---|
   | `proposed/` | `## Proposal`（将来时）→ `## Alternatives considered` → `## Acceptance criteria` → `## Risks` |
   | `implemented/` | `## Decision`（现在时）→ `## Alternatives considered`（必填）→ `## Consequences`（可选） |
   | `rejected/` | 提案原文冻结（保留 `## Proposal`），verdict 只写 `Status:` 行 |
8. **迁移状态** = 移动文件到目标 lifecycle + 改 `Status:` 行 + 把提案时代标题改写为现在时——三件事在同一次变更内完成。

## 校准样例

- 正例（该写）：把某命令的退出码语义从 0 改为 1 → 行为变更，写 `implemented/feature/…`。
- 正例（该写）：评审后否决"用 yaml 多字段记版本" → 写 `rejected/process/…`，Status 行带一行理由。
- 正例（该写）：引入 lefthook 管理本地钩子 → 流程与工具链变更，写 `implemented/process/…`。
- 反例（豁免）：改 README 错别字、调整测试断言文案 → 纯机械/局部编辑，不写。
- 反例（豁免）：格式化重排、重命名局部变量 → 无行为/契约/结构/流程变化，不写。
- 边界（靠 review）：重构归哪个 class？判别器是"可观察行为是否改变"——改变 → feature/bug-fix；只为减复杂度 → simplification（`refactor` 刻意缺席，与之重叠）。
- 反例（时态错位，判据 1.8）：proposed/ 下用现在时 `## Decision` → 未批准的提案伪装成已落地的决定；proposed 必须用 `## Proposal`，迁移到 implemented 时再改写为现在时。
- 反例（混合 ADR）：把"治理已采纳"（implemented/process）与"架构尚在设计中、零代码"（proposed/architecture）写进同一条 implemented note → 状态轴失真（未实施伪装成已落地）+ class 污染；**多类拆条**（notes/README 触发规则）：已落地的记 implemented，未实施的记 proposed。
- 反例（文不对题，判据管不到、靠 review）：TUI 定价表单功能（11 个 `.rs` 含大文件重写）携带一篇多协议声明 proposed note——标题与代码无关，ADR 成了"有记录无对应"的伪证据；正确做法是为 TUI 定价单独立条，把多协议提案放到它自己的分支/commit。
- 正例（对应性）：ADR 的 Problem/Decision 引用它治理的代码路径（`crates/…/src/….rs`）或提交 sha；多阶段提案部分落地时，落地部分拆条或在原文链入 implemented（勿让 proposed 整条失准，如 error-observability 四层中已落地的 `AttemptObserver` 构造器）。

## 验证与报告

- 跑 **`bash .agents/skills/write-adr/verify-note.sh`**（无参 = 整树；或跟单个相对路径）——机械影子 1.1–1.8（路径两轴/文件名/日期/Status/骨架标题）+ 整树 1.9（文档有家：docs/AGENTS.md 存在）全部通过，exit 0 才算完成；FAIL 项逐条修复重跑。
- 再跑 `ponygo status`：决策数 +1、L1 判据全绿、无卫生 WARN。
- 报告格式：新记录路径 + lifecycle/class + 一句话 Problem + verify-note.sh 的通过输出。
