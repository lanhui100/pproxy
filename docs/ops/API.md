# API 协议

> M1 起数据面带 token 鉴权、管理面为完整 REST API（tokens/routes/usage/health）。路由存 SQLite，热生效。

## 数据面（:8899）

### 网关转发
```
路径模式（推荐）：http://127.0.0.1:8899/{token}/{route}/{path}?{query}
header 模式：    http://127.0.0.1:8899/{route}/{path}?{query}  +  Header: X-Pony-Token: {token}
```
- `{token}`：管理 API 创建的明文 token（`pony_` + 32 hex；仅创建时返回一次）
- `{route}`：SQLite routes 表 name（初始由 config.json 迁移导入）
- 鉴权失败（不存在/撤销/过期）统一 `401 {"error":"unauthorized"}` 同体，防枚举
- 首段以 `pony_` 开头即按路径模式处理（route 名创建时禁 `pony_` 前缀，无歧义）
- 转写为 `上游?url=https://{target_host}/{path}?{query}`，透传 method/body/业务 header
- 剥离：hop-by-hop + geo 泄露头 + `x-pony-token`（S-P1-1，两种模式都删，防 token 泄露给上游）
- body 上限 32MB（超限 413）；响应流式透传（SSE 兼容）

### 正向出海代理 (CONNECT 隧道)
```
CONNECT {host}:443 HTTP/1.1
Proxy-Authorization: Basic {base64(username:password)} 或 X-Pony-Token: {token}
```
- **鉴权**：支持 Basic Auth（users 表校验）与 Token Auth；未鉴权一律返回 `407 Proxy Authentication Required`
- **安全防爆破**：Gatekeeper 连续失败锁定 IP，返回 `429 Too Many Requests`
- **Allowlist 策略**：仅放行白名单域名（默认 OpenAI/Anthropic/Google/GitHub 等，支持通配/自定义扩展），未命中返回 `403 Forbidden (no_tunnel_route)`
- **传输**：通过 `TunnelPool` 待命 WebSocket 连接池向 `gate.ponyjob.top/ws` 发起 1-RTT 隧道绑定，随后进入双向二进制透传

- 网关 → 上游：`X-Proxy-Secret`（服务器内注入，客户端无感）

### 路由 → 上游映射
- 决策规则（resolve 实时决策）：routes 表 `override_upstream` 列优先；否则按 host 规则——`api.openai.com`/`opencode.ai` → vercel，其余 → worker
- `override_upstream` 可经管理 API PATCH 热改；`upstream` 列是创建时快照仅展示

## 外部访问（M4 公网入口）

```
https://access.ponyjob.top/{token}/{route}/{path}      # 与局域网路径模式完全同构
https://access.ponyjob.top/{route}/{path} + X-Pony-Token  # header 模式
```
- 链路：CF 边缘（TLS 终结）→ CF Tunnel（出站长连接，服务器零入站端口）→ 本机 :8899
- 鉴权与错误语义同数据面；token 等同密码，禁止写入客户端持久日志/剪贴板同步
- 平台限制（CF 免费层，Tunnel 不豁免）：非流式请求边缘等待 ~100s → 524；请求体上限 ~100MB；长任务必须 streaming
- 运维细节见 DEPLOY.md「CF Tunnel 公网入口」

## 管理面（:8900，仅 127.0.0.1，Bearer admin_token）

