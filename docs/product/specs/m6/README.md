# M6 任务级 Specs — 系统级白名单代理（桌面隧道通道）

> 上游: [ROADMAP M6](../../ROADMAP.md) | 状态: 待审核 | 日期: 2026-08-22
>
> 方向裁决（2026-08-22 用户立项）：产品从 SDK 网关升维为"自有可控的白名单代理"。**本 spec 修订 ADR-001 的适用边界**：SDK 数据面维持 API 网关模式不变（CONNECT 拒绝仍成立）；新增的桌面隧道是**独立通道**（新协议/新端点），两者并存，产出物含 **ADR-008**。
>
> 关键前提裁决：
> - **出口通道 = CF Worker WebSocket↔TCP 桥**（`cloudflare:sockets`）：TLS 端到端透传，Worker 全程只见密文（ADR-006 明文顾虑在此通道天然消失）
> - **MITM 明确不做**：不解密任何流量，白名单按 CONNECT 目标 host 判定
> - **分发复用**：桌面端改动随 pony-desktop 常规版本发布（v0.3.0 起），更新走既有自更新链路

## 0. 现状探测结论（2026-08-22 实测/查证）

| 项 | 结论 | 来源 |
|----|------|------|
| `cloudflare:sockets` 可行性 | GA 能力：`connect(host, port)` 可出站 443，社区大规模验证（Workers 承载代理协议） | 官方文档 + 实践 |
| **CF 自家托管站点禁止直连** | 出站到 Cloudflare IP 段被设计性阻断 → **白名单中"托管在 CF 的域名永远连不通"**（如部分 AI 站） | 官方 Considerations 原文 |
| 其他禁则 | localhost/私网 IP 禁止；25 端口禁止；TCP 回环检测（Worker→自身） | 同上 |
| 并发上限 | 免费版存在同时打开连接数上限（个位数，精确值实现期实测钉住）；HTTP/2 多路复用使"每域名一条隧道"成为常态，单用户浏览场景可控 | 官方 limits 页 |
| 现有 edge worker | 单文件 fetch 代理（deploy/cf-worker/worker.js，X-Proxy-Secret 鉴权，wrangler.toml 就绪） | 实测 |
| 桌面端 | v0.2.x 已有 updater/keyring/notification/http 插件底座；无本地代理能力 | 实测 |
| 国内可达 | edge.ponyjob.top 经 CF 边缘可达已被长期验证；wss 与 https 同源同路径特征 | 生产先例 |

## 1. 目标与范围

Windows 上实现**白名单式系统代理**：用户在 GUI 维护域名白名单（如 `youtube.com`、`github.com`），命中流量的 TLS 密文经 CF Worker 隧道从海外出口透传；未命中流量本机直连、不经任何代理组件。

验收场景（ROADMAP 原文化）：Windows 浏览器开启代理开关后，youtube.com/google.com 正常播放/搜索；非白名单网站（如 baidu.com）确认不走隧道（服务器侧日志零命中）。

不在范围内：
- **MITM**：不解密 TLS（白名单按目标 host 判定足够，证书告警/隐私风险为零收益）
- macOS/Linux 客户端、移动端（P2）
- 全局 VPN 模式（exit node 已由 Tailscale 承担，与本产品正交）
- UDP/QUIC 代理（浏览器对代理后的站点自动回落 TCP/TLS，HTTP/3 被禁用是业界通行做法）
- 服务端带宽/流量计费（CF 免费额度 + 配额告警沿用现有 monitor）

## 2. 架构

```
[Windows] pony-desktop v0.3.0（新增本地代理引擎，嵌入应用进程）
   ├─ 引擎监听 127.0.0.1:18900（HTTP 代理语义：absolute-form + CONNECT）
   ├─ PAC 服务 http://127.0.0.1:18900/pac （白名单粗筛：命中→PROXY 127.0.0.1:18900，否则 DIRECT）
   ├─ 系统代理两种模式：PAC 模式（推荐）/ 手动模式（全量进引擎二次分流）
   ├─ 分流判定：host 后缀匹配白名单 → 是：WS 隧道；否：本机直接 dial
        │ wss://gate.ponyjob.top/ws （Authorization: Bearer <pony-token>）
        ▼
[CF Worker] gate worker（新部署，独立于 edge 生产通道 —— 故障隔离）
   ├─ 校验 Bearer（复用 pony 数据 token，Tokens 页统一管理）
   ├─ 首帧 JSON {"host":"www.youtube.com","port":443} → ACL 校验（仅 80/443）
   ├─ cloudflare:sockets connect(host, port) ──▶ 目标站（TLS 字节透传，不解密）
   ▼
[youtube.com 等]
```

