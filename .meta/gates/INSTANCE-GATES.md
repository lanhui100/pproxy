# 实例门登记（本仓自有；不属框架骨架文件）

> 为什么单独一个文件：`.meta/gates/README.md` 在 ponygo 的 `UPGRADE_UNIVERSE` 内（逐字节锁定），
> 往里追加实例门会让 `ponygo upgrade --check` 长期报漂移（实测 rc=1）。框架 README 只登记 A1–A9；
> 实例门与「承诺 → 命令」写在这里，既不制造骨架漂移，也不把门禁藏进无人看的地方。

| 门 | 脚本 | 负样本 spec | 接线 | 退出语义 |
|---|---|---|---|---|
| backlog 账本门 | `.meta/gates/backlog-gate.sh` | `.meta/gates/backlog-gate.spec.sh` | pre-commit（哨兵块之外）+ `.github/workflows/ci.yml` 的 `backlog-ledger` job | 0 放行 / 非零拒绝；账本缺失 → 明确跳过；无 `node` → 拒绝 |

## 承诺 → 非零退出命令

| 承诺 | 命令 | 命中判据 |
|---|---|---|
| 账本事件/记录合法且跨批次归档对账干净 | `bash .meta/gates/backlog-gate.sh` | `lint`（batch.start 首行/事件种类/事件时间非递减/记录字段与条件必填/重复 record/租约-状态互斥/布局一致）+ `doctor`（fold 视图 ID 重复/孤儿租约/归档与清单对账）+ `verify-archive`（sha256 封存、未入清单） |
| 门自己会拒绝非法样本 | `bash .meta/gates/backlog-gate.spec.sh` | P1–P3 放行（单批次 / 双批次并发 / v2 过渡只读）+ N1–N8（首行非 batch.start / 未知事件 / v2 缺 fingerprint / 归档被改 / 归档未入清单 / 锁存而记录非 in_progress / 有 log 缺 MODEL / 无 node）+ H（pre-commit 接线：门在场放行 / 门缺失 fail-closed） |

## 契约与真源

- 可读契约：`backlog/README.md`（布局/字段/状态机/租约/归档/命令）
- 判定真源：`scripts/backlog.mjs`（`validateTask` / `lintV3` / `fold` / `doctor` / `verifyArchive`）——本目录脚本只编排，不复刻判据
- 诚实边界：spec 只证明「门会拒非法样本」；账本内容真实性靠 review。`doctor` 不解析 git 历史，
  无法识别「同一会话内既改事件又改归档清单」的伪装（归档清单只能追加，改动由 `verify-archive` 抓）。
- 本地钩子前提：`git config core.hooksPath .githooks`（git 不随克隆携带该配置；本仓已按既有方式配置）。
  未装配时本地门禁静默失效，CI 的 `backlog-ledger` job 仍兜底——这是 git 的机制限制，不是门禁缺陷。
