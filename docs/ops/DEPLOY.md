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
| `GATE_TUNNEL_TOKEN` → `TUNNEL_TOKEN_HASH` | CF Gate Worker `wrangler versions secret put TUNNEL_TOKEN_HASH` + `wrangler versions deploy <version-id>`（wrangler 4，旧 `secret put` 已被拒）；Vercel env `TUNNEL_TOKEN_HASH`（改后必须重新部署，改 env 不自动生效） | 隧道 Bearer 鉴权；`TUNNEL_TOKEN_HASH = sha256(GATE_TUNNEL_TOKEN)`（`printf '%s'` 取 hash，禁用 `echo`）；** live 双端（CF+Vercel）必须同源**（VPS 为遗留备选未部署，不计入）；两端均做 `trim().toLowerCase()` 归一化；轮换后须同步重录 |
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
| `PPROXY_CLUSTER_PEERS` | 远端对等备灾节点地址列表（逗号分隔，如 `100.64.0.2:18899,100.64.0.3:18899`） |
| `PPROXY_CLUSTER_KEY` | 集群间通信签名密钥（跨节点转发签发 `X-Pony-Cluster-Ticket` 鉴权） |
| `USER_VERIFYING_KEY` | 多租户 Ed25519 验签公钥 Hex（各节点本地无状态验签，只读注入） |
| `GATE_ADMIN_TOKEN` | 轻量 Gate 服务管理接口 Bearer Token（用于黑名单撤销热推送） |
| `GATE_ADMIN` | 网关管理端点地址（CLI 撤销热推送目标，默认 `http://127.0.0.1:3101`） |
| `PPROXY_SERVICE_USER` | `m4_test.sh` 断言的服务运行用户（默认 `pproxy`） |

### 4. 桌面端配置

桌面端（Tauri）无需改源码：在「设置 → 方案 A」粘贴**授权码**（`pony-gate://` 口令或裸 token，口令自带端点）即可开通隧道；端点在客户端侧持久化，代码内回退默认端点仅为占位。

### 5. 凭据卫生（开源红线）

- 真实值只放不入库位置：`.secrets.env`（chmod 600）、`config.json`、`.pproxy.env`、`*.env.local`、`.vercel/`。
- 轮换 `GATE_TUNNEL_TOKEN` 纪律（P0，双出口 401 头号嫌疑）：
  1. `printf '%s' '<token>' | sha256sum` 取 hash（禁用 `echo`，防尾换行）；
  2. CF：`printf '%s' '<hash>' | npx wrangler versions secret put TUNNEL_TOKEN_HASH` → `npx wrangler versions deploy <version-id>`，记录版本 ID；
  3. Vercel：更新 env 后**必须重新部署**（`workflow_dispatch` 全量部署或 `vercel --prod`），改 env 不自动对 Fluid 实例生效；
  4. 留痕：时间/操作人/双端 hash 前 8/版本 ID/redeploy 顺序；用 `node scripts/collect-401-evidence.mjs --token-file <f>` 验证双端 OK 后再分发 `pony-gate://` 口令；
  5. 桌面端重录授权码前，先对照自检 fingerprint（H2 本地分叉未排除前不得定 H1）。
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
| 内部引擎健康 | `curl -s http://127.0.0.1:18899/`（启用了 HA Forwarder 时） | JSON 路由表 |
| 集群状态大盘 | `pproxy cluster status` | 节点列表与在线状态 |
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

## 多机分布式集群组网实操

在跨机器、跨局域网（如 Tailnet 内部网络）或海外轻量备灾节点（如 RackNerd VPS）场景下，通过种子节点与自愈加入令牌（One-Time Join Token）完成多机零接触接入组网。

### 1. 种子节点生成加入令牌

在已正常运行的主节点（种子节点）上生成加入令牌。令牌内嵌种子节点监听地址、集群通讯密钥、出海隧道端点及多租户验签公钥：

```bash
# 使用本机默认检测地址（优先 TAILSCALE_IP / HOST，端口 8899），有效时长 30 分钟
pproxy cluster token-create --valid-minutes 30

# 或显式指定种子节点的内网/Tailnet 访问地址与端口
pproxy cluster token-create --seed 100.64.0.1:8899 --valid-minutes 60
```

输出示例：
```
╔════════════════════════════════════════════════════════════════╗
║   ✓ 节点全量配置自愈加入令牌 (Zero-Touch Token) 生成成功       ║
╚════════════════════════════════════════════════════════════════╝
  集群标识:     pproxy-mesh
  种子节点:     100.64.0.1:8899
  有效时长:     30 分钟 (单次使用 / 0600 安全约束)
  自愈配置载荷: 出海端点=[已内嵌 ✓], 验签公钥=[已内嵌 ✓]
  加入令牌:     eyJjbHVzdGVyX2lkIjoicHByb3h5LW1lc2giL...
```

