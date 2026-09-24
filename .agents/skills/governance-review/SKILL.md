---
name: governance-review
description: 何时用：用户明确要求审查本项目治理体系 / 治理体检 / 找治理问题点 / 判断是否升级治理级时。**只许用户点名调用**（disable-model-invocation: true）——重型审查流程防模型自主误路由；用户在一轮对话里提出即触发。
user-invocable: true
disable-model-invocation: true
---

# governance-review —— 全面治理审查

一次可复现、证据锚定的治理体系审查。只产出**问题清单 + 证据 + 最小改进动作**，
不代为决策（值不值归用户）。

## 真相源（冲突以它们为准）

- `.agents/notes/README.md` —— 决策契约：双轴路径 / 格式 / 时序 / 多类拆条 / 计划文档的家
- `.agents/skills/write-adr/SKILL.md` + `verify-note.sh` —— 写 ADR 的程序与机械校验
- `.meta/constitution/constitution.md` + 根 `AGENTS.md` 投影体 —— 命约 / 停止线
- `docs/AGENTS.md` —— 文档标准的家（tier 分类 / 写作规则 / slop checklist，agent 自动加载）
- `.meta/` 与 `.agents/` 各目录 README —— L1/L2/L3 升级契约

## 本审查不覆盖（显性边界，报告中必须复述）

以下维度本技能**不查**，报告末尾须原文复述本节，防止"治理体检"结论被高估：

- **文档分层漂移全量（L2 doc-sync 门）**：tier 放置语义 / slop 清单逐项 / 词数预算——本技能查"家是否存在且自动加载"（0.5/1.9/2.5），不查"内容是否放对家"（doc-sync 门，靠 review）；
- **methodology 界外项**：供应链安全（audit/SBOM/许可证）、可观测性（遥测/告警/oncall）、发布与版本治理、开发环境可复现、API 契约演进治理——这些不在 ponygo 门禁语料内，需另行专项；
- **语义真实性终审**：本技能对语义判断只列疑点 + 附原文，结论归用户（P6 形意分离）。

## 步骤

1. **跑证据命令**（输出贴进报告，作为每一结论的锚）：

   ```bash
   ponygo status
   ponygo audit
   bash .agents/skills/write-adr/verify-note.sh
   git log --oneline
   # 项目自身的测试门禁（按项目实况替换；无则声明"无测试门禁"）：
   bash tests/run.sh 2>/dev/null || <项目等价命令> || echo "无测试门禁"
   ```

   命令不可解析的环境（如无 bash）降级为 review，并在报告中声明"未机械判过"——不得假装跑过。

2. **逐面审查**：
   - **结构面（机械）**：骨架完整？status/audit/verify-note 全绿？WARN 有哪些（尤其游离计划文档 / 非 git / 缺 .gitignore / 新鲜度启发式 / 未来日期）？
   - **决策面（语义 + 机械）**：每条 ADR 的 Status/class 与内容真实一致吗（implemented 是否真落地、proposed 是否未实施）？时序符合"先 ADR 后代码/同提交"吗（git log 对照）？有多类混装吗？Alternatives 是真实候选而非凑数吗？
     - **对应性抽查（v2.1）**：抽 N 条 implemented ADR，用 `grep`/`git log -S <关键符号>` 对照代码现实——Decision 声称的构造/行为在代码里存在吗？抽 proposed ADR，看是否有已部分落地未拆条？同提交携带的 ADR 是否文题相符（反例：功能提交携带无关提案 = 伪证据）？
   - **文档面（v2.0，机械 + 语义）**：文档体系是否真正生效——
     - 家载体对吗？`docs/AGENTS.md` 存在且是文档标准的家（agent 自动加载）？根 `AGENTS.md` 链到它了？（判据 0.5/1.9）
     - 包文档有家吗？每个包目录有 `AGENTS.md` 或 `README.md`，或入豁免清单？（判据 1.9c）
     - **文档与代码同提交吗？**（dsh same-commit 规则）`git log` 对照：改行为/契约的提交是否带了对应文档更新？文档家是不是空壳（有文件无内容/无索引）？
     - **新鲜度抽查（v2.1）**：取近 5 个功能提交，逐个对照有无对应文档更新（计数排除 `.agents/notes/`——ADR 不算用户文档更新）；用户可见行为（新命令/新端点/新配置项/新面板）必须在 README 或包 README 可寻，否则记"新鲜度欠账"。
     - 分层激活了吗？`.meta/docs-tier/README.md` 是模板态还是已激活清单？（判据 2.5）
   - **卫生面**：游离计划文档？.gitignore/.rgignore 有效性？构建产物入版本库？
   - **负空间面（语义 + 初筛机械）**：语料是否开始腐烂——
     - 初筛：`find .agents/notes/implemented -name '*.md' -mtime +180`（半年未动的 implemented note 清单，仅作候选，不作结论）；
     - supersession 检查：近 90 天新增的决策是否覆盖了旧决策却未处理旧条（`git log --since='90 days ago' --name-only .agents/notes/` 对照）；
     - `.rgignore` 是否含 `/.agents/notes/archived/` 隔离（`grep -qF '/.agents/notes/archived/' .rgignore`）；
     - 哪些候选该归档/保留是语义判断（判据：理由与负面保证是否还指导未来工作），标"靠 review"。
   - **流程资产面（元审查）**：`.agents/skills/` 自身也要被审——每个技能的触发式 description 与实况一致吗？校准样例还锚得住当前判据吗？技能还有真实消费者吗（无消费者 + 无恢复计划 = 退场候选，按 L3 退场条件记录）？
   - **升级面**：按 maturity-ladder 判据，到 L2 差哪几条（gates / 负样本 spec / hooksPath / 文档分层激活 2.5）？哪些"承诺"至今没有非零退出命令看守、全凭自觉（这是 L2 的信号清单）？
   - **盲区声明**：明确列出"判据管不到、全靠自觉"的清单（至少包含文档分层语义部分与负空间面的语义部分）。

