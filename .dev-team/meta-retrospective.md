# 元框架复盘：治理修复 governance-remediation-2026-10-30（C 级）

## 1. 通信拓扑与信噪比（Topology & Noise）

- **名册**：Lead + Test Agent + Executor = 3 常驻（≤8），另挂 6 个 Subagent（3 路 L3 审查、1 路方案逆向、1 路 L3-P、1 路复审），审查全部走计算池、未挤占常驻槽位。3 人团队对应「写域强耦合」的实况：安全脚本、pre-commit、CI、文档高度共享同一批文件，并行 Executor 只会互相踩，k=1 串行是对的。
- **信噪比**：Execute 侧三次波次回报都是「文件清单 + 原始命令 + 退出码 + 遗留项」三段式，无复述；Test Agent 在冻结前主动给出逐文件 sha256 指纹并在冻结点自证未漂移，这条动作把「改测试换绿」的嫌疑从根上堵死，是本会话最有价值的单点产出。
- **重复命令率**：低。`tests/governance/run.sh` 被有意重复 5 次（红相 1、Lead 抽验 2、Test Agent 三次一致性含 cwd=/tmp），每次都能回答不同问题（红/绿/确定性/cwd 无关），属门禁必需。真正的浪费在**审查侧**：3 路 L3 审查跑了 15–25 分钟，Lead 其间只能空等（无法 steer 已派生的 subagent），墙钟成本明显高于实施本身。
- **探索者/审查者路由**：审查一律 Subagent 是正确选择（无状态、交付即解耦），但缺少时限契约，导致长尾。

## 2. 门禁穿透与误杀率（Gate Penetration / False Negatives & Positives）

- **红相先行有效**：Test Agent 的冻结套件红相 PASS=2/FAIL=8，实施后 10/10 绿；`tests/governance` 自 `e0acfe3` 起逐字节未动（`git diff e0acfe3..HEAD -- tests/governance` 为空），排除「改考卷换绿」。
- **汇聚栅栏真实拦截（本会话核心价值）**：第一轮 3 路 L3/L3-P 全部判 FAIL，抓出 3 个真 BLOCKING——① `secret-scan` 扫工作树而非索引（暂存含凭据、工作区改净时可放行，评审给了可复现反例）；② CI 远端没有凭据步（A1「本地+CI 各一次」只做到本地）；③ `docs/AGENTS.md` 声称 `rejected/` 有家而目录不存在。另有一条 P1：`implemented` 的 admin quota ADR 文本与 `lib.rs:502` 现实相反。**这些都是机械测试全绿时仍然存在的东西**——说明「测试绿 ≠ 承诺成立」，L2 的「承诺可验」必须配独立审查，不能只看 CI 绿灯。
- **误杀率**：可接受。复审时把「HEAD 残留默认 FAIL + SKIP_HISTORY 逃生口」「`*.zh.md` 1.10 豁免」「ci-verify 把 SKIPPED 记 PASS」三条判为 ADVISORY 而非 BLOCKING，其中前两条经 Lead 核对是既定设计（脚本头部有明文口径），第三条是模板资产语义、改它会产生新 drift。没有出现需要驳回的误判。
- **门禁自身的门**：`dev-review-lint.py` 的 v2 溯源字段（schema_version/reviewer.session_id/fresh_context/executed_by/attestation）第一次直接打回 4 份收据——「审查收据」也有假收据风险，这条机械门有效。

## 3. 分工契约与隔离有效性（Contract Isolation）

- **测试分权**：Executor 全程未触碰 `tests/governance/**`（冻结 diff 为空），Test Agent 未在冻结后改一字；红相 commit 早于首个实现 commit，`red-before-impl` 成立。
- **git 写权单一**：Executor 只报「需 `git rm --cached` 的文件清单」，索引写入全部由 Lead 执行，5 次定向暂存均先 `git status --porcelain` + `git diff --cached --stat` 留收据，没有出现混入非本任务文件的事故。
- **契约越界（已声明）**：`implemented/feature/2026-10-03-admin-token-…` 不在 Executor 的写域（`implemented/process/**`），由 Lead 亲自改一行 Decision 文本——属 Lead 写权例外里的「非业务代码小修」，但纪律上应提前登记到任务写域，本次是事后补记。
- **信息下发**：Executor 的 prompt 逐条点了 W1/W2/W3 与验收命令，未出现「契约写了但执行者没拿到」的旧病；Test Agent 拿到的是黑盒契约（G0–G9），没有拿到实现细节，隔离有效。
- **遗留（合法 backlog）**：pre-push 中继层、supersession/toast 三连、ROADMAP 索引 M1–M6→M7、`archived/` 哈希封印、crates README 空壳、`scripts/*-gate.sh` 未登记承诺表、observe 声明、公共历史里已失效 token 的 `filter-repo` 清理——均写入 `.dev-team/governance-state.json.known_gaps`，未假装修完。

## 4. 元协议迭代建议（Self-Evolving Protocol）

1. **方案审查必须在实施前**：本次 Phase-1 方案逆向审查被排到实施之后（时间压力），结果它审的是「已实施 + 未修项」，虽然仍抓到 3 条真缺陷，但代价是返工。建议 SKILL 明确：Phase-1 收据缺失时禁止开波，且由 lint 机械拒绝（`review-plan*.json` 的 `reviewed_commit` 必须是实现首个 commit 的祖先）。
2. **审查者需要时限契约**：给 L3 Subagent 的 prompt 必须带「≤N 分钟、超时用已有证据出 JSON、BLOCKING 必须已实测」的硬约束；本次 15–25 分钟的长尾完全可以用这条压掉。
3. **「门禁对象 = 提交对象」应上升为通用判据**：`secret-scan` 扫工作树而非索引，是「门禁输入面与真实输入面不一致」的典型。建议在 references 里沉淀一条通则：*任何 pre-commit 门必须对 `git diff --cached` 的字节生效，而不是编辑器里的工作树*。
4. **FAIL 后的复审要有单一路径**：本轮由 1 个复审 Subagent 覆盖全部 BLOCKING，并把原 3 路判据逐条回放（含未修复的 ADVISORY 是否被吹成 PASS），比重新派 3 路更省且更聚焦；建议写进 SKILL 的 barrier 回退闭环。
5. **测试套件的外部 oracle 要 pin**：`tests/governance` 依赖 PATH 上的 `ponygo`，未版本化；建议后续把 oracle 收进 `tests/governance/tools/`（或 CI 安装钉版本），否则套件只在单机可信。