### 2. 工作节点一键入网自启

在全新服务器或备灾节点执行入网命令。`--auto-start` 参数将自动将配置写入 `~/.pony/cluster.json`（严格 `0600` 权限），自愈装配出海隧道配置与验签公钥，并在后台拉起局域网双模服务：

```bash
# 格式：pproxy cluster join --token "<TOKEN>" --auto-start
pproxy cluster join --token "eyJjbHVzdGVyX2lkIjoicHByb3h5LW1lc2giL..." --auto-start
```

若需覆盖种子节点地址（例如特定 NAT 穿透端口）：
```bash
pproxy cluster join --token "<TOKEN>" --peer 100.64.0.1:8899 --auto-start
```

### 3. 验证集群拓扑与节点状态

在集群内任意节点执行状态查询，查看全景健康大盘与对等节点连通性：

```bash
pproxy cluster status
```

期望输出包含当前节点角色、种子节点、已同步的出海网关与对等节点在线状态。

---

## 多租户管理运维

系统采用非对称隔离架构：管理端持有离线私钥签发租户凭据，生产网关节点仅分发公钥用于本地毫秒级无状态验签。

### 1. 私钥离线保存规范（`cluster_signing_key.hex` 0600）

**运维红线**：
- 严禁将签名私钥上传至生产网关节点、CI 构建机或写入 Git 仓库；
- 签名私钥仅允许保存在管理员离线控制机 `~/.pony/cluster_signing_key.hex`，权限必须保持 `0600`；
- 私钥丢失将无法签发新租户凭据；私钥泄露会导致未授权伪造租户令牌。

在离线管理机上初始化多租户签名密钥对：
```bash
pproxy user keygen
```

输出展示生成的公钥 Hex，并自动将私钥以 `0600` 权限落盘至 `~/.pony/cluster_signing_key.hex`：
```bash
# 校验私钥权限是否合规
ls -l ~/.pony/cluster_signing_key.hex
# 期望权限输出: -rw------- 1 user user 64 ... /home/user/.pony/cluster_signing_key.hex
```

### 2. 公钥分发（`USER_VERIFYING_KEY`）

公钥为 32 字节 Ed25519 公钥的 Hex 编码（64 字符十六进制）。将公钥分发至所有接入网关与代理节点，节点无需访问中心数据库即可实现无状态本地验签：

- **环境变量注入**（systemd 服务或 `/etc/environment`）：
  ```bash
  # /etc/systemd/system/pproxy.service.d/override.conf 或 gate-server 环境
  [Service]
  Environment="USER_VERIFYING_KEY=8f14e45fceea167a5a36dedd4bea2543..."
  ```
- **配置生效验证**：
  ```bash
  # 验证轻量 gate-server 日志回显
  journalctl -u gate-server -n 20 | grep "User Token Verifier enabled"
  ```

### 3. 租户令牌签发与撤销热生效

#### 3.1 签发租户商业化令牌
在离线管理机执行命令（私钥自动参与签名）：
```bash
# 签发 30 天有效、配额 50GB、最大 3 并发的租户令牌
pproxy user add alice --quota 50G --expires-days 30 --max-conns 3
```

#### 3.2 撤销令牌 / 封禁租户热生效
当租户逾期、滥用或凭据泄露时，执行撤销命令。系统支持传入完整令牌（`usr_live_...`）或指定用户名（`username`）：

```bash
# 导出网关管理鉴权口令（与网关部署环境变量 GATE_ADMIN_TOKEN 保持一致）
export GATE_ADMIN_TOKEN="your_secure_gate_admin_token"
export GATE_ADMIN="http://127.0.0.1:3101"   # 若网关在远端则配置对端地址

# 执行撤销
pproxy user revoke usr_live_3fa85f6476104b2f90a9866572e9d81a
# 或按用户名撤销
pproxy user revoke alice
```

#### 3.3 验证热生效
1. **CLI 回显核验**：
   ```
   落盘路径:   /home/user/.pony/revoked_tokens.txt
   网关热更新: 已生效 ✓ (http://127.0.0.1:3101)
   ```
2. **网关端点直接验证**：
   向网关管理接口发起热撤销推送：
   ```bash
   curl -i -X POST http://127.0.0.1:3101/api/user/revoke \
     -H "Authorization: Bearer ${GATE_ADMIN_TOKEN}" \
     -H "Content-Type: application/json" \
     -d '{"identifier": "usr_alice"}'
   # 期望返回: HTTP/1.1 200 OK，{"ok":true,"identifier":"usr_alice"}
   ```
