# 元框架复盘：clash-meta-scan-no-proxy（C 级）

## 1. 通信拓扑与信噪比（Topology & Noise）

- **名册**：Lead + 3 Executor（gate-connect / clash-config / desktop-lan）+ 1 Test Agent（qa）= 5，
  审查一律走 Subagent（2 路 L3），未挤占常驻槽位。符合 Roster ≤ 8 与拓扑公理。
- **探索者外派**：2 份 E1 侦察在立项阶段并行下发（公网 gate 部署面 / 桌面引擎监听面），
  各回 25 与 10 条带 `path:line` 的事实条目；Lead 只做定点复核（CONNECT 实测、8899 进程归属），
  未把侦察原始输出吞进主上下文。**这是本会话信噪比最高的动作**——两条根因各自 5 分钟内定论，
  且后续三个 Executor 的 prompt 直接复用这些事实，没有一个人重复调研。
- **零废话协议**：三个 Executor 的回报全部为「文件表 + 验收命令原始输出 + 遗留风险」三段式，
  无客套、无过程复述。Test Agent 主动做了**自查纠错**（发现 `run_helper` 管道吞掉非零退出码
  导致 G7-desktop 假绿），并把它固化成常驻门禁 `HARNESS-SELFTEST`——这是本次最有价值的单点产出。
- **重复命令率**：低。唯一的重复是 `cargo test --workspace` 跑了 3 次（基线 / 全量 / 隔离验证），
  每次目的不同（基线对照、回归、暂存态隔离验证），属门禁必需而非浪费。

## 2. 门禁穿透与误杀率（Gate Penetration / False Negatives & Positives）

- **真实拦截**：验收套件 R2 变异测试（把 `port 8443` 改回 443 / 把 Basic 改回 Bearer /
  把 `MATCH,PROXY` 改回 `MATCH,DIRECT`）三处**全部被咬住**——这证明断言不是"跑得过就行"的
  空转绿灯，而是真的钉在契约上。另加 `META-AT1-NEGCTRL` 负控制（哑服务必须被判红），
  堵住"helper 自己坏了导致假绿"的通道。
- **一次真实假绿被抓住**：Test Agent 首版 helper 管道吞退出码 → 我复核时该用例显示 PASS 但
  内部输出含 `FAIL:` 字样。**教训**：`[PASS]` 字样的存在本身不是证据，必须验"断言失败能否
  真的让脚本非零退出"。此后该模式被固化为常驻自检用例。
- **门禁穿透（本会话真实缺口）**：C6（兜底 `MATCH,DIRECT → MATCH,PROXY`）在派单时漏派给
  desktop-lan，只写进了共享契约文件。desktop 侧的 C6 是在 Test Agent 首轮报红后由 Lead 补发 CR
  才修好的。**根因是"契约文件写了 ≠ 每个 Executor 都拿到了"**——我依赖 Executor 自己读契约，
  但三个 Executor 的 prompt 里我只逐条点名了自己认为相关的那几条。改进项见第 4 节。
- **误杀率**：0。L3 审查未产生需要驳回的误判（两路 findings 见 `.dev-team/review-l3*.json`）。

## 3. 分工契约与隔离有效性（Contract Isolation）

- **写域正交生效**：gate-server 的 `lib.rs`（W2）与 `main.rs`/`connect_proxy.rs`（W1）被切开后，
  两个 Executor 全程零冲突；期间出现一次真实边界争议（`src/tests.rs` 归属），Executor 主动
  上报而不是自行扩大写域，Lead 一票裁决后归属清晰。**这正是 advisory 写域应有的行为**。
- **测试分权真实有效**：Test Agent 全程只写 `tests/acceptance/`，从未被允许改实现；
  它报出的唯一红项（desktop C6）反过来驱动了实现方修复，而不是它自己去改代码。
- **Lead 写入边界**：Lead 只写了 ADR、契约、`docs/ops/*`、README 单 hunk 与部署动作，
  没有向业务代码写一行逻辑。业务代码 100% 由 Executor 产出。
- **生产部署作为门禁**：真机验收不是"本地跑通就算"，而是把 8443 打通后用 **Mihomo v1.19.2**
  真实加载生产下发的 YAML —— 这一步抓到了所有本地测试抓不到的东西（ufw 静默丢包、
  nginx conf.d 上下文非法、stream 动态模块缺失）。

## 4. 元协议迭代建议（Self-Evolving Protocol）

1. **契约分派必须逐任务点名，不能只写共享契约文件**。建议在任务书模板里加一条硬要求：
   `契约条目覆盖清单`——每个 Executor 的 prompt 必须显式列出"本条任务负责 C1/C3/C6 中的哪几条"，
   Lead 在栅栏前机械比对契约条目并集 == 全部冻结条目。本次 C6 漏派就是缺这道机械比对。
2. **"扫码/导入成功"类假信号应立为通用审计项**：凡是下发配置的端点，其"可被消费"必须与
   "可被使用"分开断言。本次订阅端点不校验 token 导致 200 假成功，是典型的可用性假信号。
   建议在 B/C 级任务识别清单里加一条：*分发面必须校验凭据，且失败要显式可见*。
3. **部署类任务的验收必须包含"环境前提"断言的独立用例**：本次 ufw 默认 DROP 只放行 22/80/443，
   新端口静默超时且 nginx 零日志——本地测试无法覆盖。建议在 DEPLOY 模板里固定"新端口三件套"：
   `ss`（本机监听）+ `ufw status`（安全组）+ 从外部打一发（真实可达）。
4. **隔离验证应固化**：本次用 `git apply` 到临时 worktree 验证"暂存区独立可编译"，
   解决了脏工作区（多会话并行）下"提交内容是否自洽"无法证明的问题。建议把它写成脚本
   （`dev-verify-staged.sh`），成为收口标准动作。
