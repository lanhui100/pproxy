# PProxy 分布式商业化实施 — 遗留问题与后续工作清单

> 所属规格：`docs/product/specs/distributed-commercialization/README.md`（v1.1.0）
> 本文记录 Phase 1~5 落地与 2026-09-25 preprod 联调后仍未闭合的差距。
> 事项层登记见 `backlog/backlog.md`（B 编号引用本文条目标识 `#F-xx`）。
> 原则：每一条都含**现象 / 根因 / 影响 / 建议方案 / 验收口径**，杜绝"以后可能用得上"式怀旧——只有明确差距与恢复条件才入册。

---

## 一、总体状态

| 能力面 | 状态 | 证据 |
| --- | --- | --- |
| 多租户令牌（Ed25519 签发/验签） | ✅ 已闭合 | `pproxy user add` → gate-server Profile 200 |
| 令牌撤销闭环 | ✅ 已闭合（联调期修复） | revoke → 网关热更新 → Profile 401 Token Revoked |
| 配额 Profile 展示 | ✅ 已闭合 | 返回 quota/used/expire/max_conns |
| 节点在线/离线探测 | ✅ 已闭合（管理面实网探测） | dev 停机 → 离线(✗)；恢复 → 在线(✓) |
| 零停机滚动升级 | ⚠️ 骨架已落地，真实滚动编排未联调 | 见 #F-05 |
| 跨节点用量/并发一致性 | ⚠️ 未实现 | 见 #F-01 / #F-02 |
| 撤销跨节点传播 | ⚠️ 单机热更新已通，集群广播未实现 | 见 #F-03 |
| 多节点身份标识 | ⚠️ 未注入机器名 | 见 #F-04 |

---

## 二、遗留问题明细

### #F-01 — 跨节点配额/用量一致性（信用租约未落地）
- **现象**：`gate-server` 的 `user_used_bytes` 为节点本地内存 `DashMap`；用户流量在节点 A 消耗后，节点 B 无法感知。
- **根因**：Phase 2 设计中的"局部信用租约 (Local Credit Leases)"未实现；当前是单机原子累加，无跨节点同步。
- **影响**：
  - 同一用户在多节点并发使用时，配额可被放大（无界透支窗口）；
  - 节点 A 宕机后，节点 B 看不到 A 已累计的用量，配额判断失真。
- **建议方案**（设计已在 v1.1.0 spec §1 记录）：
  1. 令牌 Claims 已含 `lease_bytes`，先落"单节点按租约切片放行"；
  2. 节点间增量用量 Gossip（`/internal/cluster/sync` 端点 + 周期对账）；
  3. 终极形态：配额租约预分配 + 402 截断（spec §3.1）。
- **验收口径**：同一令牌在两个节点先后各消耗 5G（总配额 8G），第二个节点在用量到达 8G 时返回 402。
- **登记**：backlog B005

### #F-02 — 单用户并发限制（max_conns=3）跨节点穿透
- **现象**：`user_active_conns` 为节点本地计数；用户可同时连 nodeA 3 条 + nodeB 3 条。
- **根因**：无跨节点连接槽位协调（spec 中的"哈希锚点/Affinity Anchor"方案未实现）。
- **影响**：max_conns 授权约束在多节点下形同虚设。
- **建议方案**：按 spec 采用 UID 一致性哈希锚点或客户端接入点亲和性（Cold-Standby 不散布并发）。
- **验收口径**：同一令牌在 A、B 两节点合计在线连接 >3 时，第 4 条返回 429。
- **登记**：backlog B005

### #F-03 — 撤销（revoke）未跨节点传播
- **现象**：`pproxy user revoke` 已实现"本机落盘 + 本机 gate-server 热更新"闭环；但集群内其他节点（dev / RackNerd / 未来新节点）不会同步该撤销。
- **根因**：撤销表 `revoked_tokens` 是各 gate-server 进程内存态；`revoked_tokens.txt` 为单机文件，无集群广播。
- **影响**：被撤销令牌仍可在未收到撤销的节点上继续使用（撤销真空期）。
- **建议方案**：
  1. 短期：将 `revoked_tokens.txt` 纳入集群 Gossip 反熵（增量同步）；
  2. 中期：撤销走管理面广播（eager fanout + ACK），并绑定令牌 `exp` 自动清理。
- **验收口径**：节点 A revoke 后，节点 B 在 ≤1 心跳周期内对同一令牌返回 401。
- **登记**：backlog B006

### #F-04 — 节点身份标识未注入机器名（大盘展示 `node-local`）
- **现象**：`cluster status` 中所有节点均显示 `node-local (★ self)`，peer 无法区分。
- **根因**：CLI 取 `HOSTNAME`/`HOST` 环境变量，serve 进程未把实例签名/机器名注入 ClusterManager。
- **影响**：多节点大盘无法辨识"这是 dev 还是 preprod"。
- **建议方案**：`cluster join` 时自动采集 hostname（或 `/etc/machine-id` 短哈希）作为 node_id；serve 启动时从 `cluster.json` 读取并上报。
- **验收口径**：`cluster status` 显示 `devserver` / `jobcopilot-preprod` 而非 `node-local`。
- **登记**：backlog B007

