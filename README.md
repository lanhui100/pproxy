# Pony Proxy (pproxy)

自托管个人智能代理网关：让国内设备无障碍访问海外 API 服务（LLM 优先）。按需代理，不影响国内流量，零信用卡成本。

## 架构总览

```
客户端 SDK (base_url 指向网关)
  → pony-server (dev 服务器, systemd 常驻, 127.0.0.1:8899)
      ├─ anthropic/google/github/x/facebook → CF Worker  (edge.ponyjob.top)
      └─ openai/opencode                   → Vercel 函数 (vedge.ponyjob.top, AWS 出口)
```

- 数据面协议：HTTP 网关模式（`/{route}/*path` → 上游 `?url=` 转发），非 CONNECT 隧道
- 上游密钥 `X-Proxy-Secret` 仅存服务器，客户端无感

## 快速开始

```bash
# 服务管理
sudo systemctl status pproxy

# 使用（示例：Anthropic）
export ANTHROPIC_BASE_URL=http://127.0.0.1:8899/anthropic
export ANTHROPIC_API_KEY=<your-key>

# 健康检查
curl http://127.0.0.1:8899/
curl http://127.0.0.1:8900/stats
```

## 当前路由

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
| [docs/product/PRD.md](docs/product/PRD.md) | 产品需求（Pony Proxy 产品化） |
| [docs/product/TECH_DESIGN.md](docs/product/TECH_DESIGN.md) | 目标态技术方案 |
| [docs/product/ROADMAP.md](docs/product/ROADMAP.md) | 里程碑 M1-M6 |
| [docs/architecture/CURRENT.md](docs/architecture/CURRENT.md) | 当前运行系统架构 |
| [docs/architecture/decisions/](docs/architecture/decisions/) | 架构决策记录（ADR） |
| [docs/ops/DEPLOY.md](docs/ops/DEPLOY.md) | 部署、更新、回滚 |
| [docs/ops/API.md](docs/ops/API.md) | 数据面/管理面协议 |
| [docs/ops/TROUBLESHOOTING.md](docs/ops/TROUBLESHOOTING.md) | 故障排查手册 |

## 代码结构

```
crates/core/    # EdgeClient（上游转发协议）、代理池（已停用）、relay
crates/server/  # 数据面网关 + stats API
deploy/cf-worker/   # CF Worker（edge.ponyjob.top）
deploy/vercel/      # Vercel 函数（vedge.ponyjob.top）
deploy/hf-space/    # 已废弃（免费层不含 Docker）
config.json     # 运行配置（路由、上游、密钥）
systemd/        # pproxy.service
.secrets.env    # 凭据（chmod 600，勿提交）
```
