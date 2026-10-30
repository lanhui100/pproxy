# docs-tier/ —— 文档分层（激活状态与家清单索引）

> **口径真源**：tier 分类法、每个 tier 的家、写作规则、slop checklist 一律以 `docs/AGENTS.md` 为准
> （agent 自动加载的家）。本文件只回答两件本目录独有的问题：**分层是否已激活**、**哪些家已就位**。
> 不在此复制 tier 表——同一条规则两个家 = 迟早漂移（`docs/AGENTS.md` Slop checklist 第 1 条）。
>
> 激活声明：`.meta/meta.yaml` 的 `docs-tier: on`（判据 2.5：声明 on 即承诺分层已落地且无占位残留）。

## 已激活文档家清单

本仓文档分层已激活。各 tier 的家在 `docs/AGENTS.md` §Tier 分类法（唯一真源），此处只列就位状态：

| tier | 家（已就位） | 状态 |
|---|---|---|
| 根 AGENTS.md（常载 standing orders） | `AGENTS.md`、`CLAUDE.md`（`ponygo sync` 投影同源） | ✓ |
| 文档标准 | `docs/AGENTS.md` | ✓ |
| 架构地图 | `docs/architecture/CURRENT.md` | ✓ |
| 决策记录（活动） | `.agents/notes/{proposed,implemented,rejected,archived}/` | ✓ |
| 决策记录（历史存档，冻结只读） | `docs/architecture/decisions/`（001–008，不再新增） | ✓ 冻结 |
| 包/模块契约 | `crates/{cli,core,engine,gate-server,server,transport}/README.md`、`desktop/README.md` | ✓ |
| how-to（运行手册） | `docs/ops/`（DEPLOY / ROLLING-UPGRADE / TROUBLESHOOTING / DESKTOP-TROUBLESHOOTING / MUSE-REMOTE-CONNECT / CF-RISK-SOP） | ✓ |
| 接口契约 | `docs/ops/API.md` | ✓ |
| 事故复盘 | `docs/ops/stability-report-2026-09-29.md` | ✓ |
| 用户文档 | `README.md`、`README.en.md`、`desktop/README.md` | ✓ |

**放置速查**（与 `docs/AGENTS.md` 同口径，此处只留最常用四条）：bug→事故复盘；理由→决策记录；
过程→how-to；契约→包 README。

## 本目录不承载的事

- **tier 分类表与写作规则**：在 `docs/AGENTS.md`（真源；改口径只改那里）。
- **词数/体积门禁（doc-sync 门）**：**未实现，靠 review**——本仓无 `scripts/check-docs-budget.sh`，
  pre-commit 与 CI 都没有词数判定；经验阈值（>1000 词的文档考虑拆分）只在 `docs/AGENTS.md` 声明，
  不产生退出码。把它写成"已启用"就是虚假安全感。
- **包文档豁免清单**：若某包确需豁免，写 `.meta/docs-tier/exemptions`（判据 1.9 的读取面），不写进本表。
