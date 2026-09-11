# M6 任务级 Specs — 系统级白名单代理（桌面隧道通道）

> 上游: [ROADMAP M6](../../ROADMAP.md) | 状态: 已审核（2026-08-22 对抗评审 F1-F23 → R1-R8 回填；**开工前置 P0 spike 见 §0.1，结论可能推翻出口通道裁决**） | 日期: 2026-08-22
>
> 方向裁决（2026-08-22 用户立项）：产品从 SDK 网关升维为"自有可控的白名单代理"。修订 ADR-001 适用边界：SDK 数据面维持网关模式不变；新增桌面隧道为独立通道（ADR-008）。
>
> 审核裁决摘要：
> - **R1（P0 前置 spike）**：Workers 免费计划单次调用 **10ms CPU** 上限对 WS↔TCP relay 拷贝型负载是真实威胁（视频吞吐为最坏负载）；实现前必须完成"单隧道拉 1080p ≥10 分钟"spike 并回填数据；若撑不住则重裁出口通道（$5 paid / 替代出口）。另：同账户下 gate 与 edge **共享免费额度**（日请求/CPU），存在互挤风险须登记。
> - **R2**：凭据统一为**独立 tunnel_token**（哈希入 wrangler secret）：收益=泄漏面隔离+轮换不伤 SDK；代价=worker 仅持哈希无在线撤销能力（轮换走重部署分钟级窗口）——取舍成文。明文仅经 keyring，严禁与 whitelist.json 同目录落盘。§2/§3 全文同步，删除"吊销即时生效"表述。
> - **R3**：白名单种子补 `googlevideo.com`（YouTube 视频流）、`githubassets.com`、`googleusercontent.com`——否则验收场景必然假失败；验收前置子资源域名清点。
> - **R4**：PAC 返回串钉死格式：命中条目返回**单条 `PROXY 127.0.0.1:18900`，禁止 DIRECT 兜底**（防隧道失败静默裸连的安全错觉）；kill 引擎反证测试并入清单。
> - **R5**：私网防护显式声明依赖 workerd 平台层解析后 IP 阻断（引用官方文档），sanitize 测试扩充变形集（IPv6 字面量/十进制 IP/八进制/末尾点）。
> - **R6**：手动模式降格为实验性：ProxyOverride 默认写 `<local>`+内网段排除；Firefox 不读 WinINET 的边界写入验收声明。
> - **R7**：connect() 测试 spike（vitest-pool-workers 可行性）结论回填 §9.5；mock 层与平台层证明力分层表述。
> - **R8**：ACL 默认仅 443（80 需显式开关，避免明文透传复活 ADR-006 顾虑）；gate 日志 host 哈希化；轮换停机窗口明示。

## 0. 现状探测结论（2026-08-22 实测/查证）

| 项 | 结论 | 来源 |
|----|------|------|
| `cloudflare:sockets` 可行性 | GA：`connect(host, port)` 可出站 443，社区大规模验证 | 官方文档 |
| **CF 自家托管站点禁止直连** | 出站到 CF IP 段被设计性阻断 → 白名单中此类域名永远连不通 | 官方 Considerations |
| 其他禁则 | localhost/私网字面量禁止（解析后 IP 由 **workerd 平台层强制**，R5 声明依赖）；25 端口禁止；TCP 回环检测 | 同上 |
| **CPU 限制（R1 核心）** | 免费计划**每次调用 10ms CPU**（付费 30s 起）；WS↔TCP relay 的每帧内存拷贝计 CPU，视频吞吐为最坏负载；idle 等待不计费救不了拷贝 | Workers Limits 页 |
| **额度账户级共享（F2）** | gate 与 edge 生产 worker 共享同账户的日请求数/CPU 额度——互挤风险成立，edge 承载全部生产 SDK 流量 | 平台计费模型 |
| 并发连接数 | 社区流传"个位数"可能已过时（2026-04 有放宽 changelog）；精确值以当前 limits 页实测钉住 | 待实测 |
| 现有 edge worker | 单文件 fetch 代理（X-Proxy-Secret 鉴权，wrangler custom_domain 部署） | deploy/cf-worker/ |
| 桌面端底座 | updater/keyring/notification/http 插件就绪；无本地代理能力 | v0.2.x 实测 |

## 0.1 P0 前置 spike（实现开工闸门，R1）

