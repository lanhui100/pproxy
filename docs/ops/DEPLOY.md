# 运维手册：部署 / 更新 / 回滚

## 组件清单

| 组件 | 部署方式 | 位置 |
|------|---------|------|
| pony-server (pproxy-server) | systemd `pproxy.service` | dev 服务器 `/home/USER/pproxy/target/release/` |
| CF Worker | `wrangler deploy` | Cloudflare（edge.example.com） |
| Vercel 函数 | Vercel API（v13 deployments） | Vercel（vedge.example.com） |
| 桌面分发（updater 主端点） | `scripts/publish-desktop-dist.sh` | Vercel 静态（dl.example.com，项目 pony-dsk） |
| 凭据 | `.secrets.env`（600） | 本地，不部署 |

## 开源部署清单（占位符与凭据）

> 与 [README「开源部署清单」](../../README.md) 同源维护。本仓库为开源中立形态：私有域名已占位为
> `*.example.com`，真实凭据不入库（`.secrets.env` / `config.json` / `.pproxy.env` / `*.env.local` / `.vercel/` 均已 gitignore）。
> **未改占位符会导致部署失败；未注凭据会导致边缘 fail-closed（403/401）。**

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
| `PPROXY_DIST_BASE` | 桌面版静态分发基础地址（`publish-desktop-dist.sh` 写入 latest.json 的下载源，默认占位） |
| `PONY_DIST_URL` | CLI 自更新（`pproxy upgrade`）分发源 |
| `PPROXY_DESKTOP_DIST_DIR` | `/dsk/` 静态分发目录（默认 `/opt/pony-desktop-releases`） |
| `PPROXY_CONFIG` | server 配置文件路径（默认 `/etc/pproxy/config.json`） |
| `PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_ALLOWLIST` | 隧道端点/令牌/白名单（server 侧 `.pproxy.env`） |
| `PPROXY_LISTEN_ADMIN` | 管理面监听地址（tailnet 重绑，systemd drop-in） |
| `PPROXY_SERVICE_USER` | `m4_test.sh` 断言的服务运行用户（默认 `pproxy`） |

### 4. 桌面端配置

桌面端（Tauri）无需改源码：在「设置 → 方案 A」粘贴**授权码**（`pony-gate://` 口令或裸 token，口令自带端点）即可开通隧道；端点在客户端侧持久化，代码内回退默认端点仅为占位。

### 5. 凭据卫生（开源红线）

- 真实值只放不入库位置：`.secrets.env`（chmod 600）、`config.json`、`.pproxy.env`、`*.env.local`、`.vercel/`。
- 轮换 `GATE_TUNNEL_TOKEN` 后必须**同步三端 `TUNNEL_TOKEN_HASH`**（CF secret / Vercel env / VPS env）并让桌面端重录授权码，否则隧道 401。
- 本仓库 git 历史已做凭据清洗（filter-repo）；**后续提交严禁引入任何真实 token / 密钥字面量**。

## 日常操作

### 更新 server（本机）
```bash
cd /home/USER/pproxy
cargo build --release
sudo systemctl restart pproxy
curl -s http://127.0.0.1:8899/ | head -c 100   # 健康检查
```

### 发桌面版（Windows，含自更新分发）
```bash
# 1) 本机（Windows）构建：nsis + updater 签名
#    签名私钥在 dev 主机 ~/.tauri/pony-desktop.key（公钥须与 tauri.conf.json 一致）
cd desktop && pnpm tauri build
# 2) GitHub Release 归档源（tag desktop-vX.Y.Z；gh 已认证）：
#    上传产物 *_x64-setup.exe / .sig / latest.json
#    latest.json 的 platforms.*.url 指向 https://dl.example.com/<点号文件名>
# 3) Vercel 静态分发（updater 主端点，与 dev 在线状态无关）：
VERCEL_TOKEN=$(ssh dev 'grep "^PPROXY_VERCEL_TOKEN=" ~/pproxy/.pproxy.env' | cut -d= -f2) \
  scripts/publish-desktop-dist.sh \
  desktop/src-tauri/target/release/bundle/nsis latest.json
# 4) 过渡期回退端点（客户端 <0.3.18 只认 access.example.com/dsk/）：
ssh dev 'cd ~/pproxy && scripts/sync-desktop-release.sh desktop-vX.Y.Z'
# 5) 验证：curl -s https://dl.example.com/latest.json | grep version
```

