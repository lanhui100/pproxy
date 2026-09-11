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

## 开源部署清单：占位符与凭据

> 本仓库为**开源中立形态**：所有私有域名已占位为 `*.example.com`，真实凭据一律不入库
> （`.secrets.env` / `config.json` / `.pproxy.env` / `*.env.local` / `.vercel/` 均被 `.gitignore` 忽略）。
> 克隆/自部署前，按下表逐项填写。**未改占位符会导致部署失败；未注凭据会导致边缘 fail-closed（403/401）。**

### 1. 部署前必须改回真实域名的文件

| 文件 | 占位位置 | 不改的后果 |
|---|---|---|
| `deploy/cf-worker/wrangler.toml` | `routes` 的 `pattern` | `wrangler deploy` 失败 |
| `deploy/cf-gate-worker/wrangler.toml` | 注释与绑定域名 | gate 隧道桥域名错误 |
| `deploy/cloudflared/config.yml` | `ingress.hostname` | CF Tunnel 入口域名错误 |
| `desktop/src-tauri/tauri.conf.json` | updater `endpoints` | 桌面端无法自动更新 |
| `scripts/install.sh` | CDN `get.example.com`、`GITHUB_RELEASE_BASE` | 一键安装下载源 404 |
| `scripts/publish-desktop-dist.sh` / `sync-desktop-release.sh` | `dl` / `access` 分发地址 | 桌面版发布/同步失败 |
| `scripts/m4_test.sh` | `PUBLIC_URL` | 生产冒烟测试打错端点 |

### 2. 必填凭据（环境变量/密钥注入，勿写进提交）

| 凭据 | 注入位置 | 说明 |
|---|---|---|
| `PROXY_SECRET` | CF Worker `wrangler secret put PROXY_SECRET`；Vercel env `PROXY_SECRET` | edge/vedge 共享上游密钥；缺失即 500 fail-closed |
| `GATE_TUNNEL_TOKEN` → `TUNNEL_TOKEN_HASH` | CF Gate Worker `wrangler secret put TUNNEL_TOKEN_HASH`；Vercel env `TUNNEL_TOKEN_HASH`；VPS 版 `.pony-gate.env` | 隧道 Bearer 鉴权；`TUNNEL_TOKEN_HASH = sha256(GATE_TUNNEL_TOKEN)`，**三端必须同源**，轮换后须同步重录 |
| `VERCEL_TOKEN` / `VERCEL_ORG_ID` / `VERCEL_PROJECT_ID_EDGE` / `VERCEL_PROJECT_ID_GATE` | GitHub Actions secrets | 供 `.github/workflows/deploy-vercel.yml` gitOps 部署 |

### 3. 可用环境变量覆盖的默认值（无需改代码）

| 变量 | 作用 |
|---|---|
| `PPROXY_EDGE_URL` / `PPROXY_VERCEL_URL` | `pproxy serve` 的上游出口端点（默认占位） |
| `PPROXY_DOWNLOAD_BASE` | `install.sh` 的二进制下载源（默认 GitHub Releases） |
| `PONY_DIST_URL` | CLI 自更新（`pproxy upgrade`）分发源 |
| `PPROXY_DESKTOP_DIST_DIR` | `/dsk/` 静态分发目录（默认 `/opt/pony-desktop-releases`） |
| `PPROXY_CONFIG` | server 配置文件路径（默认 `/etc/pproxy/config.json`） |
| `PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_ALLOWLIST` | 隧道端点/令牌/白名单（server 侧 `.pproxy.env`） |
| `PPROXY_LISTEN_ADMIN` | 管理面监听地址（tailnet 重绑，systemd drop-in） |
| `PPROXY_SERVICE_USER` | `m4_test.sh` 断言的服务运行用户（默认 `pproxy`） |

### 4. 桌面端配置

桌面端（Tauri）无需改源码：在「设置 → 方案 A」粘贴**授权码**（`pony-gate://` 口令或裸 token，口令自带端点）即可
开通隧道；端点在客户端侧持久化，代码内的回退默认端点仅为占位。

### 5. 凭据卫生（开源红线）

- 真实值只放不入库位置：`.secrets.env`（chmod 600）、`config.json`、`.pproxy.env`、`*.env.local`、`.vercel/`。
- 轮换 `GATE_TUNNEL_TOKEN` 后必须**同步三端 `TUNNEL_TOKEN_HASH`**（CF secret / Vercel env / VPS env）并让桌面端重录授权码，否则隧道 401。
- 本仓库 git 历史已做凭据清洗（filter-repo）；**后续提交严禁引入任何真实 token / 密钥字面量**。

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
