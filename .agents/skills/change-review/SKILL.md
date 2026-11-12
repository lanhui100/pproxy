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

### 删除/收窄类变更（负空间程序，附加在上面四步之上）

命中任一即走本程序：删符号 / 删文件或目录 / 删依赖 / 把公共面收窄（改私有、降导出、缩参数、下线功能）。
**"没人用"不是证据，"检索不到生产消费者"才是**（P9 负空间对称：删除不是添加的逆操作）。

1. **三行证据**（每行都要磁盘或检索输出；缺一行即 blocker）：
   - `owner`：这块东西声称服务谁？（模块 / 团队 / 外部消费者）
   - `producer`：谁写入或生成它？（构造函数、写入路径、生成脚本、迁移）
   - `consumer`：谁读取或调用它？（`grep -rn '<符号>'`、`git grep`、`rg --fixed-strings` + 读调用点）
   - **判别句**：唯一调用者是**自身测试**或**同级辅助包**时，它只是**删除候选**，不是既成事实——
     测试为自己的实现服务不算消费者；把候选当结论直接开删 = 拿"测试覆盖"当"生产使用"的证据。
2. **五件同提交清单**（连根拔，缺一件就是留壳）：
   - ① 源码（删定义，不留 `deprecated` 空壳 / `TODO` 墓碑 / 空 `mod` 残块）
   - ② 测试与快照（删对应用例，刷新 golden 快照；不留被 skip 的用例当"纪念"）
   - ③ 依赖声明（`Cargo.toml` / `package.json` / `requirements` / lock 中随之无用的依赖）
   - ④ 目录条目（导出清单、入口文件、`mod` / `index` / 注册表 / 构建清单里的逐一声明处）
   - ⑤ 文档索引链（README / AGENTS 索引 / 包文档里指向它的路径、链接与计数）
3. **验收 = 检索零残留 + 门禁绿**：同一组检索词在生产树里 0 命中（命令与输出贴进报告，不写"应该没问题"）；
   `bash .meta/gates/ci-verify.sh`（含 A8 `no-residue.sh` 清单门，见 `.meta/gates/README.md`）与项目自身测试全绿。
4. **留痕**：删除是一次决策——`implemented/simplification/` 落一条 note，含 `## Removed surface`
   与 `## Reintroduction conditions`（判据 1.11：复活条件写"什么条件下把它接回来"，不写"以后再说"）。

## 校准样例

- 正例：评审报告每条 blocker 都附 `git show --stat <sha>`、ADR 路径、`grep` 命中行；
  "一个可证实 blocker 的短评审 > 一串 nit"。
- 反例：无证据断言"这段代码与 ADR 不符"却不给符号对照；把 review 判断说成"机器已验证"；
  堆 20 个 nit 却漏掉文不对题的 ADR。
- 反例（文不对题）：TUI 功能提交携带多协议提案——标题与代码无关，记 blocker 并要求拆条。
- 正例（删除评审）：删一个导出函数 —— 报告贴 `consumer` 三行（`owner`/`producer`/`consumer`）、
  `rg --fixed-strings '<符号>'` 的 0 命中输出、五件同提交的 `git show --stat`、`ci-verify.sh` exit 0。
- 反例（删除评审）：只跑 `rg <符号>` 命中测试文件就宣布"有消费者"（把自身测试当生产调用），
  或删了源码却留 README 里的路径与旧依赖声明（五件只做了一件）。
- 边界（靠 review）：重构归类、改动是否用户可见——判据管不到，列疑点 + 证据让人定夺。

## 验证与报告

- 本技能无退出码门禁（审查是 review 层）。
- 交付物 = 评审报告（blocker/non-blocker + 证据 + 合入条件）。
- 删除/收窄类变更另需：五件同提交清单在现场点齐 + 检索零残留的输出留档；缺件即 blocker。
- 后续动作：blocker 修完重审；需补/迁移的 note 按 write-adr 落盘——审完不落盘等于没审。
