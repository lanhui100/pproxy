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
| 账本记录合法且跨归档/租约对账干净 | `bash .meta/gates/backlog-gate.sh` | `lint`（必填字段/条件必填/状态字段合法性/租约-状态互斥/活跃池 done 残留/ID 唯一/高水位）+ `doctor`（ID 重复/高水位/孤儿租约/租约-状态互斥） |
| 门自己会拒绝非法样本 | `bash .meta/gates/backlog-gate.spec.sh` | P1 放行 + N1–N7（缺 fingerprint / 活跃 done / 孤儿租约 / 高水位越界 / 无账本跳过 / 无 node 拒绝 / 锁存而记录非 in_progress）+ H（pre-commit 接线：门在场放行 / 门缺失 fail-closed） |

## 契约与真源

- 可读契约：`backlog/README.md`（布局/字段/状态机/租约/归档/命令）
- 判定真源：`scripts/backlog.mjs`（`validateRecord` / `lint` / `doctor`）——本目录脚本只编排，不复刻判据
- 诚实边界：spec 只证明「门会拒非法样本」；账本内容真实性靠 review。`doctor` 不解析 git 历史，
  无法识别「同时改记录与高水位」的伪装。
- 本地钩子前提：`git config core.hooksPath .githooks`（git 不随克隆携带该配置；本仓已按既有方式配置）。
  未装配时本地门禁静默失效，CI 的 `backlog-ledger` job 仍兜底——这是 git 的机制限制，不是门禁缺陷。
