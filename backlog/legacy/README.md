# backlog/legacy/ —— 冻结历史（只读）

本目录保存旧账本的单文件 markdown 看板与迁移映射。**只读、不参与校验**：
`scripts/backlog.mjs` 的 `lint` / `doctor` 不读本目录，历史内容一律不改写。

| 文件 | 角色 |
|---|---|
| `backlog.md` | 旧单文件看板（原文冻结，一字不改） |
| `README.md` | 本文件：旧锚点 → 账本记录的映射表 |

旧路径 `backlog/backlog.md` 的东西现在在此处的 `backlog.md`：
历史文档（`docs/dev-team/reviews/`、`.agents/notes/`）里指向旧路径的行号引用保持原样，按本表换算即可。

## 映射表（旧锚点 → 账本记录）

| 旧位置 | 旧编号 | 旧标题（截断） | 账本记录 | 迁移后状态 |
|---|---|---|---|---|
| `backlog.md:7` | B010 | gate 多出口兜底 P0-1 | `B010` | `in_progress`（代码+单测已落地，待部署验收） |
| `backlog.md:23` | B017 | 发布通道命名治理 | `B017` | `todo` |
| `backlog.md:37` | B018 | macOS 桌面版发版评估 | `B018` | `todo` |
| `backlog.md:64` | B019 | 会员生命周期运营缺口 | `B019` | `todo` |
| `backlog.md:78` | B020 | pre-push 治理门 G4 长期红 | `B020` | `done`（归档） |
| `backlog.md:94` | B021 | 生产 gate 验签公钥不配对 | `B021` | `todo` |
| `backlog.md:108` | B011 | gate 伪装页 + 反指纹 P0-2 | `B011` | `done`（归档） |
| `backlog.md:118` | B012 | CF 请求量自省 + 配额联动 P0-3 | `B012` | `done`（归档） |
| `backlog.md:131` | B013 | 部署形态 + 风控 SOP P1-4 | `B013` | `done`（归档） |
| `backlog.md:140` | B014 | 标准协议入站 P1-5 | `B014` | `todo`（proposed ADR 在案） |
| `backlog.md:148` | B015 | 客户端配置细节 P2-6 | `B015` | `done`（归档） |
| `backlog.md:161` | B003 | vedge 改 CF 橙云代理回源 Vercel | `B003` | `todo`（待决策） |
| `backlog.md:172` | B002 | Vercel 出口超时排查与恢复 | `B002` | `todo`（blocked-by-external） |
| `backlog.md:193` | B001 | CF 恢复后重测 S1/S2 spike | `B001` | `todo`（blocked-by-external） |
| `backlog.md:208` | B002 | M6 实现收尾 | **`B022`** | `done`（归档） |
| `backlog.md:211` | B003 | 未来拓展池（按需启动） | **`B023`** | `todo` |
| `backlog.md:217` | B004 | 白名单代理多出口容灾（按需启动） | `B004` | `todo` |
| `backlog.md:222` | B005 | 跨节点配额/用量一致性 #F-01 #F-02 | `B005` | `todo`（proposed ADR 在案） |
| `backlog.md:230` | B006 | 撤销跨节点传播 #F-03 | `B006` | `todo` |
| `backlog.md:236` | B007 | 节点身份标识注入机器名 #F-04 | `B007` | `todo` |
| `backlog.md:242` | B008 | 零停机滚动升级真实编排 #F-05 | `B008` | `todo` |
| `backlog.md:248` | B009 | 跨发行版 GLIBC 兼容 #F-06 | `B009` | `todo` |
| `backlog.md:254` | B010 | CF worker 标记头与 PROXY_SECRET 对齐上线 | **`B024`** | `done`（归档） |
| `backlog.md:260` | B016 | CONNECT 数据面 L3 审查遗留 | `B016` | `todo` |

## 两处口径说明（迁移时的裁决，不是原文的事实）

1. **拆号**：旧看板有三组「同号不同义」——`B002`（:172 Vercel 出口 / :208 M6 收尾）、
   `B003`（:161 vedge 橙云 / :211 未来拓展池）、`B010`（:7 gate 多出口 / :254 CF worker 标记头）。
   保留跨文档引用指向的那一条（:172 / :161 / :7），另一条改号 `B022` / `B023` / `B024`。
2. **状态按事项内容判，不按旧看板分区判**：旧「已完成」区实际混着未来池（B003→B023、B004）、
   仅出方案的提案（B005）与待办（B006–B009、B016），迁移时按内容落 `todo`；
   只有原文带完成标记或明确写「已完成」的事项（B011/B012/B013/B015/B020/B024）落 `done`。
   唯一例外是 `:208`（B022）：标题仍写「🔄 进行中」，但位于「已完成」区，且其交付物有机器旁证
   （`desktop-v0.3.0` 标签在场，其后还有 66 个 `desktop-v*` 连续发版）——故判 `done`，
   证据里明写这条判定依据而不是冒充原文完成标记。
