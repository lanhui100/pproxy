# 运维手册：部署 / 更新 / 回滚

## 组件清单

| 组件 | 部署方式 | 位置 |
|------|---------|------|
| pony-server (pproxy-server) | systemd `pproxy.service` | dev 服务器 `/home/USER/pproxy/target/release/` |
| CF Worker | `wrangler deploy` | Cloudflare（edge.ponyjob.top） |
| Vercel 函数 | Vercel API（v13 deployments） | Vercel（vedge.ponyjob.top） |
| 桌面分发（updater 主端点） | `scripts/publish-desktop-dist.sh` | Vercel 静态（dl.ponyjob.top，项目 pony-dsk） |
| 凭据 | `.secrets.env`（600） | 本地，不部署 |

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
#    latest.json 的 platforms.*.url 指向 https://dl.ponyjob.top/<点号文件名>
# 3) Vercel 静态分发（updater 主端点，与 dev 在线状态无关）：
VERCEL_TOKEN=$(ssh dev 'grep "^PPROXY_VERCEL_TOKEN=" ~/pproxy/.pproxy.env' | cut -d= -f2) \
  scripts/publish-desktop-dist.sh \
  desktop/src-tauri/target/release/bundle/nsis latest.json
# 4) 过渡期回退端点（客户端 <0.3.18 只认 access.ponyjob.top/dsk/）：
ssh dev 'cd ~/pproxy && scripts/sync-desktop-release.sh desktop-vX.Y.Z'
# 5) 验证：curl -s https://dl.ponyjob.top/latest.json | grep version
```

> **分发拓扑（2026-08-30 起）**：updater 双端点容灾——主 `dl.ponyjob.top`（Vercel 静态，
> 项目 `pony-dsk`，与 dev 在线状态无关）+ 备 `access.ponyjob.top/dsk/`（dev 主机
> pproxy-server 数据面，pony-tunnel 隧道；客户端 <0.3.18 只认备端点）。dev 上的
> pproxy-server 不再承担"检查更新"的可用性，仅作过渡回退与 CLI 管理面。
> DNS：dl → cname.vercel.com（DNS only）；命名约定：分发文件名统一点号
> （Pony.Proxy_X.Y.Z_x64-setup.exe），tauri 产物空格由发布脚本归一。

### 更新 CF Worker
```bash
source /home/USER/pproxy/.secrets.env   # 如脚本需要
cd /home/USER/pproxy/deploy/cf-worker
wrangler deploy
```
注意：wrangler.toml 含 routes 配置（edge.ponyjob.top custom_domain）。

### 更新 Vercel 函数
```bash
# 经 API 部署（api.vercel.com 大陆直连可达），见 deploy/vercel/
# 部署后注意: 项目 ssoProtection=all_except_custom_domains
#   vercel.app 域名有登录墙，必须走 vedge.ponyjob.top（自定义域名无墙）
```

> **2026-08 审计整改**：生产已迁移至项目 `pproxy-edge-v2`（vedge.ponyjob.top 已重绑至新项目，
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
| Worker 可达 | `curl -o /dev/null -w '%{http_code}' https://edge.ponyjob.top/` | 403（未带密钥） |
| Vercel 可达 | `curl -o /dev/null -w '%{http_code}' https://vedge.ponyjob.top/api/proxy` | 400/403 |
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
| systemd | `systemd/pony-tunnel.service` → `/etc/systemd/system/`，User=dm |
| 公网入口 | `https://access.ponyjob.top/{token}/{route}/...` |
| metrics | `127.0.0.1:19099/metrics`（只读观测） |

### 更新 / 重启
```bash
sudo systemctl restart pony-tunnel && sleep 5
curl -s -o /dev/null -w '%{http_code}\n' https://access.ponyjob.top/openai/models   # 期望 401
```

### 回滚 / 卸载（公网入口关闭程序）
```bash
sudo systemctl disable --now pony-tunnel
sudo rm /etc/systemd/system/pony-tunnel.service && sudo systemctl daemon-reload
cd ~/pproxy && cloudflared tunnel route ip delete access.ponyjob.top   # 或 dashboard 删 CNAME
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

- 更新源 = `access.ponyjob.top/dsk/latest.json`，由本机 `pony-dsk-sync.timer`
  （每 15 分钟）检测新 tag 并自动执行 `scripts/sync-desktop-release.sh` 同步到
  `/home/USER/pony-desktop-releases`（服务端 `/dsk/:filename` 按请求实时读盘，无需重启）
- **2026-08-26 事故复盘**：0.3.7/0.3.8 发布后漏跑同步脚本，线上滞留 0.3.6，
  已装客户端「检查更新」永远提示最新。现已由 timer 兜底自动化；
  手动补同步仍可用 `scripts/sync-desktop-release.sh <tag>`
- 排查口令：`journalctl -u pony-dsk-sync.service -n 20`、
  `cat /home/USER/pony-desktop-releases/.synced-tag`（应等于最新 tag）、
  `curl -s https://access.ponyjob.top/dsk/latest.json | grep version`
