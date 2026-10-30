# docs/AGENTS.md —— 文档标准（每个事实一个家）

> 本文是文档体系的**家索引**：tier 分类法与「每个 tier 的家在哪」的**唯一真源**。
> 文档治理规则的家载体是 **AGENTS.md**（agent 处理 docs/ 子树时自动并入上下文）；
> **README** 给人读（根 README + 各包 README 承载 config/semantics/limitations），
> agent 不主动读不进上下文——两者分工，不混用（原理：methodology §5.1，实证 dsh）。
> `.meta/docs-tier/README.md` 只记录「分层是否已激活 + 家清单索引」，**不再自带一份 tier 表**
> （同一张表两个家 = 迟早漂移，见 Slop checklist 第 1 条）。

## Tier 分类法（家清单为当前态；改这里即改口径，其他文件只链不抄）

| tier | Job（该承载） | 不承载 | 本项目家 |
|---|---|---|---|
| 根 AGENTS.md | 常载 standing orders（每次会话必载，1-3 行每条，链到 home） | 故事/示例/复述 | `AGENTS.md` ✓（`ponygo sync` 投影，与 `CLAUDE.md` 同源） |
| 文档标准 | tier 分类 / 写作规则 / slop checklist | 具体文档正文 | `docs/AGENTS.md` ✓（本文件） |
| 架构地图 | 有序地图：组合、核心模块、接缝、扩展点 | 类型细节/决策理由/状态标注 | `docs/architecture/CURRENT.md` ✓ |
| 决策记录 | 活动决策（当前态现在时） | 迁移计划/验收清单 | `.agents/notes/{proposed,implemented,rejected,archived}/` ✓（历史存档 `docs/architecture/decisions/` 冻结只读，见 `.agents/notes/implemented/process/2026-10-30-alt-governance-dual-track.md`） |
| 包/模块契约 | 单模块配置/语义/限制/扩展点 | 逐行注释复述 | `crates/{cli,core,engine,gate-server,server,transport}/README.md` ✓、`desktop/README.md` ✓ |
| how-to | 带编号验证步的操作指引 | 设计理由（→ 决策记录） | `docs/ops/`（`DEPLOY.md`、`ROLLING-UPGRADE.md`、`TROUBLESHOOTING.md`、`DESKTOP-TROUBLESHOOTING.md`、`MUSE-REMOTE-CONNECT.md`、`CF-RISK-SOP.md`、`API.md`）✓ |
| 接口契约 | 端点/请求响应形状/错误码 | 设计理由（→ 决策记录） | `docs/ops/API.md` ✓ |
| 事故复盘 | 事故年表、证据、因果链、预防 | 教学叙事 | `docs/ops/stability-report-2026-09-29.md` ✓（下一篇按同一命名：`docs/ops/<incident>-report-<date>.md`） |
| 用户文档 | 产品面向指南（安装/使用/配置项） | 贡献流程/决策史 | 根 `README.md` ✓、`README.en.md` ✓、`desktop/README.md` ✓（桌面端用户视角） |

**放置速查**：bug→事故复盘；理由→决策记录；过程→how-to；契约→包 README；
standing orders→根 AGENTS.md + 理由链。

**新增文档时先问**：这条事实属于哪个 tier？现有哪个家能承载？——能塞进已有家就不新建文件；
新建前先在本表登记（本表漏登记 = 该文档没有家，review 时按违例处理）。

## 写作规则

- **当前状态，不写变更历史**：durable prose 禁 "previously/now/no longer"、禁 PR/提交/栈位；
  变更故事进 commit/PR/决策记录/postmortem；
- **文档与代码同提交**：改行为/契约/配置的那次提交必须同步更新对应文档的家
  （dsh same-commit 规则）；纯机械/局部编辑豁免；
- 注释与 JSDoc 陈述**完整契约**（行为/失败/时序/归属/模态/例外/后果），删叙事/走查/复述。

## Slop checklist（审计清单）

- 同一条规则出现在多个家（grep 特色短语，留一个家链其余）；
- 叙述历史/战争故事（previously/now/no longer/renamed/PRs/commits）；
- 实现状态标注（"implemented!"/"future:"——状态会腐烂，布局与 manifest 携带它）；
- 手抄目录/JSDoc/测试包状态清单（源码或生成器是权威）；
- 段落墙、强调通胀（bold 到处都是 = 没有强调）。

## 词数预算（doc-sync 门）——**未实现，靠 review**

本仓**没有**词数/体积门禁脚本（无 `scripts/check-docs-budget.sh`，pre-commit 与 CI 均不判）。
现状口径：超大文档（经验阈值 >1000 词）应拆到它的家或压缩，由 review 提示，不产生退出码。
若将来要变成真门禁，须同时落「脚本 + 负样本 spec + 挂载点」，否则宁可维持本段的诚实声明
（把没接线的承诺写成门禁 = 虚假安全感，P6）。