```
Authorization: Bearer <admin_token>
```
- admin_token 来源：首启生成打印一次（日志恰一次），或环境变量 `PPROXY_ADMIN_TOKEN` 注入（跳过生成）
- 鉴权失败统一 401 固定文案；错误响应统一 `{"error":"<固定文案>"}`，不透传内部信息

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/tokens` | POST | `{name, expires_days?}` → 201 `{id, name, token, expires_at}`（**明文仅此一次**） |
| `/api/tokens` | GET | 列表（脱敏 C-P1-8：不返回 token_hash/前缀，仅 id/name/created_at/expires_at/revoked_at/last_used_at/status） |
| `/api/tokens/{id}` | DELETE | 撤销（软删 revoked_at，即时生效）；admin 行拒绝（防自锁）→ 400 |
| `/api/routes` | GET | 列表（含 effective_upstream 实时决策） |
| `/api/routes` | POST | `{name, target_host, override_upstream?}` → 201；SSRF 校验（禁 IP/内网域名/端口/通配） |
| `/api/routes/{name}` | PATCH | 三态（double_option）：`override_upstream` 缺席=不改/null=清除/值=设置；`enabled` 同理；`upstream` 字段出现 → 400（快照列不可改） |
| `/api/routes/{name}` | DELETE | 删除（即时生效） |
| `/api/routes/{name}/test` | POST | 连通性实测 `{ok, status?, latency_ms?, error?}`（10s 超时独立 client） |
| `/api/usage` | GET | `?hours=24&route=&token_id=`（hours 限 1..=720）→ `{hours, since_hour, rows[{route,token_id,requests,bytes_in,bytes_out}], total}` |
| `/api/health` | GET | `{status, routes{name:{enabled,upstream}}, tokens_active, db}`；DB 探测失败 → 500 |
| `/api/alerts` | GET | `?unread=1&limit=50`（limit 钳制 ≤500）→ `{alerts:[{id,ts,level,message,read_at}]}`，倒序；level ∈ {warning, critical} |
| `/api/alerts/{id}/read` | POST | 标记已读 `{read:true}`；重复标记幂等 200（R4）；不存在 → 404 |
| `/api/quota` | GET | `{snapshots:[{ts,upstream,metric,used,quota,pct}], sources:[{name,state,last_ok}]}`——snapshots 为每 (upstream,metric) 最新值；sources 状态 ∈ {ok,disabled,error,unsupported_plan}；quota=-1/pct=-1 为"未知上限"哨兵（Vercel Hobby 降级） |

注：M0 的 `/stats`、`/refresh` 已随代理池停用一并下线。

### 监控配置（M3，全环境变量，config.json 零改动）
`PPROXY_CF_API_TOKEN`/`PPROXY_CF_ACCOUNT_TAG`（缺失 → cf 来源 disabled）、`PPROXY_VERCEL_TOKEN`/`PPROXY_VERCEL_TEAM_ID`、`PPROXY_ALERT_WEBHOOK_URL`（告警 POST `{event:"quota_alert", level, message, ts}`）、`PPROXY_ALERT_THRESHOLD_PCT`（默认 80，≥95 critical）、`PPROXY_POLL_INTERVAL_SEC`（默认 3600）；测试覆盖专用：`PPROXY_CF_GRAPHQL_URL`/`PPROXY_VERCEL_API_BASE`。

## 上游协议（服务器 ↔ 出口）

### CF Worker（edge.ponyjob.top）
```
ANY https://edge.ponyjob.top/<任意路径>?url=<urlencoded target>
Header: X-Proxy-Secret: <worker_secret>
```
- Worker 剥离 geo/hop-by-hop 头后 fetch 目标，流式回传
- 无密钥 → 403 "Unauthorized"

### Vercel 函数（vedge.ponyjob.top/api/proxy）
```
ANY https://vedge.ponyjob.top/api/proxy?url=<urlencoded target>
Header: X-Proxy-Secret: <vercel secret>
```
- Node fetch 转发，流式回传（maxDuration 300s）
- vercel.app 域名有登录墙（ssoProtection），必须用自定义域名

## 错误语义速查

| 状态码 | 来源 | 含义 |
|--------|------|------|
| 401 unauthorized | 网关 | token 缺失/无效/撤销/过期（同体防枚举） |
| 407 proxy_authentication_required | 正向代理 | CONNECT 隧道缺少或包含错误 Proxy-Authorization / Token |
| 403 no_tunnel_route | 正向代理 | CONNECT 目标 host 未在 Allowlist 白名单中 |
| 429 too_many_requests | 门禁 | 暴力破解多次失败触发 Gatekeeper IP 临时封锁 |
| 502 tunnel_failed | 正向代理 | Gate WebSocket 隧道建连失败 |
| 404 unknown_route | 网关 | route 名不在表中 |
| 502 upstream_error | 网关 | 上游未配置或请求失败（日志不含完整 URL） |
| 413 body_too_large | 网关 | body 超 32MB |
| 400 Missing ?url= | 上游 | 网关→上游拼装错误 |
| 401 invalid_request_error | OpenAI | key 错误（地区已通） |
| 401 authentication_error | Anthropic | key 错误（链路通） |
| 403 Unauthorized | 上游 | X-Proxy-Secret 错误 |
| 403 unsupported_country_region_territory | OpenAI | 走错上游（应走 vercel） |
| 405 Method Not Allowed | Anthropic | GET 打 POST 端点（链路通） |
| 429 | Google | CF 出口限流（重试） |
| DataPolicyError | zen | 账号未 opt-in 数据政策 |
| RegionError | zen | 走错上游（应走 vercel） |

## 运维提示

- admin_token 明文会进 journald（首启打印）：可用 `PPROXY_ADMIN_TOKEN` 注入规避；清理历史：`sudo journalctl --unit=pproxy --vacuum-time=1s`
- config.json 与 SQLite（默认 `~/.pony/state.db`）含敏感凭据：权限 600/700，勿入 git（`.gitignore` 已排除 config.json）
