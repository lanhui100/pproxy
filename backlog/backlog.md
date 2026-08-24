# 项目 Backlog

> 轻量模式（未迁移 taskctl）。事项层记录；执行级细节见对应 spec。

## 进行中

（无）

## 待办

### B003 — vedge 改 CF 橙云代理回源 Vercel（中期方案）⏸ 待决策
- **背景**：B002 判决——中国联通出口→Vercel anycast(66.33.60.x/76.76.21.x) 路由间歇性劣化；
  当前稳态为 openai `override=worker` 止血 + `~/vedge-monitor.log` 每 5 分钟探针监控
- **方案**：ponyjob.top 的 DNS 把 `vedge` CNAME `cname.vercel.com` 开 **CF 橙云代理**；
  SSL 模式 Full(strict)；Vercel 侧域名绑定不动（已 verified 无需重验）
- **收益**：中国方向经 CF 边缘可达，恢复「vercel 出口多样性」的设计意图；分钟级生效、关橙云即时回滚
- **代价/风险**：客户端改看 CF 通用证书（与 edge 同款 GTS，已在用）；SSE/WebSocket 经 CF 代理需实测兼容；
  代理态下 Vercel 域名校验行为需观察
- **决策触发**：探针日志显示 vercel 线持续劣化 >48h 或一周内复发 ≥2 次 → 执行本方案；自行稳定则关闭本项
- **关联**：B002「判决与处置」小节

### B002 — Vercel 出口（vedge.ponyjob.top）超时排查与恢复 ⏸ blocked-by-external
- **触发**：2026-08-24 M7 前端 UX 实测发现——服务端测速 `POST /api/routes/openai/test`（vercel 线）10s timeout；
  直探 `https://vedge.ponyjob.top` 6s 无响应；`/api/quota` 监控同步报 `vercel: error`
- **对照证据**：同期 CF Worker 线路正常——anthropic 经 worker 1025ms 穿透上游（404=根路径正常）、
  edge 边缘可达 → 故障定位于 **Vercel 函数/出口侧，而非 CF**
- **动作**：① 查 Vercel dashboard 部署状态与区域事件；② `curl -m6 https://vedge.ponyjob.top` 复测至恢复；
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

### B001 — CF 故障恢复后重测 S1/S2 spike ⏸ blocked-by-external
- **触发**：M6 P0 spike 期间遭遇 CF 全球 PoP 部分中断（Minor Service Outage），WS 数据帧黑洞，测试数据不可信
- **动作**：监控 https://www.cloudflarestatus.com 恢复全绿后，重跑
  `node scripts/spike-tunnel.mjs wss://gate.ponyjob.top/ws <token> /files/100Mb.dat --loop 600`（S1）
  与 `--concurrency 4`（S2），结论回填 spec m6 §12 与 ADR-008
- **附带裁决**：恢复窗口内验证生产边缘 accept() 旧式与 ctx.acceptWebSocket 新式哪个可用（当前证据互相矛盾：本地 alpha 禁旧式、生产 ctx 缺失）
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