> **分发拓扑（2026-08-30 起，2026-09 存储治理与防爆升级）**：
> 1. **主分发**：支持 **Cloudflare R2 / S3 对象存储**（零出网费，永久保留历史版本且无空间爆炸上限）与 **Vercel 静态托管**（双轨自适应）。走 Vercel 时发布脚本自动聚合最近 3 个历史版本，彻底解决“快照覆盖导致老版本 404”；
> 2. **备端点**：`access.example.com/dsk/`（dev 主机数据面），`sync-desktop-release.sh` 内置轮转淘汰策略（默认只保留最近 3 个版本安装包与签名，旧版本自动淘汰，杜绝磁盘撑爆 `No space left on device`）；
> 3. **对象存储直传配置**：设置 `R2_BUCKET`（或 `S3_BUCKET`）及 `R2_ENDPOINT`，发布脚本优先直传桶内；未配置时无缝回退 Vercel。
> DNS：dl → cname.vercel.com（或 R2 custom domain）；命名约定：分发文件名统一点号
> （Pony.Proxy_X.Y.Z_x64-setup.exe），tauri 产物空格由发布脚本归一。

### 更新 CF Worker
```bash
source /home/USER/pproxy/.secrets.env   # 如脚本需要
cd /home/USER/pproxy/deploy/cf-worker
wrangler deploy
```
注意：wrangler.toml 含 routes 配置（edge.example.com custom_domain）。

### 更新 Vercel 函数
```bash
# 经 API 部署（api.vercel.com 大陆直连可达），见 deploy/vercel/
# 部署后注意: 项目 ssoProtection=all_except_custom_domains
#   vercel.app 域名有登录墙，必须走 vedge.example.com（自定义域名无墙）
```

> **2026-08 审计整改**：生产已迁移至项目 `pproxy-edge-v2`（vedge.example.com 已重绑至新项目，
> `deploy/vercel/.vercel/` 已 link 过去）。原因：旧项目 `pproxy-edge` 被平台滥用检测标记，
> API token 部署一律 BLOCKED（hello-world 对照实验可正常部署，确认为项目级拦截而非账号级）。
> 旧项目暂保留作回滚，确认稳定后可在 dashboard 删除。再遇 BLOCKED 时：先用无关内容对照
> 测试区分账号级/项目级；项目级则新建项目→迁域名即可恢复。

### 回滚 server
```bash
cd /home/USER/pproxy && git log --oneline -5     # 找上一个可用 commit
git checkout <commit> && cargo build --release
sudo systemctl restart pproxy
```

## 配置变更

`config.json` 修改后需 `sudo systemctl restart pproxy` 生效。
路由表/上游/密钥均在 config.json（M1 后迁移 SQLite，路由可热更）。

## 凭据管理

- `.secrets.env`：VERCEL_TOKEN / HF_TOKEN / OPENCODE_ZEN_KEY / PROXY_SECRET
- 使用：`source /home/USER/pproxy/.secrets.env`
- CF wrangler OAuth 由工具自管（`~/.wrangler/`），过期时 `wrangler login`
- 轮换：各平台控制台轮换后同步更新 .secrets.env

## 监控点

| 检查 | 命令 | 期望 |
|------|------|------|
| 服务状态 | `systemctl is-active pproxy` | active |
| 网关健康 | `curl -s http://127.0.0.1:8899/` | JSON 路由表 |
| 池状态 | `curl -s http://127.0.0.1:8900/stats` | JSON |
| Worker 可达 | `curl -o /dev/null -w '%{http_code}' https://edge.example.com/` | 403（未带密钥） |
| Vercel 可达 | `curl -o /dev/null -w '%{http_code}' https://vedge.example.com/api/proxy` | 400/403 |
| 全路由体检 | （M2: pony doctor） | — |

日志：`journalctl -u pproxy -f`

## CF Tunnel 公网入口（pony-tunnel.service，M4）

