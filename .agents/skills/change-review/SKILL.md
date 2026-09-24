---
name: change-review
description: 何时用：评审一处变更（工作区 diff / 单个提交 / 分支）是否符合已记录决策时——对照相关 ADR 查对应性、same-commit 文档、Status 迁移需求。多类大改先拆评审 scope，一次只审一个主题。
---

# change-review —— 变更评审（读旧 ADR 再动手）

一次轻量、可复现的单变更评审。只产出 **blocker / non-blocker + 证据**，
不代为决策（修不修、值不值归用户）。

## 真相源（冲突以它们为准）

- `.agents/notes/README.md` —— 决策契约 + 读触发规则
- `.agents/skills/write-adr/SKILL.md` —— 写前检索程序 + 对应性校准
- `.meta/constitution/constitution.md` —— 常载命约（先 ADR 后代码 / same-commit）
- `docs/AGENTS.md` —— 文档标准的家（用户文档落点）

## 步骤

1. **定范围**：评审对象是工作区 diff、单个提交还是分支？列出文件清单。
   多主题混装先要求拆 scope（一次评审只审一个主题；混装本身记一条 non-blocker）。
2. **检索相关 ADR**：按主题词、模块路径、关键符号 `grep -r` 扫 `.agents/notes/`
   活动树（archived 只读史不判）。无命中且变更非平凡 → 先按 write-adr 补 ADR 再评审
   （无对应 ADR 的评审是无米之炊）。
3. **逐项检查**（每项附磁盘证据）：
   - **对应性**：ADR 的 Problem/Decision 与代码现实一致吗？用 `grep`/`git log -S <关键符号>`
     验证声称的构造/行为存在；同提交携带的 ADR 是否文题相符（文不对题 = blocker）？
   - **same-commit 文档**：改用户可见行为/契约/配置了吗？对应文档的家（README/包 README/
     docs/）同步更新了吗？计数排除 `.agents/notes/`（ADR 不算用户文档更新）。
   - **Status 迁移**：PR 落 proposed note 是否同 diff 改写为 implemented？新废弃旧决策是否
     走 supersession（同一提交处理）？
   - **门禁与测试证据**：行为变更有聚焦测试吗？门禁全绿吗？（只认命令输出，不认口头声称）
4. **输出报告**：blocker（必须修：文不对题、无 ADR 的非平凡变更、Status 失真）/
   non-blocker（建议修：文档滞后、nit）各附证据；末尾一句话：合入条件。

## 校准样例

- 正例：评审报告每条 blocker 都附 `git show --stat <sha>`、ADR 路径、`grep` 命中行；
  "一个可证实 blocker 的短评审 > 一串 nit"。
- 反例：无证据断言"这段代码与 ADR 不符"却不给符号对照；把 review 判断说成"机器已验证"；
  堆 20 个 nit 却漏掉文不对题的 ADR。
- 反例（文不对题）：TUI 功能提交携带多协议提案——标题与代码无关，记 blocker 并要求拆条。
- 边界（靠 review）：重构归类、改动是否用户可见——判据管不到，列疑点 + 证据让人定夺。

## 验证与报告

- 本技能无退出码门禁（审查是 review 层）。
- 交付物 = 评审报告（blocker/non-blocker + 证据 + 合入条件）。
- 后续动作：blocker 修完重审；需补/迁移的 note 按 write-adr 落盘——审完不落盘等于没审。