在写任何产品代码之前完成，结论写进 ADR-008：

| Spike | 方法 | 通过标准 |
|-------|------|---------|
| S1 吞吐/CPU | 最小 gate worker + 单条 WS 拉 YouTube 1080p 视频 ≥10 分钟 | 无 CPU limit 错误、吞吐无明显衰减、CF Dashboard CPU 曲线留档 |
| S2 并发 | 单站点页面加载 + 多站点并行，观测同时连接数峰值 vs 免费上限 | 峰值 < 上限 × 50% 或明确排队策略可行 |
| S3 日请求基线 | 统计 edge 生产 worker 当前日请求量 | gate 预估增量 < 剩余额度 × 30%，否则评估 paid/独立账户 |
| S4 connect() 测试可行性 | vitest-pool-workers 最小 connect() 用例跑通或证伪 | 结论决定 §9.5 自动化范围 |

S1 失败的处理：升级 $5 paid 重测一轮；仍失败则回到用户重新裁决出口方案（spec 作废重写）。**S1 未出结论前不写一行产品代码。**

## 1. 目标与范围

Windows 上实现**白名单式系统代理**：GUI 维护域名白名单，命中流量的 TLS 密文经 CF Worker 隧道从海外透传；未命中流量本机直连、不经任何代理组件。

验收场景：Windows 浏览器（Edge/Chrome，**Firefox 不读 WinINET 系统代理——超出本里程碑边界声明**，R6/F7）开启代理后 youtube.com 正常播放、google.com 正常搜索；非白名单网站确认不走隧道（证据链见 §9 反例口径）。

不在范围内：MITM；macOS/Linux/移动端；全局 VPN（Tailscale 正交）；UDP/QUIC 代理（浏览器自动回落 TCP）；流量计量计费（告警≠计量，配额告警沿用 monitor）；80 端口明文透传（默认禁用，见 §3 ACL）。

## 2. 架构

```
[Windows] pony-desktop v0.3.0（本地代理引擎嵌入进程）
   ├─ 引擎监听 127.0.0.1:18900（CONNECT + absolute-form；端口已在 Windows 本机专用约定中登记，F11）
   ├─ PAC 服务 /pac（命中→单条 PROXY 无兜底；未命中→DIRECT；格式钉死见 §5，R4）
   ├─ 系统代理模式：PAC（推荐）/ 手动（实验性，Override=<local>+内网排除，F7）
   ├─ 分流：host 后缀匹配 → 是：wss 隧道；否：本机 dial
        │ wss://gate.example.com/ws （Authorization: Bearer <tunnel_token>）
        ▼
[CF Worker] gate worker（新部署，独立于 edge；账户级额度共享风险见 §10）
   ├─ Bearer 校验（TUNNEL_TOKEN_HASH，sha256(tunnel_token)）
   ├─ 首帧 {"host","port":443} → ACL → connect() TLS 字节透传
   ▼
[目标站]
```

- 引擎为纯 TCP 分流器：无协议转换、无缓存、无解密。
- 托盘开关 = 总开关（WinINET 写/还原 + 启停引擎）。

## 3. WS 协议规格

| 项 | 规格 |
|----|------|
| 端点 | `wss://gate.example.com/ws` |
| 鉴权 | Upgrade 头 `Authorization: Bearer <tunnel_token>`；服务端 sha256 后比对 TUNNEL_TOKEN_HASH；失败 401 关闭 |
| 首帧 | 文本帧 `{"host":"…","port":443}`；ACL 校验后回 `{"ok":true}` / `{"ok":false,"reason":"…"}` 后关闭 |
| ACL | **port 仅允许 443**（80 明文透传默认禁用，需显式配置开关才放行——R8/F12）；host 经归一化后拒绝空值/私网与 CF 字面量（解析级校验视 S4 spike 决定，平台依赖声明见 §7/R5） |
| 数据帧 | 二进制双向透传（背压处理见 §5 工程规格） |
| 心跳/超时 | 服务端 30s ping、5min 空闲关闭；**进行中连接失败即向浏览器报错，不透明恢复；新 CONNECT 自动按当前配置重试**（F14 措辞修正） |
| 凭据语义（R2） | tunnel_token 为独立凭据：泄漏面隔离、轮换不影响数据 token；**代价=worker 仅持哈希、无在线撤销检查**，轮换=服务器生成新 token → wrangler secret 更新（分钟级停机窗口，双活方案 P2）→ GUI 重新录入；**明文仅存 keyring，严禁落盘至 whitelist.json 同目录** |