- **独立 worker + 独立主机名 `gate.ponyjob.top`**（ADR-003 中性命名）：不碰 edge 生产通道（所有 SDK 流量经此），故障隔离优先于少一个部署面。
- 本地引擎为**纯 TCP 分流器**：CONNECT 取目标 host；absolute-form HTTP 取 Host 头。白名单命中与否都只是"换一条 TCP 出口"，无协议转换、无缓存、无解密。
- 引擎嵌入 pony-desktop 进程（tokio 任务），托盘开关控制"系统代理总开关"（写/还原 WinINET 注册表 + 启停引擎）。

## 3. 协议规格（WS 隧道）

| 项 | 规格 |
|----|------|
| 端点 | `wss://gate.ponyjob.top/ws` |
| 鉴权 | Upgrade 请求头 `Authorization: Bearer <pony-data-token>`（401 拒绝；token 复用现有 Tokens 页凭据，吊销即时生效） |
| 首帧 | 客户端发文本帧 `{"host":"…","port":443}`；服务端校验 port ∈ {80,443}、host 非空非私网字面量 → 回 `{"ok":true}` 或 `{"ok":false,"reason":"…"}` 后关闭 |
| 数据帧 | 二进制帧双向透传（CF 默认单帧上限内分块） |
| 心跳/超时 | 服务端 30s ping、5min 空闲关闭；客户端断线自动重建（浏览器侧表现为连接刷新） |
| 并发 | 每 CONNECT 一条 WS（HTTP/2 多路复用下每站点通常 1 条）；客户端设全局并发上限 32，超出排队 |

## 4. 白名单语义与数据模型

- 匹配规则：**域名后缀匹配**——条目 `youtube.com` 命中 `youtube.com` 与 `*.youtube.com`；不含路径/通配符语法（保持心智简单）
- 存储：`%APPDATA%/pony-desktop/proxy-whitelist.json`（本地资产，不上服务器——仅本设备路由语义）；格式 `{"entries":["youtube.com","github.com"],"updated_at":…}`
- GUI：新页面 **Proxy**——白名单 CRUD + 总开关 + 当前状态（引擎运行/系统代理生效中）+ 导入/导出
- 预置种子：首次启用时预填 `github.com`、`google.com`、`youtube.com`、`gstatic.com`、`googleapis.com`、`ytimg.com`、`ggpht.com`（覆盖三大站的资源域，避免"主站通而图片挂"）

## 5. 本地代理引擎规格

| 项 | 规格 |
|----|------|
| 监听 | `127.0.0.1:18900`（仅回环；端口常量避开既有段） |
| CONNECT | 解析 host:port → 分流判定 → 隧道或直连；返回 `200 Connection Established` |
| absolute-form | 读 Host 头分流；请求体原样转发（罕见路径，尽力而为） |
| PAC 端点 | `/pac` 返回由当前白名单动态生成的 PAC 脚本（FindProxyForURL 后缀匹配同 §4 规则） |
| 系统代理写入 | PAC 模式：WinINET 设 PacUrl；手动模式：ProxyServer=127.0.0.1:18900 + Override 留空；关闭时完整还原快照 |
| 生命周期 | 托盘菜单：启用/停用/退出；随主程序退出自动还原系统代理设置（防"退出后断网"经典事故） |
| 失败语义 | 隧道建立失败（Worker 不可达/token 失效/目标被 CF 禁）→ 关闭该 CONNECT 并让浏览器报错；**绝不静默回落直连**（防"以为在走代理实际裸连"的安全错觉，UI 有明确错误计数） |

## 6. Worker 改造规格（新 gate worker）

```js
// gate worker 核心流程（deploy/cf-gate-worker/）
export default {
  async fetch(request, env) {
    if (url.pathname !== "/ws") return404
    if (request.headers.get("Upgrade") !== "websocket") return400
    if (!await verifyToken(request.headers.get("Authorization"), env)) return401  // 复用 Tokens 表哈希校验?
    const { host, port } = 首帧JSON
    aclCheck(port ∈ {80,443}) && 私网/CF字面量拒绝
    const sock = connect({ hostname: host, port })
    return WebSocketPair 双向 pipe（背压用 ws sender 原生机制）
  }
}
```