### 组件

| 项 | 值 |
|----|-----|
| 二进制 | cloudflared（pkg.cloudflare.com apt 源，`/usr/local/bin/cloudflared`） |
| 隧道 | `pony-access`（UUID 见 `~/.cloudflared/config.yml`） |
| 凭据 | `~/.cloudflared/`（cert.pem + tunnel UUID.json + config.yml，均 dm/600，不入库） |
| 配置模板 | `deploy/cloudflared/config.yml`（占位符 `<TUNNEL_ID>`） |
| systemd | `systemd/pony-tunnel.service` → `/etc/systemd/system/`，User=pproxy |
| 公网入口 | `https://access.example.com/{token}/{route}/...` |
| metrics | `127.0.0.1:19099/metrics`（只读观测） |

### 更新 / 重启
```bash
sudo systemctl restart pony-tunnel && sleep 5
curl -s -o /dev/null -w '%{http_code}\n' https://access.example.com/openai/models   # 期望 401
```

### 回滚 / 卸载（公网入口关闭程序）
```bash
sudo systemctl disable --now pony-tunnel
sudo rm /etc/systemd/system/pony-tunnel.service && sudo systemctl daemon-reload
cd ~/pproxy && cloudflared tunnel route ip delete access.example.com   # 或 dashboard 删 CNAME
cloudflared tunnel delete pony-access                                   # 需先确认隧道已停
# 局域网路径不受影响：http://127.0.0.1:8899 照常服务
```

### 运维红线
- **本机 WARP 与本隧道互斥**：`warp-cli connect` 会把 cloudflared 出站连接卷入 WARP 隧道（延迟/可达性全部污染）。两者只能二选一连接。
- **禁止** `cloudflared service install`——会生成同名 root 权限 unit 覆盖加固配置；本机 unit 名为 `pony-tunnel.service` 即为规避。
- 凭据命令（login/create/route dns）一律以 dm 身份执行，sudo 执行会产生 root 属主文件导致服务无限崩溃循环。
- 平台限制：非流式请求 >100s 被 CF 边缘 524；请求体上限 ~100MB。长任务走 streaming。

## 管理面 tailnet 重绑（ADR-007，M5 落地）

- `pproxy.service` 不内嵌地址（tailnet 标识不入库）：实际值由部署机 drop-in 注入
  `Environment=PPROXY_LISTEN_ADMIN=<TAILNET_IP>:8900`
  （/etc/systemd/system/pproxy.service.d/override.conf；节点重新认证才会变更，变更时需同步 drop-in 与 Windows GUI Settings）
- 防火墙放行：`sudo ufw allow in on tailscale0 to any port 8900 proto tcp`
- 回滚 = 删除 drop-in 的 Environment 行（未注入时应用回落 127.0.0.1:8900）+ `sudo ufw delete allow in on tailscale0 ...` + restart
- GUI 连接失败三分支：地址解析失败→检查 Tailscale 连接；拒绝→检查 ufw 规则；
  超时→检查对端入网状态

## 桌面版发布与更新源（/dsk/）

```bash
# 发布：打 tag 触发 GitHub Actions 构建 NSIS 安装包 → GitHub Release
git tag desktop-v0.3.x && git push origin desktop-v0.3.x
```

- 更新源 = `access.example.com/dsk/latest.json`，由本机 `pony-dsk-sync.timer`
  （每 15 分钟）检测新 tag 并自动执行 `scripts/sync-desktop-release.sh` 同步到
  `/home/USER/pony-desktop-releases`（服务端 `/dsk/:filename` 按请求实时读盘，无需重启）
- **2026-08-26 事故复盘**：0.3.7/0.3.8 发布后漏跑同步脚本，线上滞留 0.3.6，
  已装客户端「检查更新」永远提示最新。现已由 timer 兜底自动化；
  手动补同步仍可用 `scripts/sync-desktop-release.sh <tag>`
- 排查口令：`journalctl -u pony-dsk-sync.service -n 20`、
  `cat /home/USER/pony-desktop-releases/.synced-tag`（应等于最新 tag）、
  `curl -s https://access.example.com/dsk/latest.json | grep version`
