# Pony Proxy (pproxy)

自托管开发者出海代理与智能 API 网关：
- **第一优先级（正向出海代理）**：基于 HTTP/HTTPS CONNECT 隧道与 WebSocket 待命连接池，提供 CLI 一键环境代理管理（`pproxy on / off / status / env`）、Windows 桌面端白名单代理与移动端/全平台 HTTP 节点接入，毫秒级出海。
- **第二优先级（API 反向代理网关）**：LLM 优先的多上游智能分发（`/{token}/{route}/*`），按需代理，多节点出口（CF Worker / Vercel AWS IP），自带用量统计与 Token 鉴权。

## 架构总览

```
[开发终端 / 浏览器 / 手机客户端]
   │
   ├─ 1. 正向代理流量 (CONNECT / HTTP Proxy)
   │     └─ pony-engine (:8899 / :18900) ──[TunnelPool 待命 WS 隧道]──> gate.example.com (CF gate-worker)
   │           └─ Basic Auth / Token 鉴权 + Allowlist 过滤 ──> 目标海外站点 (TCP 443)
   │
   └─ 2. 反向 API 网关流量 (/{token}/{route}/*)
         └─ pony-server (dev 服务器, 127.0.0.1:8899)
               ├─ anthropic/google/github/x/facebook → CF Worker  (edge.example.com)
               └─ openai/opencode                   → Vercel 函数 (vedge.example.com, AWS 出口)
```

- **双模架构**：正向 CONNECT 隧道（支持通用出海与全平台代理节点）+ 反向 HTTP 网关（`/{token}/{route}/*path` 零配置客户端 SDK）。
- **待命连接池（TunnelPool）**：预建 WebSocket 会话，将正向出海冷建连延迟压低至 1 RTT。
- **安全与控制**：强制 Basic Auth / Token 鉴权 + Gatekeeper 防爆破限流，路由/Token/用量存 SQLite（`~/.pony/state.db`）。

## 快速开始

### 1. 本机开发环境代理（正向出海）

```bash
# 开启当前 shell / 持久化环境代理（自动配置 http_proxy / https_proxy / no_proxy）
pproxy on

# 查看代理运行状态与外网连通性测速
pproxy status

# 临时挂起 / 恢复代理
eval "$(pproxy env suspend)"
eval "$(pproxy env resume)"

# 关闭环境代理
pproxy off
```

### 2. 移动端与全平台客户端接入（作为出海节点）

* **启动局域网共享服务**：
  ```bash
  pproxy serve --lan   # 绑定 0.0.0.0 并自动提示手机可连接的局域网 IP 与端口
  ```
* **手机 Clash Meta 扫码一键导入（最便捷）**：
  ```bash
  pproxy clash         # 自动生成配置并在终端打印二维码，手机 Clash 扫码即用
  ```
* **手机系统 Wi-Fi 代理**：Wi-Fi 设置中配置 HTTP 代理 `http://<电脑局域网IP>:8899`，输入账号密码（Basic Auth 或 Token）。
* **手机 VPN 客户端（Clash Meta / Shadowrocket / Surge 等）**：添加 HTTP 代理节点指向 `pproxy`，通过手机端 TUN 虚拟网卡实现**全局 VPN**或**基于规则的智能分流**。

### 3. API 反向代理网关使用

```bash
# 创建数据面 token（明文仅返回一次）
curl -X POST http://127.0.0.1:8900/api/tokens \
  -H "Authorization: Bearer <admin_token>" -H "Content-Type: application/json" \
  -d '{"name":"my-laptop"}'

# 使用（示例：Anthropic）
export ANTHROPIC_BASE_URL=http://127.0.0.1:8899/<token>/anthropic
export ANTHROPIC_API_KEY=<your-key>

# 健康检查（管理面）
curl http://127.0.0.1:8900/api/health -H "Authorization: Bearer <admin_token>"
```

- admin_token：首启日志打印一次，或以 `PPROXY_ADMIN_TOKEN` 环境变量注入。
- 完整协议见 [docs/ops/API.md](docs/ops/API.md)。

## 当前路由 (API 反向网关)

路由存 SQLite 热管理（`/api/routes` 增删改即时生效）。初始 7 路由由 config.json 迁移：

| 路由 | 目标 | 上游 |
|------|------|------|
| /anthropic | api.anthropic.com | CF Worker |
| /google | www.google.com | CF Worker |
| /github | github.com | CF Worker |
| /x | api.twitter.com | CF Worker |
| /facebook | www.facebook.com | CF Worker |
| /openai | api.openai.com | Vercel |
| /opencode | opencode.ai | Vercel |

## 文档索引

| 文档 | 内容 |
|------|------|
| [docs/product/PRD.md](docs/product/PRD.md) | 产品需求与定位（正向代理优先 + API 反向网关） |
| [docs/product/TECH_DESIGN.md](docs/product/TECH_DESIGN.md) | 目标态技术方案 |
| [docs/product/ROADMAP.md](docs/product/ROADMAP.md) | 里程碑 M1-M6 |
| [docs/architecture/CURRENT.md](docs/architecture/CURRENT.md) | 当前运行系统架构（双模拓扑与隧道） |
| [docs/architecture/decisions/](docs/architecture/decisions/) | 架构决策记录（ADR） |
| [docs/ops/DEPLOY.md](docs/ops/DEPLOY.md) | 部署、更新、回滚 |
| [docs/ops/API.md](docs/ops/API.md) | 数据面/管理面协议 |
| [docs/ops/TROUBLESHOOTING.md](docs/ops/TROUBLESHOOTING.md) | 故障排查手册 |

## 代码结构

```
crates/transport/ # 跨平台底层传输协议（WS 隧道 / 待命连接池 / 重试 / Ping-Pong 保活）
crates/engine/    # 嵌入式网关核心引擎（CONNECT 隧道 / Basic&Token 鉴权 / 防爆破门禁）
crates/core/      # store（SQLite）/token/route/usage/EdgeClient（上游转发协议）
crates/server/    # 独立守护服务（gateway 分发 + 管理 API）
crates/cli/       # CLI 工具链（环境代理 on/off/env/status、服务管理、路由与用量管理）
desktop/          # Windows 桌面客户端（Tauri 2 + 托盘 + 白名单代理引擎）
deploy/cf-worker/      # CF Worker（edge.example.com）
deploy/cf-gate-worker/ # CF Gate Worker（gate.example.com，出海 WS↔TCP 隧道桥）
deploy/vercel/         # Vercel 函数（vedge.example.com）
config.json       # 运行配置（上游、密钥；路由已迁 SQLite，勿提交）
systemd/          # pproxy.service
.secrets.env      # 凭据（chmod 600，勿提交）
```
