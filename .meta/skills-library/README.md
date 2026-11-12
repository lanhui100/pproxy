# skills-library/ —— 可选技能模板库（三层归属里的「模板库」）

本目录是 ponygo 的**可选技能模板库**：六个 `pg-` 前缀的通用流程技能以**模板**形态随
`init` 播种，但**不自动安装**——安装是用户显式执行的安装器动作，副本归实例所有、可自由改造。

> 本文件是这套契约（三层归属 / 三态 / marker / 不计 L3 / 安装用法）的**唯一家**：
> 安装器、CLI 与测试只引用本文，不复述规则。库技能正文的五要素契约在
> `.agents/skills/README.md`，此处不重复。

---

## 三层归属

| 层 | 位置 | 所有者 | 计入 L3 | `init` |
|---|---|---|---|---|
| 基线 | `.agents/skills/{write-adr,change-review,governance-review}/` | 实例可改，模板逐字节锁 | 否 | 必播 |
| 模板库 | `.meta/skills-library/pg-*/`（本目录） | 仅模板 | 否 | 必播、**不安装** |
| 实例自有 | 其余 `.agents/skills/*/` | 实例 | 是 | 不生成 |

- 库技能名一律带 `pg-` 前缀（本项目命名空间）；基线名不变。
- 库技能的**安装副本**（带 `.ponygo-installed`）是实例文件，但**不计入 L3**：
  `audit` 的「实例自有技能 ≥1」按归属判定，模板库副本与基线 3 技能同样被排除——
  否则一条安装命令就能买到 L3，等级失去含义。
- 一条技能落进哪一层只看**位置 + marker**，不看内容：带 marker 的副本永远是模板库副本，
  想让它成为实例自有资产，就按 Rule of three 自己改写/新写一篇（并删除 marker）。

## 文件清单

| 文件 | 职责 |
|---|---|
| `README.md` | 本文件：契约与用法（唯一家） |
| `install.sh` | 安装器：`list` / `install <name>` / `check`（位置子命令，零旗标） |
| `catalog.tsv` | 必要性元数据（`audit` B3 能力缺口建议的唯一数据源） |
| `pg-<name>/SKILL.md` | 六个库技能正文（模板） |

## 安装用法

```bash
bash .meta/skills-library/install.sh list                    # 每技能一行，查看可安装项与三态
bash .meta/skills-library/install.sh install pg-doc-hygiene   # 安装到 .agents/skills/pg-doc-hygiene/
bash .meta/skills-library/install.sh check                    # 只读：逐已安装项报三态
```

| 动词 | 行为 | 退出码 |
|---|---|---|
| `list` | 每技能一行：`<name>\t<installed\|absent>\t<ok\|stale\|modified\|->` | 0 |
| `install <name>` | 复制库技能文件到 `.agents/skills/<name>/`，成功后写 marker | 0 成功 / 1 目标已存在（零写入）/ 2 用法错、未知技能或无法写入 |
| `check` | 逐已安装项一行：`<name>\t<state>`（state = `ok` / `stale` / `modified`） | 0（只读、advisory，恒不改退出码） |

零旗标：`--list`、`-n` 一类旗标形态一律按未知输入拒绝（exit 2）。错误信息全部走 stderr；
`list` / `check` 的 stdout 只承载上表的制表符数据行（无副本时 `check` 的提示也只走 stderr）。
无参数调用同样是用法错（exit 2）——三个动词之外不设默认动作，也不替调用方猜意图。
写目标失败、取不到 sha256 工具或日期这几种**无法完成安装**的情形也归 exit 2，且回滚本次新建目录。

`list` 的技能集合 = `catalog.tsv` 登记的库技能 ∪ 库内 `pg-*/SKILL.md` ∪ 已装副本
（三者的并集、按名排序去重）：模板文件缺失时该技能仍列出（`absent` + `-`），缺口因此可见；
此时 `install` 会以 exit 2 指出库内缺 `SKILL.md`，修法见文末「获取与修复」。

安装语义：

- **永不覆盖**：目标 `.agents/skills/<name>` 已存在（普通文件、目录或符号链接）即拒绝，
  零写入。要换版本或放弃本地改造，由人 review 后手工移除副本再装——安装器不动实例文件。
- 复制只写新建的副本目录；**全部复制成功后才写 marker**，任何一步失败即回滚本次新建目录，
  不留"有副本无 marker"的半装状态。
- 复制源是库内该技能目录下的普通文件（当前为 `SKILL.md`）；库内技能目录不存在 →
  该名字是未知技能（exit 2），不会凭空创建。

## `.ponygo-installed` marker

安装成功后写在副本根，两行 `<key>=<value>`：

```
library_sha256=<安装时库内 SKILL.md 的 sha256>
installed_at=<yyyy-mm-dd>
```

- `library_sha256` 是**安装那一刻**模板的指纹，三态判定的基准，不是库的当前值。
- marker 存在 ⟺ 该目录是模板库安装副本（`audit` 归属判定与 `list` 的 `installed` 列都据此）。
- marker 损坏或缺 `library_sha256` 视为**不可校验**，判 `modified`（不是 `ok`）。

## 三态判定

按 `modified` > `stale` > `ok` 的顺序求值，先命中先返回：

| 态 | 判据 | 含义 |
|---|---|---|
| `modified` | 副本 `SKILL.md` 的 sha256 ≠ `marker.library_sha256`（含副本 `SKILL.md` 缺失、marker 无 `library_sha256`） | 实例改造过（或副本被删/被改），安装器不会碰它 |
| `stale` | 副本与 marker 一致，但**库内** `SKILL.md` 当前 sha256 ≠ `marker.library_sha256`（含库内文件已不存在） | 模板已更新，副本是旧版本 |
| `ok` | 副本、marker、库内当前值三者一致 | 副本与模板同步且未被改造 |

- 未安装的技能在 `list` 里输出 `-`（第四态，仅表示"无副本"，不是判定结果）。
- 三态都是**观测**，不是自动修复：`stale` 由人决定是否手工合并模板变更，`modified`
  由人决定是否放弃本地改造。`check` 决不写盘，也不改任何退出码。
- `audit` 对已装副本的三态只做 advisory：非 `ok` 报 `skill_library_stale` /
  `skill_library_modified` 提示，不计分、不影响退出码。

## catalog.tsv 与能力缺口建议

`catalog.tsv` 是制表符五列 `name / evidence_kind / evidence_arg / min / hint`，`#` 起注释。
`evidence_kind` 取封闭集合（`notes_class` / `archive_sealed` / `docs_scale` / `web_dirs` /
`test_dirs` / `release_surface` / `none`），求值口径见 `spec-ponygo-v2.6.md` §3.1。

`audit` 拿 `catalog.tsv × 证据` 求值，**未安装且证据达阈值**时打印"能力缺口建议"
（含证据摘要与安装命令）；已安装但证据不再成立时打印一行退场候选。建议是
**advisory，靠 review 判定，不计分、不影响退出码**——必要性是语义判断，不装假计数。

## 获取与修复

库文件属可选资产：`init` 必播、`upgrade` 不判 drift、缺失只提示。历史项目或删过库文件
的仓库用 `ponygo init --repair-skeleton` 补缺失库文件（只补不覆盖）。
