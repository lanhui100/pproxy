# Pony Proxy — PRD

> 版本: v0.2 | 日期: 2026-09-02 | 状态: 已更新（M6 / 代理优先演进）

## 1. 定位

自托管的**现代开发者出海代理与智能 API 网关**：
- **第一优先级（正向出海代理）**：为开发者设备（PC / CLI / 移动端）提供低延迟、高可靠的 HTTP/HTTPS 正向出海代理（CONNECT 隧道 + WebSocket 待命连接池），一键环境管理与全平台节点接入。
- **第二优先级（API 反向代理网关）**：海外 API 服务（LLM 优先）的按需反向代理分发，零配置客户端 SDK，双上游智能路由与用量计量。

## 2. 用户故事

- 作为开发者，我在终端输入 `pproxy on` 即可瞬间让 Git / Cursor / npm / 命令行工具无缝出海，不用时输入 `pproxy off`。
- 作为开发者，我可以在手机（Wi-Fi 或 Clash Meta / Shadowrocket）上把 `pproxy` 当作 HTTP 代理节点，实现外网访问或规则分流。
- 作为开发者，我打开 Pony Proxy 桌面端面板，一眼看到出海隧道健康状态、反向路由与用量。
- 作为开发者，我一键添加新 API 路由（如 Gemini），系统自动选择上游并生效。
- 作为开发者，我为每台设备生成独立 token / Basic Auth 凭据，设备丢失时一键撤销。

## 3. 功能需求

### P0 — 核心功能（已实现）
| 模块 | 需求 |
|------|------|
| 正向出海代理 | HTTP/HTTPS CONNECT 隧道；Basic Auth / Token 鉴权；Gatekeeper 防爆破限流；TunnelPool 待命 WS 隧道（1 RTT 冷建连）；Allowlist 域名白名单与通用出海 |
| 环境代理管理 | CLI `pproxy on / off / status / env (suspend/resume/generate-script)`，跨 Shell (Bash/Zsh/Fish/PowerShell) 自动配置与 K8s 集群地址免代理探测 |
| 反向 API 网关 | token 鉴权（路径 token `/{token}/{route}/...`为主，`X-Pony-Token`为辅）；多上游路由分发（CF Worker / Vercel）；用量监控与 SQLite 存留 |
| 桌面端 | Tauri 2 客户端：系统托盘、Proxy 白名单/直连切换、Dashboard、Routes、Tokens、Usage 图表、自更新 |
| 管理面 API | REST API：token CRUD、user CRUD、路由 CRUD、用量查询、健康检查、实时测速 |

### P1 — 扩展与增强
- CF Tunnel 公网入口集成（手机/外网公网接入，域名 `access.example.com`）
- 通配 / 自定义出海 Allowlist 规则热管理
- 告警渠道扩展（webhook / 邮件 / 80% 用量阈值告警）

### P2 — 移动端与多节点
- Mobile 原生 App（Tauri 2 iOS/Android 控制端）
- 自定义多上游出口与智能多路径测速切换

## 4. 非目标（明确不做）

- **服务端 MITM 解密**：服务端坚持透明端到端 TLS 转发，绝不窃取或解密客户端 HTTPS 流量。
- **服务端原生 UDP 隧道**：底层依赖 Cloudflare Edge 运行时环境，仅支持 TCP 协议，不支持原生 UDP 游戏加速。
- **多租户 / 商业化**：专注个人与小团队极简自托管体验。

## 5. 关键体验指标

- 正向出海建连延迟：池化命中 < 100ms
- 环境代理切换：1 键即时生效
- 新 API 路由生效：< 1 秒
- 新设备接入：复制一行 proxy_url / base_url 即用