## 4. 白名单语义与数据模型

- 匹配：域名后缀匹配（条目 `youtube.com` 命中自身与全部子域）；输入规范化（大小写折叠、末尾点剥离、IDN→punycode 归一、拒绝前导点条目，F15 用例覆盖）
- 存储：`%APPDATA%/pony-desktop/proxy-whitelist.json`（本地资产）；`{"entries":[…],"updated_at":…}`
- GUI：新 **Proxy** 页——CRUD + 总开关状态 + 导入导出 + 条目连通性标记（失败提示可能为 CF 托管站）
- 预置种子（R3/F3 补齐）：`github.com`、`google.com`、`youtube.com`、**`googlevideo.com`**（YouTube 视频流域，缺失=视频转圈假故障）、**`githubassets.com`**、**`googleusercontent.com`**、`gstatic.com`、`googleapis.com`、`ytimg.com`、`ggpht.com`
- 验收前置任务：三大站子资源域名清点（DevTools 过滤非直连域名），结果回填种子表

## 5. 本地代理引擎规格

| 项 | 规格 |
|----|------|
| 监听 | `127.0.0.1:18900`（Windows 本机专用端口登记） |
| CONNECT | 解析 host:port → 分流 → 隧道或直连 + `200 Connection Established` |
| absolute-form | 读 Host 头分流；尽力而为（现代浏览器 HTTPS 场景几乎不触达） |
| PAC 端点 | `/pac` 动态生成；**返回串格式钉死（R4/F5）：命中 → `return "PROXY 127.0.0.1:18900";`（无 DIRECT 兜底）；未命中 → `return "DIRECT";`** |
| 系统代理写入（F10） | PAC 模式设 AutoConfigURL；手动实验模式设 ProxyServer + Override=`<local>`+内网段排除；写入后广播 `WM_SETTINGCHANGE("Internet Settings")` + `InternetSetOption(REFRESH)`；快照还原区分"用户原本自设代理"（还原原值而非清零），崩溃修复同时处理 ProxyEnable 与 AutoConfigURL 两套键 |
| 工程细节（F10） | TCP connect 显式超时；keepalive 半开检测；WS↔TCP 双向拷贝用带容量 channel 做背压（禁 unbounded OOM）；关停顺序=先还原系统代理再停引擎 |
| 生命周期 | 托盘启用/停用/退出；退出钩子还原系统代理 + 下次启动检测残留并修复 |
| 失败语义 | 隧道失败 → 关闭该 CONNECT 让浏览器报错 + UI 错误计数；**绝不静默回落直连** |

## 6. Worker 改造规格（deploy/cf-gate-worker/）

- 流程：/ws 校验 Bearer→sha256 比对 env.TUNNEL_TOKEN_HASH→首帧 ACL→connect()→WebSocketPair 双向 pipe（背压感知）
- 可观测：每连接记录 `{ts, host_hash, port, duration, up_bytes, down_bytes}`——**host 只记 SHA-256 前 16 字节**（浏览画像隐私，F13），映射表仅存本地 GUI 供展示；observability 采样率固定 100%（DEPLOY.md 登记）
- 部署：wrangler secret 注入 TUNNEL_TOKEN_HASH；route 绑定 gate.example.com

## 7. 安全

- 三重门（修正后口径，R5/F6）：token 哈希门 + ACL 门（443-only、归一化校验）+ **私网阻断主防线=workerd 平台层解析后 IP 强制**——此为显式平台依赖而非自有设计，ADR-008 引用官方文档并声明"平台放宽即失效"的风险归属
- 凭据纪律：TUNNEL_TOKEN_HASH 入 wrangler secret；tunnel_token 明文仅 keyring（硬约束）
- ADR-008 必录三项（评审指定）：账户级额度共享边界（F2）、私网阻断的平台依赖（F6）、443-only 决策（F12）；另记明文字节全程不出设备边界的事实
- 日志留存位置/期限：CF observability（平台保留期）+ 本地计数器（用户可清）

## 8. 部署与配合点

