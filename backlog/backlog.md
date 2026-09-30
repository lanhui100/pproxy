# 项目 Backlog

> 轻量模式（未迁移 taskctl）。事项层记录；执行级细节见对应 spec。

## 进行中

### B010 — gate 多出口兜底 P0-1（edgetunnel 借鉴）🔄 代码+单测已落地，待部署验收
- **动机**：gate worker 只有 `cloudflare:sockets connect()` 直连一条路——CF 重点风控
  的模式；CF 一收紧出口，整条 gate 隧道链路失效（研究背景：docs/HANDOVER §3 P0-1，已删）
- **方案**：直连失败/被 CF 收紧 → 自动切用户配置兜底（SOCKS5 链式 → SNI 反代中继）；
  默认全留空 = 仅直连（不默认走任何第三方中转）；兜底过 gate-policy.mjs 同一合规门禁
  （合规 host 需声明国家码 fail-closed）
- **验收**：`node deploy/cf-gate-worker/egress-fallback.test.mjs`（43 用例，非零退出）；
  `node deploy/cf-gate-worker/e2e-fallback.mjs`（wrangler dev 直连必败 host 自动切
  SOCKS5 仍出海，非零退出）；生产部署后断直连观察（靠 review）
- **落点**：`deploy/cf-gate-worker/{worker.js,egress-fallback.mjs,egress-fallback.test.mjs,e2e-fallback.mjs,wrangler.toml}`
- **关联**：ADR implemented/feature/2026-09-30-gate-multi-egress-fallback；B011/B012 同源

## 待办

### B011 — gate 伪装页 + 反指纹 P0-2（edgetunnel 借鉴）✅ 已完成
- **动机**：gate 非 `/ws` 路径全裸 404 = 最典型的"可疑 Worker"指纹，防扫号低成本高收益
- **方案**：非 `/ws` 返回 200 仿 nginx 欢迎页；`/ws`、`/debug` 等特征路径运行时拼装；
  token 不进 URL 查询串（Bearer header）；**不抄** edgetunnel 的"多语言无后门"注释垫片（信任争议）
- **验收**：`node deploy/cf-gate-worker/camouflage.test.mjs`（18 用例，非零退出）；
  wrangler dev 实测 `GET /` 与 `/favicon.ico` 均 200 欢迎页、`/debug` 仍 JSON（smoke 通过）；
  E2E 隧道回归（e2e-fallback.mjs）通过
- **落点**：`deploy/cf-gate-worker/{worker.js,camouflage.mjs,camouflage.test.mjs}`
- **关联**：ADR implemented/feature/2026-09-30-gate-camouflage-page；部署走优雅不下线 SOP（DEPLOY.md §更新 CF Worker）

### B012 — CF 请求量自省 + 配额联动 P0-3（edgetunnel 借鉴）✅ 已完成（最小可行增量）
- **动机**：gate 每 WS 会话/每 TCP 连接计 CF 请求；免费档 100k/天（按账号计）极易爆，
  爆了触发滥用风控；现有采集只见账号总量，无法区分 gate 与 edge 各自消耗
- **方案**：CfCollector 支持 `scriptName` 过滤（gate 单独来源 `gate_cf`），monitor 复用
  既有 tick/越线告警链路；**默认关闭**（未设 `PPROXY_CF_GATE_SCRIPT_NAME` 行为不变）
- **验收**：`cargo test -p pproxy-core quota`（含 by_script 用例）+ `cargo test -p pproxy-server`
  （含 gate_source_disabled_by_default）非零退出；生产配置后 /api/quota 出现 gate_cf
  且随隧道流量增长（靠 review）
- **落点**：`crates/core/src/quota.rs`、`crates/server/src/monitor.rs`、`docs/ops/DEPLOY.md`
- **后续增强（未做，backlog 备注）**：探活 worker 本地应答（edgetunnel 反代模式测速），
  现状探活仅消耗升级握手（1 请求/次低频）
- **关联**：ADR implemented/feature/2026-09-30-gate-cf-request-quota

### B013 — 部署形态 + 风控 SOP P1-4（edgetunnel 借鉴）
- **方案**：评估 gate 迁 Pages 部署（社区实证更耐封）；小号部署、域名轮换、
  "能用就别动"纪律；abuse 邮件处理 SOP
- **验收**：文档含"收到 abuse 邮件怎么办 / 域名轮换步骤 / 账号隔离"三节（靠 review）
- **落点**：`deploy/pproxy-service.md` 或 `docs/ops/`

