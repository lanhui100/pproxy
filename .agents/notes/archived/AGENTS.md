# AGENTS.md —— 归档区冻结规则（自动加载）

> 进 `.agents/notes/archived/` 子树即加载本文。

## 冻结即法律

1. **永不编辑**：归档文件保持归档时原貌，不改一字（ mechanical 豁免，判据 1.8 冻结豁免）。
2. **不作现行权威**：引用 archived 结论前，先确认没有更新的 implemented 取代它；
   拿归档条当"当前就该这么做"的依据是状态误读。
3. **引用方式**：链路径 + 注明"已归档（冻结快照）"，不复制正文。

## 归档动作（把一条决策移进来时，同一次变更内做完）

**归档 = `git mv` + `Status:` 保持 + `seal` 同提交**——三件事是一次变更，缺一件即欠账：

1. `git mv <lifecycle>/<class>/<file>.md archived/<class>/<file>.md`（保血缘，别用 cp/mv 抹掉重命名事实）；
2. `Status:` 保持原态（多为 `implemented`；写 `archived` 也合规）——正文一字不改；
3. `bash .meta/gates/archive-verify.sh seal` 追加封印（append-only，manifest 是归档件的唯一索引）；
4. 三件事同一提交。漏 seal 会被 A1（暂存区）与 A4（全树）以「未封印」拒绝，CI 不放行。

## 恢复流程

确需恢复某条归档决策 → 按 write-adr 新建一条 implemented（链入归档原文），
走正常评审；**禁止把文件移回 implemented/**（移动即改写历史）。
