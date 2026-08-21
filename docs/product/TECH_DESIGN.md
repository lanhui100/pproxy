# Pony Proxy — 技术方案

> 版本: v0.1 | 日期: 2026-08-21

## 1. 总体架构

```
┌─ pony-cli (Rust) ───┐
├─ pony-desktop (Tauri 2 + React) ─┐
└─ pony-mobile (Tauri 2, P2) ──────┘
        │ 管理 API（admin token 鉴权）
        ▼
pony-server (Rust + axum, dev 服务器, systemd 常驻) ← 唯一状态源
  ├─ 数据面 :8899  /{token}/{route}/...  → 上游分发（token 鉴权）
  ├─ 管理面 :8900  /api/*（tokens/routes/usage/health/alerts）
  ├─ 上游: CF Worker (edge.ponyjob.top) + Vercel (vedge.ponyjob.top)
  ├─ CF Tunnel → access.ponyjob.top（公网入口，手机/外网）
  └─ 监控: 请求计数 → SQLite(30天)；限额轮询(1h) → 80% 告警
```

原则：**后端唯一状态源**；CLI/GUI 均为管理 API 客户端，无本地状态同步。

## 2. 模块设计

### 2.1 pony-server（改造现有 pproxy-server）
```
crates/
  core/     # EdgeClient、路由匹配、上游选择策略（高内聚，无 IO 依赖）
  server/   # axum: 数据面 + 管理面 + 监控轮询
```

**鉴权**
- 数据面：URL 路径 token `http://host:8899/{token}/openai/v1/...`（兼容一切 SDK，无需自定义 header）；辅以 `X-Pony-Token` header
- token 存储：SHA-256 哈希落库，明文仅创建时展示一次
- 管理面：`Authorization: Bearer <admin_token>`（首个 admin token 首次启动生成，打印到日志）

**路由与上游自动选择**
```rust
enum Upstream { Worker, Vercel }
fn pick_upstream(target_host: &str) -> Upstream {
    match target_host {
        // 已知对 CF 数据中心 IP 敏感
        "api.openai.com" | "opencode.ai" => Upstream::Vercel,
        _ => Upstream::Worker,   // 默认
    }
} // route.upstream_override 可覆盖
```

**SQLite 表**（rusqlite, `~/.pony/state.db`）
```sql
tokens(id, name, token_hash UNIQUE, created_at, expires_at, revoked_at, last_used_at)
routes(name UNIQUE, target_host, upstream, override_upstream, enabled, created_at)
usage_hourly(ts_hour, route, token_id, requests, bytes_in, bytes_out)
quota_snapshots(ts, upstream, metric, used, limit, pct)
alerts(id, ts, level, message, read_at)
```

**监控轮询**（tokio interval，1h）
- CF：GraphQL Analytics API（Workers 请求数/日）
- Vercel：`GET /v1/usage`（函数调用/带宽）
- 阈值 80% → 写 alerts + 触发 webhook（P1）

**配置迁移**：config.json → 启动时导入 SQLite，此后 SQLite 为准（config.json 仅保留 listen/bind 等基础设施项）。

### 2.2 pony-cli（Rust + clap，同 workspace）
```
pony status                      # 服务/上游健康 + 用量摘要
pony start|stop|restart          # systemd 服务开关（本机）或远程 API
pony route list|add|rm|test      # 白名单管理（add 自动选上游）
pony token create <name> [--expires] | list | revoke <id>
pony usage [--days 7] [--route]  # 用量报表
pony doctor                      # 全路由连通性一键体检
pony config export <service> [--token <name>]   # 输出 env 片段
```
连接配置 `~/.pony/config.toml`：`server = "http://192.168.101.161:8900"` + admin token。

### 2.3 pony-desktop（Tauri 2 + React 18 + TS + Tailwind + shadcn/ui）
| 页面 | 内容 |
|------|------|
| Dashboard | 服务状态灯、7 路由健康、今日请求/流量、最近告警 |
| Routes | 白名单列表、添加（输入名称+host，自动上游可覆盖）、连通性测试 |
| Tokens | 设备 token 列表（名称/最后使用/过期）、创建（一次性明文展示）、撤销 |
| Usage | 按路由/token 图表（日粒度）、上游限额进度条（CF/Vercel） |
| Settings | 后端地址、admin token、告警阈值、CF Tunnel 状态 |

通知：tauri-plugin-notification，轮询 `/api/alerts?unread=1`（5min）。

打包：NSIS 安装包 + tauri-plugin-updater（P1 自动更新）。

### 2.4 公网入口（P1，M4）
- cloudflared tunnel（systemd），`access.ponyjob.top` → `http://127.0.0.1:8899`
- 手机/外网 SDK base_url：`https://access.ponyjob.top/{token}/{route}/...`
- TLS 由 CF 边缘提供；路径 token 鉴权；国内可达性已由 edge.ponyjob.top 验证

## 3. 安全

- 所有 token 哈希存储；admin token 与数据 token 分离
- 数据面限速（每 token 60 req/min，可配）防滥用
- CF Tunnel 仅暴露数据面；管理面仅绑 127.0.0.1（远程管理经 SSH 隧道，P2 再评估管理面公网化）
- PROXY_SECRET 上游密钥不出服务器（客户端永远只见 pony token）

## 4. 风险

| 风险 | 缓解 |
|------|------|
| Gemini 等新服务的上游敏感度未知 | route test 命令实测 + 手动覆盖 |
| CF/Vercel 限额 API 变动 | 轮询失败降级为本地计数，告警标注数据源 |
| CF Tunnel 免费额度政策变化 | 架构上 Tunnel 可选，局域网模式始终可用 |
| Vercel 300s 超时极端长响应 | 告警 + 文档标注；P2 支持自定义上游兜底 |
