# instance-commitments.md —— 本仓实例承诺 → 非零退出命令

> 本表是**实例侧**的「承诺 → 命令」映射。`.meta/gates/README.md` 是框架模板文件
> （与框架仓逐字节一致、`ponygo upgrade --check` 会比对），因此实例自己的承诺不写进去，
> 统一落在这里——`ponygo init` 重跑也不会覆盖，且不引入骨架漂移。
>
> 判据（P2）：凡机械可查的承诺，必须给出一条**能非零退出的命令**；给不出命令的，写进
> 最后一节「靠 review」并说明为什么做不成门禁（P6 诚实条款：不把 review 项伪装成门禁项）。

## 1. 承诺 → 命令

| # | 承诺 | 命令（判定本体 = 唯一真源） | 触发点 |
|---|---|---|---|
| 1 | 凭据不进版本库（索引 + HEAD 树；含 admin/gateway/tenant 令牌与上游共享密钥） | `bash .meta/gates/secret-scan.sh` | `.githooks/pre-commit` 第 0 段（秒级）；`.github/workflows/ponygo-governance.yml` 一步。负样本自检：`bash .meta/gates/secret-scan.spec.sh` |
| 2 | 桌面发版前：版本三处一致 + 前端类型/单测/构建 + 产物 UI 关键字符串在场 | `bash scripts/release-gate.sh <version>` | `.github/workflows/desktop-release.yml`（发版前：`bash scripts/release-gate.sh "$VER"`）；本地人工发版前自检。**注意**：脚本支持的 `--feed`（发版后线上 feed 双通道校验）**没有任何 workflow 接线**，见 §3「靠 review」 |
| 3 | 合规出口 host 清单在 Rust 数据面与 CF gate worker 两侧一致（ADR-008） | `bash scripts/check-egress-parity.sh` | `.github/workflows/ponygo-governance.yml` 一步（零网络依赖：纯文本比对两侧常量）；本地可随时手跑 |
| 4 | 桌面更新 feed 通道可达且版本匹配（防 CLI 发布抢占 GitHub latest） | `bash scripts/check-update-feed.sh --github-only <version>` | `.github/workflows/desktop-release.yml`、`.github/workflows/cli-release.yml`；本地 `--github-only` 可复现 CI 结论 |
| 5 | 归档决策一旦封印，内容不得静默改写 | `bash .meta/gates/archive-seal.sh --check` | ①`.github/workflows/ponygo-governance.yml` 一步（**直接**调用）；②`.githooks/pre-push` 经 `tests/governance/run.sh` 的 G14 用例**间接**调用——pre-push 本身不直调本脚本，且当套件缺失、pre-push 走降级路径（`secret-scan`+`verify-note`）时**不含**封印校验。写端：`bash .meta/gates/archive-seal.sh --write`（只在归档动作同一提交执行） |
| 6 | 治理验收套件在**固定 oracle** 下全绿（G0–G17：ADR 1.10 / 凭据门 / 封印 / 取代链 / 文档家 / 承诺表） | `bash tests/governance/run.sh` | ①`.github/workflows/ponygo-governance.yml`：先按框架仓 commit `7055ced122bec5112bc7a9ac5879784e2eecf989` 安装 `ponygo`（**不跟 main**，防上游漂移），再跑套件；②`.githooks/pre-push` 主判据（约 4s）。AI/人工本机复跑同样有效 |

**共同纪律**：这些命令都是「非零退出 = 拒绝」语义，且**判定本体只有一份**——pre-commit /
pre-push / CI 都只**调**脚本，不复制判据（P9：同一判据两份实现必然漂移）。

## 2. 为什么第 2/4 条只在发版流触发

它们是**发布语义**的承诺（版本号、更新 feed、产物字符串），只在"发版"这个动作里有意义；
放进 PR 流会变成对每个改动的分钟级空转（P7 时间经济性）。PR 流里只放全树秒级检查（第 1/3/5 条）。

## 3. 靠 review（做不成门禁的承诺）

| 承诺 | 为什么无命令 | 谁来守 |
|---|---|---|
| 改行为/契约的那次提交同步更新对应文档的家（same-commit） | "文档是否覆盖了这次行为变化"是语义判断，正则判据会被拆词绕过；判据在 `.githooks/pre-commit` 里只有启发式 WARN（不改退出码） | review + `.agents/skills/change-review` |
| 新增决策与旧决策的取代关系（supersession） | 需要读懂两条 note 的语义重叠；机械侧只能查"字段在不在"（`Supersedes:` 行），查不出"该不该取代" | review + `write-adr` 的 supersession 分支 |
| 文档分层放置语义（doc-sync） | 内容"放对家"需理解 tier 语义；词数门本仓**未实现**（见 `docs/AGENTS.md` §词数预算） | review |
| 发版后线上 feed **双通道**校验（`release-gate.sh --feed`） | 脚本有该分支但**任何 workflow 都没接线**：CI 只跑 `check-update-feed.sh --github-only`（access 通道由网关主机手动 `sync-desktop-release.sh` 更新，发布时刻可滞后，不能作硬门禁）。要变成门禁须先解决该通道的时序 | review + 发版人自查；接线时补一条 decision |

## 4. 维护方式

新增/删除承诺时同时改本表与触发点（pre-commit / pre-push / workflow），并在对应决策 note 里
说明「为什么这条承诺值得一个非零退出命令」。只加承诺不给命令 = 口号，本表不接受这种行。