- **token 校验方式**：worker 内嵌 SHA-256 校验逻辑与 pproxy 一致——但 worker 无法读服务器 SQLite！方案：env 注入**专用隧道 token 的哈希**（部署时由服务器生成随机 token 并同步给 GUI/服务端两侧；独立于数据 token，泄漏影响面更小）。Tokens 页展示该隧道凭据状态（只读）。此为 §4 的修正：**不复用数据 token**，改用独立 `tunnel_token`（生成/轮换走 GUI Settings 按钮 + 服务端 env 更新，spec 评审可挑战）。
- 变量：`TUNNEL_TOKEN_HASH`（wrangler secret）
- 可观测：每连接 log 一行（时间/host/时长/上下行字节），配合 CF Analytics 看用量

## 7. 安全

- 防开放跳板：三重门——tailnet 之外的公网扫描者无 token 即 401；ACL 仅 80/443 且拒绝私网/CF 字面量；签名流量特征为 wss（与正常 Websocket 应用无异）
- 凭据纪律：隧道 token 哈希入 wrangler secret 不入库不入日志；明文仅创建时展示一次（沿 M1 口径）
- 引擎仅绑回环：局域网设备不可借用（想借=对方自己装 pony-desktop 入 tailnet）
- ADR-008 记录：与 ADR-001 的边界区分（SDK 网关无 CONNECT ≠ 桌面隧道通道）+ CF 托管域名不可代理的限制声明

## 8. 部署与配合点

| 步骤 | 谁 |
|------|-----|
| gate worker 部署（wrangler deploy + route 绑定 gate.ponyjob.top） | 需要 **CF API Token（Account.Workers Scripts:Edit + Zone.Workers Routes:Edit）**——用户配合点①（dashboard 创建，同 cfat_ 流程） |
| 隧道 token 生成/下发 | 服务端生成 → GUI 展示一次 → wrangler secret 更新（脚本化） |
| 桌面端发布 | 常规 tag 流程（v0.3.0） |
| 白名单种子/托盘 | 随包交付 |

## 9. 测试清单

自动化（能测的尽量测）：
1. 白名单匹配器纯函数：后缀命中/未命中/大小写/端口变体/恶意输入
2. PAC 生成器：快照测试（给定 entries → 脚本字符串）
3. sanitize/ACL 纯函数：私网字面量、非 80/443、空 host
4. 引擎集成（Rust tokio test）：起引擎 + mock WS 服务端 → CONNECT 白名单域名 → 断言字节透传；CONNECT 非白名单 → 断言直连 dial 发生；CONNECT 失败 → 断言无静默回落
5. Worker 侧：wrangler 自带 vitest-pool-workers 对 ACL/鉴权分支单测（connect 本身 mock）

手动验收（Windows 实机）：
1. 开启代理 → 浏览器 youtube.com 视频播放、google.com 搜索正常
2. baidu.com 正常访问且服务器 gate 日志零命中（直连证据）
3. 关闭总开关 → 系统代理设置还原 → 一切直连
4. token 错误时浏览器明确报错而非静默直连
5. 休眠恢复/网络切换后自愈

## 10. 风险与缓解

| 风险 | 缓解 |
|------|------|
| **CF 托管域名无法代理**（设计性阻断） | 文档明示 + GUI 白名单条目加"最近连通性"标记（失败即提示可能为 CF 托管站）；后续可为这类站点评估 Vercel 侧透传（P2 探索） |
| 免费版并发连接上限 | 每域名单连接（H2 复用）+ 客户端排队；实测钉住精确值后再定是否需要付费版 |
| Worker 被扫描滥用 | Bearer 强制 + token 哈希 secret + monitor 配额告警沿用；异常流量在 CF Dashboard 可见 |
| wss 特征被识别 | 与正常 WebSocket 应用同类特征；个人低流量规模下风险极低；不做主动对抗（超出个人工具定位） |
| 退出/崩溃残留系统代理导致断网 | 引擎生命周期守卫：进程退出钩子 + 下次启动检测残留并修复（DEPLOY 文档含手工还原步骤） |
| edge 生产通道回归风险 | gate 为完全独立 worker，零共享代码路径；edge 不动 |
| 用户配合点阻塞（CF Workers 部署凭据） | 同 M3-R2/M4 模式：自动化先行，凭据到位前挂起 |

## 11. 验收标准

- 自动化全绿（§9.1-9.5）+ desktop-gate/desktop-release CI 绿
- 手动验收 §9 五项全过（含 baidu 直连零命中的反例证据）
- 文档同步：ADR-008、TECH_DESIGN §2.5、ROADMAP M6 ✅、DEPLOY.md（gate worker 运维）、API.md 不涉及
- 版本：pony-desktop v0.3.0；gate worker 独立版本号
