# 运维手册：部署 / 更新 / 回滚

## 组件清单

| 组件 | 部署方式 | 位置 |
|------|---------|------|
| pony-server (pproxy-server) | systemd `pproxy.service` | dev 服务器 `/home/USER/pproxy/target/release/` |
| CF Worker | `wrangler deploy` | Cloudflare（edge.ponyjob.top） |
| Vercel 函数 | Vercel API（v13 deployments） | Vercel（vedge.ponyjob.top） |
| 凭据 | `.secrets.env`（600） | 本地，不部署 |

## 日常操作

### 更新 server（本机）
```bash
cd /home/USER/pproxy
cargo build --release
sudo systemctl restart pproxy
curl -s http://127.0.0.1:8899/ | head -c 100   # 健康检查
```

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