| 步骤 | 谁 |
|------|-----|
| gate worker 部署（wrangler deploy + route/custom-domain 绑定） | 用户配合点①：CF API Token——scope 以 wrangler 实际要求核全（Workers Scripts:Edit + Zone 级 Routes/Custom Domains 权限，F16：创建前对照 wrangler 报错清单核全，避免返工） |
| 隧道 token 生成/secret 更新 | 服务端脚本化（生成→keyring/GUI 展示一次→wrangler secret put） |
| 桌面端发布 | 常规 tag 流程（v0.3.0，签名产物含 latest.json 自更新链） |

## 9. 测试清单

自动化：
1. 白名单匹配器：后缀命中/未命中/大小写/末尾点/IDN-punycode/前导点拒绝/恶意输入（F15）
2. PAC 生成器快照测试：**断言输出不含 "DIRECT" 于命中分支**（R4 回归锚点）
3. sanitize/ACL 纯函数：变形全集——IPv6 字面量 `[::1]`、十进制整数 IP `2130706433`、八进制 `0177.0.0.1`、末尾点、私网段各变体（F6/R5）
4. 引擎集成（tokio test + mock WS 服务端）：白名单透传字节一致；非白名单断言发生本地 dial；隧道失败断言**无静默回落**；kill 引擎进程 → 浏览器侧必须报错（PAC 无兜底反证，R4）
5. Worker 分支测试：ACL/鉴权纯函数全覆盖（connect 依赖注入 mock）——**证明力分层声明（R7/F9）：mock 层只证明逻辑，平台行为（CPU/连接限制）只能由 S1-S3 spike 与实机验收证明**
6. 会话计数器：per-domain 直连/隧道计数正确累加并可导出

手动验收（Edge/Chrome 边界内，F7）：
1. 开启代理 → youtube.com 视频播放（含 googlevideo 流域验证）、google.com 搜索
2. **baidu.com 直连证据链（R8/F8 升级口径）**：本地会话统计导出显示 baidu 直连 N 次、隧道 0 次（gate 日志降为辅助证据）
3. 关闭总开关 → 注册表还原（含 AutoConfigURL 快照分支）→ 一切直连
4. token 错误 → 浏览器明确报错而非静默直连
5. 休眠恢复/网络切换自愈；自更新重启窗口期浏览器报错属预期（注册表短暂指向重启中的引擎，F18）
6. 手动实验模式（如启用）：非浏览器应用行为记录 + 内网地址不走引擎验证
7. 卸载残留：系统代理设置与凭据清理路径验证（F18）

## 10. 风险与缓解

| 风险 | 缓解 |
|------|------|
| **免费版 10ms CPU 对 relay 拷贝型负载**（R1） | P0 S1 spike 前置；失败则 paid/换出口重裁——不带着侥幸心理开工 |
| **gate/edge 账户级额度互挤**（F2） | S3 基线核查 + ADR-008 登记共享边界；必要时 gate 迁独立账户/paid |
| CF 托管域名无法代理 | 文档明示 + GUI 连通性标记；Vercel 侧透传 P2 探索 |
| 免费并发连接上限（数值待 S2 实测） | H2 复用 + 排队 + 上限实测钉值 |
| 被扫描滥用 | Bearer 哈希门 + ACL + monitor 配额告警（告警≠计量） |
| wss 特征识别 | 个人低流量不做主动对抗 |
| 系统/注册表残留断网 | 生命周期守卫（§5）+ DEPLOY 手工还原步骤 |
| edge 回归 | gate 完全独立部署面；edge 不动 |
| 配合点阻塞 | 凭据到位前自动化先行挂起（沿 M3-R2 模式） |

## 11. 验收标准

- P0 spike（S1-S4）结论文档化进 ADR-008 且通过标准全达成
- 自动化全绿（§9.1-9.6）+ desktop-gate/release CI 绿
- 手动验收 §9 全过（含 baidu 直连的本地计数器正向证据）
- 文档同步：**ADR-008**（必录三项见 §7）、TECH_DESIGN §2.5、ROADMAP M6 ✅、DEPLOY.md（gate worker 运维/日志采样率/token 轮换 runbook）
- 版本：pony-desktop v0.3.0（自更新链路交付）
- 手机 4G 验收（M4 顺延项）仍挂起待用户安排

## 12. P0 Spike 执行记录（2026-08-22）