### B014 — 标准协议入站 P1-5（edgetunnel 借鉴，架构级大改）
- **方案**：gate 增加 VLESS-WS 入站，Ed25519 配额令牌映射成 UUID 鉴权 → 第三方客户端
  （Clash/v2rayN/Shadowrocket）直连 gate；代价=协议解析代码量+指纹面变大（需先做 B011）
- **纪律**：**先出方案（proposed ADR）再动手**，不立即实施
- **验收**：方案评审通过后立项；实现后第三方客户端可直连且配额/吊销体系保留

### B015 — 客户端配置细节 P2-6（edgetunnel 借鉴）
- **方案**：订阅链接 HOST/SNI 自动对准 gate 域名（域名轮换后免重新导入）；
  多格式导出（sing-box/Surge）；token 轮换语义
- **验收**：改 gate 域名后重新生成的订阅无需手改 HOST 即可用
- **落点**：`pproxy clash`（`crates/cli`）

### B003 — vedge 改 CF 橙云代理回源 Vercel（中期方案）⏸ 待决策
- **背景**：B002 判决——中国联通出口→Vercel anycast(66.33.60.x/76.76.21.x) 路由间歇性劣化；
  当前稳态为 openai `override=worker` 止血 + `~/vedge-monitor.log` 每 5 分钟探针监控
- **方案**：example.com 的 DNS 把 `vedge` CNAME `cname.vercel.com` 开 **CF 橙云代理**；
  SSL 模式 Full(strict)；Vercel 侧域名绑定不动（已 verified 无需重验）
- **收益**：中国方向经 CF 边缘可达，恢复「vercel 出口多样性」的设计意图；分钟级生效、关橙云即时回滚
- **代价/风险**：客户端改看 CF 通用证书（与 edge 同款 GTS，已在用）；SSE/WebSocket 经 CF 代理需实测兼容；
  代理态下 Vercel 域名校验行为需观察
- **决策触发**：探针日志显示 vercel 线持续劣化 >48h 或一周内复发 ≥2 次 → 执行本方案；自行稳定则关闭本项
- **关联**：B002「判决与处置」小节

### B002 — Vercel 出口（vedge.example.com）超时排查与恢复 ⏸ blocked-by-external
- **触发**：2026-08-24 M7 前端 UX 实测发现——服务端测速 `POST /api/routes/openai/test`（vercel 线）10s timeout；
  直探 `https://vedge.example.com` 6s 无响应；`/api/quota` 监控同步报 `vercel: error`
- **对照证据**：同期 CF Worker 线路正常——anthropic 经 worker 1025ms 穿透上游（404=根路径正常）、
  edge 边缘可达 → 故障定位于 **Vercel 函数/出口侧，而非 CF**
- **动作**：① 查 Vercel dashboard 部署状态与区域事件；② `curl -m6 https://vedge.example.com` 复测至恢复；
  ③ 恢复后经桌面端「服务」页模板网格重加 openai（默认自动决策即走 vercel），并用「测速」验证
- **关联**：docs/product/specs/m7-frontend-ux/DELIVERY.md §实测记录
- **判决与处置（2026-08-24）**：
  - **根因**：中国联通出口 → Vercel anycast(66.33.60.x/76.76.21.x) 路由劣化（高置信；全球 10 节点正常、部署 READY、LE 证书有效）
  - **已执行**：① openai 路由已按 `override_upstream=worker` 重加——创建 201，
    测速 `POST /api/routes/openai/test` ok=true / status=403 / latency=623ms；
    ② 数据面 E2E 探测通过——临时密钥（m7-b002-probe，用后即撤）穿透 `GET …/openai/v1/models`
    返回上游响应码 **403**、耗时 **1.14s**（注：数据面仅绑定 `<TAILNET_IP>:8899`，
    回环 127.0.0.1 拒连，故探测走该地址）；③ vedge 每 5 分钟只读探针已布防
    （用户级 crontab → `~/vedge-monitor.log`，HTTP 非 000 即线路回暖信号；
    布防当日手动首测已回 `404 connect≈0.46s total≈0.69s`）
  - **待决策（用户）**：中期方案「vedge 改 CF 橙云代理回源 cname.vercel.com（SSL Full strict）」
    ——低-中风险、分钟级生效、可即时回滚；因涉生产 DNS 由用户拍板；中期方案已立项 **B003**
  - **状态维持** ⏸ blocked-by-external（vercel 线本身恢复以探针日志为准）

### B001 — CF 故障恢复后重测 S1/S2 spike ⏸ blocked-by-external（执行车道：审计会话）
- **触发**：M6 P0 spike 期间遭遇 CF 全球 PoP 部分中断（Minor Service Outage），WS 数据帧黑洞，测试数据不可信
- **动作**：监控 https://www.cloudflarestatus.com 恢复全绿后，重跑
  `node scripts/spike-tunnel.mjs wss://gate.example.com/ws <token> /files/100Mb.dat --loop 600`（S1）
  与 `--concurrency 4`（S2），结论回填 spec m6 §12 与 ADR-008
