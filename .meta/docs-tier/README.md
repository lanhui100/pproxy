# docs-tier/ —— 文档分层契约（按 tier 分类法给每个事实安一个家）

## 已激活文档家清单

本项目已正式激活文档分层管理。各 tier 职责明确，家载体分工如下：

| tier | Job（该承载） | 不承载 | 本项目家（待填 / 骨架已生成标 ✓） |
|---|---|---|---|
| 常载命约 | 每次会话必载的 standing orders，1-3 行每条，链到 home | 故事/示例/复述 | `AGENTS.md` ✓（sync 投影） |
| 文档标准 | tier 分类 / 写作规则 / slop checklist | 具体文档正文 | `docs/AGENTS.md` ✓（init 播种） |
| 架构地图 | 组合、核心模块、接缝、扩展点（有序地图） | 类型细节/决策理由/状态标注 | `docs/architecture/` ✓ |
| 决策记录 | 活动决策（当前态现在时） | 迁移计划/验收清单 | `.agents/notes/` ✓ |
| 事故复盘 | 事故年表、证据、因果链、预防 | 教学叙事 | `docs/ops/` ✓ |
| how-to | 带编号验证步的操作指引 | 设计理由（→ 决策记录） | `docs/dev-team/` ✓ |
| 用户文档 | 产品面向指南 | 贡献流程/决策史 | 根目录 `README.md` ✓ |
| 包/模块契约 | 单模块配置/语义/限制/扩展点 | 逐行注释复述 | 各包 `crates/*/README.md` ✓ |
| 生成参考 | 从源码再生成的参考 + 新鲜度门禁 | 手编生成源 | `desktop/` 等独立端点 ✓ |

**放置速查**：bug→事故复盘；理由→决策记录；过程→how-to；契约→模块 README；
standing orders→常载命约 + 理由链。

## slop checklist（审计清单，原理：methodology §5.4）

- 同一条规则出现在多个家（留一个家，其余链过去）；
- 叙述历史/战争故事（previously/now/no longer——状态会腐烂）；
- 实现状态标注（"implemented!"/"future:"——布局与 manifest 携带状态，散文不携带）；
- 手抄目录/逐行注释复述（源码或生成器是权威）；
- 段落墙、强调通胀（到处都是 bold = 没有强调）。

## 词数预算（doc-sync 门，L2 选装）

standing-doc 设上限，超限按 **relocate → condense → raise** 顺序处理；
上限是护栏不是压减目标，目标线下保留至少 5% 余量。