3. **输出报告**（表格）：

   | # | 问题 | 严重度(P0/P1/P2) | 磁盘证据(路径+命令) | 最小改进动作 |
   |---|---|---|---|---|

   末尾给：当前治理水位一句话 + 是否建议升 L2（附判据依据）+ 复述「本审查不覆盖」节。

## 校准样例

- 正例：报告每条问题都附 `git show --stat <sha>`、`ponygo status` 输出、`find` 结果等硬证据；"游离计划文档"类结论直接指向文件名与 WARN 输出；拿不准的标"靠 review 待人工定夺"。
- 正例（文档面）：`docs/` 目录存在但 `docs/AGENTS.md` 缺失（只有 README）→ 判"家载体选错/缺文档标准家——README 不进 agent 上下文，文档即使写了 agent 也不自动看"（实证：ponyllm 137 行 README 一个装五职责、0 个 docs 文件）。
- 正例（文档面同提交）：`git log` 显示"改协议转译"提交只动了源码没动文档 → 列出提交 sha 与对应契约文档，标"same-commit 违例"。
- 正例（负空间面）：`find` 初筛出 12 条半年未动的 implemented note，逐条列出路径 + 最后提交 sha，其中 3 条标"疑似被 <新 note 路径> 覆盖而未归档——靠 review"，不直接判"该归档"。
- 反例：凭空断言"ADR 时序有问题"却不给 `git log` 对照；把"值不值升 L2"当结论输出（该做的只是列出判据差几条）；把 review 层能判的（真实性）说成"机器已验证"；把 `-mtime +180` 初筛结果直接当"腐烂"结论。
- 反例（隐性盲区）：报告自称"全面治理审查通过"却未复述「本审查不覆盖」节——缺口不声明等于制造虚假安全感。
- 反例（文档面）：看到 `docs/` 有 README 就判"文档治理 OK"——README 不进 agent 上下文，必须查 `docs/AGENTS.md`（自动加载的家）是否存在。
- 边界（靠 review）：某条 ADR 的 Alternatives 是否"真实"——判据管不到，只列出疑点并附原文，让用户定夺。

## 验证与报告

- 本技能无退出码门禁（审查是 review 层；机械面已由 verify-note.sh / ponygo status 各自承担）。
- 交付物 = 审查报告（上述表格 + 水位 + 升 L2 建议 + 不覆盖声明）。
- 后续动作：把问题清单落一条 `.agents/notes/proposed/{class}/yyyy-mm-dd-governance-audit.md`（含本轮审查证据），修复按最小改进动作逐个执行并迁移 implemented——审查完不落 note 等于没审。