### #F-05 — 零停机滚动升级仅骨架，真实滚动编排未联调
- **现象**：`pproxy cluster upgrade` 已实现参数互斥校验、签名/SHA 校验、`.old` 原子备份替换（单机），但"集群多节点逐台 Draining → 客户端漂移 → 探活恢复"的真实编排未在双节点实测。
- **根因**：CLI 为本地执行模型；跨节点 Draining 指令下发、升级进度广播依赖 #F-01 的 Gossip 通道，尚未建设。
- **影响**：双节点备灾下的升级仍可能手动逐台操作。
- **建议方案**：结合 #F-01 Gossip 通道，落地 `cluster upgrade` 的逐节点状态机（Draining 15s 硬超时 → 替换 → 探活 → 下一台）。
- **验收口径**：双节点并发下载时触发升级，客户端错误率 0、升级中途另一节点宕机不阻塞。
- **登记**：backlog B008

### #F-06 — 部署流程：GLIBC 版本兼容需本地编译
- **现象**：dev（GLIBC 2.39）编译的 release 二进制在 preprod（Ubuntu 22.04，GLIBC 2.35）上报 `GLIBC_2.39 not found`。
- **根因**：Rust 默认链接系统 GLIBC；跨发行版直接拷贝二进制不兼容。
- **影响**：新节点加入集群前必须本地安装 Rust 工具链编译（已用 rsproxy 国内镜像打通，约 10 分钟/次）。
- **建议方案**（择一）：
  1. 引入 `x86_64-unknown-linux-musl` 静态编译（单二进制通用，但需处理 ring/rusqlite 等 C 依赖的 musl 构建）；
  2. 或按节点发行版分别编译发布（当前做法）。
- **验收口径**：同一 release 产物可在 dev / preprod / tencent（GLIBC 2.35/2.39 混布）直接运行。
- **登记**：backlog B009

### #F-07 — OpenAI 偶发 421（CF 出口拉黑，非新缺陷，跟踪项）
- **现象**：联调期经 preprod 访问 `api.openai.com` 偶发 421 Misdirected Request。
- **根因**：Cloudflare 出口被 OpenAI 拉黑（ADR-002 已知行为）；RackNerd/Vercel 出口正常（实测恢复 401 = 链路通）。
- **影响**：偶发瞬断，非持续性。
- **建议方案**：无需新开发——维持多出口 failover（RN 优先 → Vercel → CF），客户端 `order_endpoints` 已覆盖。
- **验收口径**：`curl` 重试两次内恢复非 421 状态。
- **登记**：跟踪项（不占用 B 编号；ADR-002 已记录根因）

---

## 三、联调期已修复的缺口（关闭记录，防止重复排查）

### ✅ 配置自愈与零接触入网（Zero-Touch Bootstrap，2026-09-25 升级）
- **原缺陷**：新节点执行 `cluster join` 后仅写入 `cluster.json`，缺少出海网关与验签密钥，启动后报 `403 tunnel_not_configured`，需手动二次复制配置。
- **升级落地**：
  1. `crates/core/src/cluster.rs`：`ClusterJoinToken` 升级为全量配置自愈载荷包（State Bundle），封装 `tunnel_gate_url`、`tunnel_token`、`user_verifying_key` 并全量纳入 HMAC-SHA256 签名校验；
  2. `crates/cli/src/cmd/cluster.rs`：
     - `token-create` 自动提取本机生效的出海端点与多租户验签密钥；
     - `join` 自动将载荷解密、验签并原地自动装配至 `~/.pony/config.toml` 与 `.pproxy.env`；
     - 支持 `--auto-start` 参数，新机器贴入一条令牌即可全自动配置并立即在后台拉起服务，彻底消除任何手工输入与二次配置。
- **实测验证**：在 `preprod` 节点清空旧配置，执行单条 `pproxy cluster join -t <token>`，日志回显 `出海隧道: 已就绪`，零手动配置。

### ✅ 撤销热更新闭环（2026-09-25 修复）
- **原缺陷**：`pproxy user revoke` 仅写本地文件，运行中 gate-server 内存黑名单不加载 → 撤销后令牌仍可用（实测 200，安全缺陷）。
- **修复**：
  1. `crates/gate-server/src/lib.rs`：启动加载 `~/.pony/revoked_tokens.txt` 初始化 `revoked_tokens`；
  2. 新增 `POST /api/user/revoke` 管理端点（`GATE_ADMIN_TOKEN` Bearer 固定时间比较鉴权），实时插入黑名单 + 幂等落盘；
  3. `crates/cli/src/cmd/user.rs`：`user revoke` 落盘后自动推送本机 gate-server 热更新（`GATE_ADMIN_TOKEN` 环境变量），回显"网关热更新: 已生效 ✓"。
- **实测**：revoke → Profile 401 Token Revoked；WS 握手同样 401。
- **测试**：`cargo test --workspace` 271 passed。

### ✅ cluster status 探测式节点状态（2026-09-25 修复）
- **原缺陷**：status 读内存空快照，恒显示 0 节点。
- **修复**：`crates/cli/src/cmd/cluster.rs` 改为读取 `~/.pony/cluster.json` 种子节点，优先探测管理面 `:8900`（401/403 视为在线），回落数据面 8899。
- **实测**：dev 停机 → `离线(✗)`；dev 恢复 → `在线(✓)`。

---

## 四、文档与决策

- 架构决策：`.agents/notes/proposed/architecture/2026-09-25-distributed-commercialization.md`（proposed，尚未迁移 implemented）
- 规格：`docs/product/specs/distributed-commercialization/README.md`（v1.1.0）
- 本文（遗留清单）：`docs/product/specs/distributed-commercialization/FOLLOW-UPS.md`
- 事项层登记：`backlog/backlog.md`（B005~B009，见第二节"登记"）
