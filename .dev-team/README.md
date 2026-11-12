# .dev-team/ —— dev-team 执行态账本与物理收据（非任务看板）

> **任务真源是 `backlog/tasks.jsonl`**（`node scripts/backlog.mjs list`）。本目录**不是待办清单**：
> 它按 dev-team skill 的约定收存「某次会话的运行账本（state / NFR 基准 / 契约）与门禁收据
> （review / 相位记录 / 元复盘）」。要开新任务、看未完成事项、认领与闭环，一律走 `backlog/`。

## 布局约束：收据必须平铺，禁止移进子目录

`~/.agents/skills/dev-team/scripts/` 的两个收口脚本用**非递归 flat glob** 读收据，目录一挪即失效：

| 脚本 | 依赖 | 证据 |
|---|---|---|
| `dev-retro.py` | `<repo>/.dev-team/review-*.json` | `dev-retro.py:247`、`:695` 的 `.glob("review-*.json")`（非 `rglob`） |
| `dev-meta-lint.py` | `Path(复盘文件).parent` 下的 `review-plan*.json` / `review-join*.json` / `review-l3*.json` / `review-b*.json` | `dev-meta-lint.py:35-37` 的 `dev_team_dir = path.parent` |

实测（2026-11-12，临时克隆内，未动本仓）：把 `review-*.json` 移入 `.dev-team/archive/` 后
`dev-retro.py receipt-gate` 的 `receipts_total` 由 9 归零并 exit 1，`dev-meta-lint.py` 报
「缺失计划阶段/实施阶段收据」exit 1；保持平铺时 `python3 ~/.agents/skills/dev-team/scripts/dev-meta-lint.py
.dev-team/meta-retrospective.md` → exit 0 PASS。

推论：**已完成会话的收据也留在本目录根**，用「文件名 + 本表 + git 历史」表达归属，
不用目录分层——分层会打断 Phase 5 收口门。要清理时只删**无引用且无证据价值**的残渣，
不搬收据。

## 现存文件归属（都是历史收据；新会话覆盖同名文件）

| 文件 | 会话 / 用途 | 活引用 |
|---|---|---|
| `contract.md` | 冻结契约（C1–C6 + 门禁 G1–G8） | `tests/acceptance/{run.sh,lib.sh,helpers/yaml_contract.py,README.md}` 声明的「合同唯一依据」 |
| `nfr-baseline.json` | NFR 商业基准（accept-loop：超时 3000ms / 重试退避 / 并发锁 / 结构化日志） | `crates/server/tests/saturation_contract.rs` 与 ADR 2026-10-09 以 `nfr-baseline-pproxy-accept-loop.json` 之名引用同一物（文件名不一致，属既存断链） |
| `governance-state.json` | governance-remediation-2026-10-30 会话账本（5 波） | `.agents/notes/rejected/process/2026-10-30-history-rewrite-rejected.md` 作证据 |
| `state.json` | clash-meta-scan-no-proxy 会话账本（5 波全 completed，无在飞波次） | 无（冷启动只读同名文件，此处表示「上次会话已收口」） |
| `review-l3a.json` / `review-l3b.json` / `review-plan.json` | 该会话的 L3 对抗与方案审查（verdict=FAIL 的 advisory 清单） | `backlog/tasks.jsonl` 的 `B016`（未清 advisory 的登记源，勿移动） |
| `review-plan-governance.json` / `review-l3p.json` / `review-join-{a,b}.json` / `review-join-wave2-{a,b}.json` | 各阶段 PASS 收据 | `governance-state.json` 的收据注册表 |
| `red-phase-legacy.md` / `green-phase-legacy.md` / `red-evidence/legacy-items-red.txt` | 2026-10-08 legacy 缺陷红相/绿相机器记录 | 无（对应事项已在账本闭环：B024 等） |
| `meta-retrospective.md` / `meta-retrospective-2026-10-08-*.md` | 各会话元复盘（Phase 5 产物） | `dev-meta-lint.py` 默认读 `.dev-team/meta-retrospective.md` |

## 已知断链（既存债，非本目录职责）

`crates/server/tests/{relay_dead_tunnel,adversarial,saturation_contract}.rs` 与
`deploy/watchdog/README.md` 引用的 `.dev-team/contracts/2026-10-09-pproxy-accept-loop.md`、
`.dev-team/nfr-baseline-pproxy-accept-loop.json` 从未入库（ADR 2026-10-19 已自述「未随仓库提交」）。
这些是注释里的语义指针，不影响任何门禁；修它的正确做法是补文件或改注释，靠 review 决定。