- **附带裁决**：✅ 已完成（2026-08-25 恢复窗口实证）——生产边缘可用口径为 `server.accept()` + Response 携带 `pair[0]`；
  `ctx.acceptWebSocket(server)`/返回 server 升级阶段抛 500（两会话独立实测交叉验证一致）。
  注意重跑 S1/S2 需使用轮换后的新 tunnel_token（旧明文令牌 gate-spike-**** 已作废）
- **进展**：CF 状态页已全绿（2026-08-25 复核）；剩余=S1/S2 执行 + ADR-008 回填（凭据在审计会话手中）
- **关联**：docs/product/specs/m6/README.md §0.1/§12

## 已完成

（B 编号从 B004 起递增；M6 主实现不占 backlog——执行真源为 spec m6）

### B002 — M6 实现收尾 🔄 进行中（执行真源=spec m6）
- 隧道凭据接线内测（tunnel_token 下发 GUI）、v0.3.0 发布、Windows 实机验收

### B003 — 未来拓展池（按需启动，暂不排期）
- CF 托管域名白名单站点的 Vercel 侧透传探索（M6 已知限制缓解）
- tunnel_token 双活轮换（消除分钟级停机窗口）
- 自动更新国内镜像加速（tauri updater 走 CF 分发）
- 80 端口明文透传显式开关（默认禁用维持）
- 手机端（Tauri mobile）/ M7 打磨与模板库（ROADMAP 既有）
### B004 — 白名单代理多出口容灾（按需启动）
- 背景：M6 隧道出口单一依赖 CF Worker；CF 平台故障期隧道不可用
- 唯一可行替代：海外 VPS 跑轻量 TCP 中继（Vercel 无 TCP 能力，不可行）
- 触发条件：CF 故障频发 / 单点依赖成为实际痛点

### B005 — 跨节点配额/用量一致性（信用租约） #F-01 #F-02
- 现象：gate-server 用量为单机内存 DashMap；多节点并发可放大配额（无界透支）
- 子项：#F-01 用量 Gossip 同步；#F-02 单用户 max_conns 跨节点穿透
- 方案：令牌 lease_bytes 单节点切片放行 → 增量用量 Gossip → 402 截断；
  并发用 UID 哈希锚点/接入点亲和性
- 验收：同令牌两节点各耗 5G（总额 8G），第二节点用量达 8G 返回 402；
  合计在线连接 >3 时第 4 条返回 429
- 关联：docs/product/specs/distributed-commercialization/FOLLOW-UPS.md #F-01/#F-02

### B006 — 撤销（revoke）跨节点传播 #F-03
- 现象：撤销已实现本机落盘 + 本机 gate-server 热更新闭环；集群其他节点不同步
- 方案：revoked_tokens.txt 纳入集群 Gossip 反熵；中期走管理面 eager-fanout + ACK，绑定 exp 自动清理
- 验收：节点 A revoke 后，节点 B ≤1 心跳周期对同令牌返回 401
- 关联：docs/product/specs/distributed-commercialization/FOLLOW-UPS.md #F-03

### B007 — 节点身份标识注入机器名 #F-04
- 现象：cluster status 恒显示 node-local，多节点无法辨识
- 方案：cluster join 采集 hostname/machine-id 作 node_id；serve 从 cluster.json 上报
- 验收：status 显示 devserver / jobcopilot-preprod
- 关联：docs/product/specs/distributed-commercialization/FOLLOW-UPS.md #F-04

### B008 — 零停机滚动升级真实编排 #F-05
- 现象：upgrade 已支持单机签名校验/原子备份替换；集群逐台 Draining→漂移→探活未联调
- 方案：结合 B005 Gossip 通道落地逐节点状态机（Draining 15s 硬超时→替换→探活→下一台）
- 验收：双节点并发下载触发升级，客户端错误率 0；升级中途另一节点宕机不阻塞
- 关联：docs/product/specs/distributed-commercialization/FOLLOW-UPS.md #F-05

### B009 — 跨发行版 GLIBC 兼容（静态编译） #F-06
- 现象：dev（GLIBC 2.39）编译产物在 preprod（GLIBC 2.35）报 GLIBC_2.39 not found
- 方案：x86_64-unknown-linux-musl 静态编译；或按节点发行版分别发布
- 验收：同一 release 产物可在 GLIBC 2.35/2.39 混布环境直接运行
- 关联：docs/product/specs/distributed-commercialization/FOLLOW-UPS.md #F-06