3. **数据面拦截验证**：
   持已撤销令牌请求用户 Profile 或建立代理连接：
   ```bash
   curl -i http://127.0.0.1:3101/api/user/profile \
     -H "Authorization: Bearer usr_live_..."
   # 期望返回: HTTP/1.1 401 Unauthorized
   ```

---

## Local HA Forwarder 运维指引

Local HA Forwarder 是 pproxy 内置的原生本地高可用分发桩，用于彻底消除本地 AI 工具链（如 ponyllm、IDE 插件、自动化流水线）对单机 pproxy 进程重启、崩溃或升级时的单点故障敏感。

### 1. 端口职责分工与架构设计

| 端口 | 监听模式 | 进程属主 | 职责分工 |
|------|---------|---------|---------|
| `8899` | 外部稳定入口（`127.0.0.1:8899` 或 `0.0.0.0:8899`） | `ha-forwarder` 独立守护进程（PID 文件 `~/.pony/ha-forwarder.pid`） | **对外常驻统一门面**：ponyllm 与客户端配置唯一连接目标。与主引擎生命周期隔离，引擎重启或升级时端口永不关闭、连接不 reset。 |
| `18899` | 内部核心数据面（`0.0.0.0:18899`） | `pproxy serve` 主服务进程 | **实际代理运算引擎**：承载真实路由转发、出海 WebSocket 隧道连接、SQLite 统计与限流逻辑；供本地 ha-forwarder 零延迟转发及集群对等节点跨机互联。 |

### 2. 工作原理与故障转移（Failover）

1. **零延迟直通（正常态）**：
   `ha-forwarder` 维持后台异步探活（每 300ms 探测一次 `127.0.0.1:18899`）。正常情况下请求以 0ms 关键路径延迟直连本地 18899 引擎，零多余等待。
2. **零毫秒故障转移（异常态）**：
   当 `pproxy serve` 发生 SIGKILL、OOM 崩溃或热升级下线时，探活熔断器立即切断本地路由，后续接入请求 0 延迟秒级重定向至 `PPROXY_CLUSTER_PEERS` 或 `cluster.json` 内的远程备灾节点，同时注入带有 HMAC 签名的 `X-Pony-Cluster-Ticket` 凭据。
3. **无缝自愈回切**：
   本地引擎恢复监听后，熔断器自动重置，流量无缝回归本地，零人工介入。

### 3. 环境变量配置与启动方式

#### 3.1 环境变量与配置来源
- **候选对等节点**：
  优先读取环境变量 `PPROXY_CLUSTER_PEERS`（支持逗号/分号分隔的 SocketAddr，例如 `100.64.0.2:18899,100.64.0.3:18899`）；若未配置则回退至 `~/.pony/cluster.json` 中的 `seed_addr`。
- **集群机器鉴权密钥**：
  环境变量 `PPROXY_CLUSTER_KEY`（或从 `cluster.json` 读取 `cluster_auth_key`），用于跨节点故障转移时生成安全集群票证。

#### 3.2 生产环境 systemd 配置示例
在部署机创建 drop-in 配置 `/etc/systemd/system/pproxy.service.d/ha.conf`：
```ini
[Service]
Environment="PPROXY_CLUSTER_PEERS=100.64.0.2:18899,100.64.0.3:18899"
Environment="PPROXY_CLUSTER_KEY=0123456789abcdef0123456789abcdef"
```

加载配置并启动：
```bash
sudo systemctl daemon-reload
sudo systemctl restart pproxy
```

### 4. 运行验证与故障演练

#### 4.1 端口与进程双重检查
```bash
# 验证 8899 与 18899 同时处于 LISTEN 状态
ss -tlnp | grep -E '8899|18899'
# 期望：
# LISTEN  0  512  127.0.0.1:8899   (pproxy ha-forwarder)
# LISTEN  0  512  0.0.0.0:18899    (pproxy serve)

# 验证 PID 记录文件
cat ~/.pony/ha-forwarder.pid
```

#### 4.2 业务入口探活
```bash
# 请求对外 8899 入口，验证数据面返回
curl -s http://127.0.0.1:8899/ | head -c 100
```

#### 4.3 模拟引擎下线演练（零停机验证）
```bash
# 1. 模拟杀掉 18899 主引擎进程（模拟崩溃或升级）
fuser -k -9 18899/tcp

# 2. 立即请求 8899 门面端口，验证自动 failover 到远端备灾节点（TCP 端口依然联通）
curl -s -w "\nHTTP_CODE: %{http_code}\n" http://127.0.0.1:8899/

# 3. 重启恢复主服务
sudo systemctl restart pproxy
```