| Spike | 结论 |
|-------|------|
| S4 connect() 测试可行性 | **证伪**：本机工具链（miniflare 5.20260820-alpha + workerd 1.20260820）对 WS 101 响应处理存在缺陷——旧式 `accept()` 后返回直接抛错；新式 `ctx.acceptWebSocket()` 下消息投递异常且 miniflare pretty-error 对 101 空响应体崩溃。结论：worker 自动化测试范围收敛为纯函数（ACL/鉴权/匹配器），WS 行为验证以真机为准；另发现新版运行时要求 `ctx.acceptWebSocket(server)` 替代手动 `accept()`（已迁移，生产代码就绪） |
| S1 吞吐/CPU | **待部署后执行**：需真实 CF 边缘（本地 workerd 无法度量平台 CPU 计费）。测试台已就绪（scripts/spike-tunnel.mjs，TLS-over-tunnel 完整性由证书校验自然保证） |
| S2 并发 | 待部署后执行（harness 支持 --concurrency） |
| S3 日请求基线 | 已有数据：edge worker 今日 invocations=3642（GraphQL Analytics 实测），gate 预估增量远低于免费额度 |

**下一步阻塞点**：CF API Token（Workers Scripts:Edit + Zone Routes 权限）→ gate worker 远程部署 → 执行 S1/S2。

### 追加诊断（2026-08-22 部署后，同日）

| 发现 | 证据 |
|------|------|
| gate.example.com 已上线 | 账户级 Custom Domains API 绑定成功（绕开 dashboard 故障）；/debug 200 |
| **CF 官方 Minor Service Outage 进行中** | 状态页：全球数十个 PoP partial_outage/under_maintenance；但 Workers/WebSockets/Dashboard 组件标记 operational（组件级与 PoP 级状态背离） |
| **WS 数据帧黑洞（核心症状）** | 101 握手协议层正常完成（curl verbose 确认 Sec-WebSocket-Accept 正确），但握手后双向数据帧全部丢失；accept 前置/ctx 两模式、本地计数器均无法收到帧 |
| 行为抖动 | 同一请求时而 101 时而 Empty Reply（不同 PoP/路径健康度不一） |

**结论修正**：此前怀疑的代码问题（accept 模式/ctx 迁移）均非根因——当前窗口处于 CF 全球 PoP 部分中断期，spike 数据不可信。**S1/S2 顺延至官方状态页恢复全绿后重测**；重测时需一并裁决生产边缘的 accept 模式口径（旧式 accept() 与 ctx.acceptWebSocket 在恢复后的干净窗口各验一次）。期间产品代码开发不受阻（引擎/白名单/PAC 纯函数与 UI 可先行，WS 行为验证留待窗口恢复）。

### 追加记录（2026-08-25 恢复窗口：accept 口径裁决落定 + v0.3.5 发布）

| 项 | 结论 |
|----|------|
| CF 状态页 | 恢复全绿（Workers/WebSockets/Dashboard operational，未解决事件=0）——重测窗口开启 |
| **accept 口径裁决（B001 附带裁决完成）** | 生产边缘实证：`server.accept()` 后 Response 必须携带 **`pair[0]`（client 端）**→ 数据帧正常；`ctx.acceptWebSocket(server)`/返回 server → 升级阶段抛 500。两个会话独立实测交叉验证一致；与 §12 S4「本地 alpha 工具链两模式全坏」不矛盾——平台行为只能真机裁决（R7 证明力分层的再验证）。gate 已按可用口径部署带鉴权正版代码 |
| 凭据轮换 | 审计整改轮换 tunnel_token：旧明文 gate-spike-**** 作废，桌面端 GUI 重录新 token 方可走隧道（R2 轮换语义兑现）；源码去硬编码端点/令牌，引擎隧道改 opt-in 由配置注入（属下一版内容） |
| v0.3.5 发布 | NSIS installerHooks（POSTINSTALL/PREUNINSTALL 定向清理历史 pony-desktop.lnk，仅匹配旧安装目标路径防误删）；sync-desktop-release.sh 资产 URL 改写口径迁 `https://access.example.com/dsk/`（与 updater 端点一致，公开可达验证 200）；本地分发目录旧版本产物已清理 |
| S1/S2 吞吐并发 | CF 全绿但执行车道移交审计会话（其持有轮换后新凭据）；数据回填 **ADR-008** 后 M6 方可整体打 ✅ |
