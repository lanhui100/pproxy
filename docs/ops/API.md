# API 协议

## 数据面（:8899）

### 网关转发
```
http://127.0.0.1:8899/{route}/{path}?{query}
```
- `{route}`：config.json routes 键（anthropic/openai/opencode/google/github/x/facebook）
- 转写为 `上游?url=https://{target_host}/{path}?{query}`，透传 method/body/业务 header
- 剥离：hop-by-hop（host/connection/transfer-encoding/content-length...）+ geo 泄露头（x-forwarded-for/x-real-ip/cf-*...）
- 响应流式透传（SSE 兼容），`connection: close`

### 上游认证
- 网关 → 上游：`X-Proxy-Secret: <REDACTED_DEV_SECRET>`（服务器内注入，客户端无感）

### CONNECT
- 池空时直连目标（兜底）；无法经上游转发（见 ADR-001）

### 路由 → 上游映射
| 路由 | 上游 | 原因 |
|------|------|------|
| openai / opencode | vercel | CF 出口被地区策略拦截 |
| 其余 | worker (edge.ponyjob.top) | 默认 |

## 管理面（:8900，仅 127.0.0.1）

| 端点 | 方法 | 说明 |
|------|------|------|
| `/stats` | GET | 池状态（dynamic_count/static_count/countries/last_refresh） |
| `/refresh` | POST | 手动触发池刷新（当前池停用，无效果） |
| `/` | GET | （:8899）路由表 JSON |

## 上游协议（服务器 ↔ 出口）

### CF Worker（edge.ponyjob.top）
```
ANY https://edge.ponyjob.top/<任意路径>?url=<urlencoded target>
Header: X-Proxy-Secret: <REDACTED_DEV_SECRET>
```
- Worker 剥离 geo/hop-by-hop 头后 fetch 目标，流式回传
- 无密钥 → 403 "Unauthorized"

### Vercel 函数（vedge.ponyjob.top/api/proxy）
```
ANY https://vedge.ponyjob.top/api/proxy?url=<urlencoded target>
Header: X-Proxy-Secret: <REDACTED_DEV_SECRET>
```
- Node fetch 转发，流式回传（maxDuration 300s）
- vercel.app 域名有登录墙（ssoProtection），必须用自定义域名

## 错误语义速查

| 状态码 | 来源 | 含义 |
|--------|------|------|
| 400 Missing ?url= | 上游 | 网关→上游拼装错误 |
| 401 invalid_request_error | OpenAI | key 错误（地区已通） |
| 401 authentication_error | Anthropic | key 错误（链路通） |
| 403 Unauthorized | 上游 | X-Proxy-Secret 错误 |
| 403 unsupported_country_region_territory | OpenAI | 走错上游（应走 vercel） |
| 405 Method Not Allowed | Anthropic | GET 打 POST 端点（链路通） |
| 429 | Google | CF 出口限流（重试） |
| DataPolicyError | zen | 账号未 opt-in 数据政策 |
| RegionError | zen | 走错上游（应走 vercel） |
