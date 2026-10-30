# pproxy-engine

**嵌入式**网关引擎：把 CONNECT 数据面 + 反向网关装进任何进程（`pproxy serve` 走的就是它）。
与 `crates/server` 的区别：engine 只做数据面与本机网关，**不带管理面**；状态（`Store`/`TokenService`/
`RouteTable`/`Gatekeeper`）由宿主构造后注入。

## 配置项

- `EngineConfig`（`server.rs`）：`listen_addr` 默认 `127.0.0.1:8899`、`instance_uuid`（未给则生成）、
  `max_connections` 默认 `MAX_CONCURRENT_CONNECTIONS = 512`。
- `GatewayState`（宿主注入）：`tokens` / `users`（可选）/ `upstream` / `usage` / `gatekeeper` /
  `tunnel`（可选 `TunnelPool`）/ `instance_uuid` / `cluster_auth_key`（可选，集群票证验签键）。
- `TunnelConfig`（`connect.rs`）：gate WS 地址默认 `wss://gate.example.com/ws`、`MAX_ATTEMPTS = 5`、
  待命池 `POOL_SIZE = 2`、`DEFAULT_ALLOWLIST`；可被环境变量覆盖。
- 环境变量：`PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_ALLOWLIST`（隧道端点/令牌/
  白名单）、`PPROXY_YOUTUBE_PROXY`（YouTube 目标专属上游，见 `docs/architecture/CURRENT.md` 的分流说明）。
- 请求体上限 `MAX_BODY_SIZE = 32 MiB`（反向网关面）。

## 语义

- **数据面路由**（`gateway.rs::build_data_router`）：`/`（info，不鉴权）、`/__pproxy_health`、
  `/clash`、`/clash.yaml`、以及路径令牌形态 `/{token}/{route}/*path`。
- **CONNECT 鉴权**（`connect.rs`）：凭据来自 `Proxy-Authorization`（优先）或 `Authorization` 的 Basic，
  或 `X-Pony-Token` 头；主体三类（`AuthSubject`：User / Token / ClusterNode）。失败 → Gatekeeper
  `record_failure` → `407` + `proxy-authenticate: Basic realm="Pony Proxy"`；成功 `record_success` 清零。
- **回环豁免**：仅当来源是真实回环**且**请求不含 `X-Forwarded-For` / `X-Real-IP` / `Forwarded` 时免鉴权。
  带转发痕迹就走完整校验——否则任何反代都能把外部请求洗成"本机请求"。
- **CONNECT 放行面**：只允许 `443`（其余 `403 x-pproxy-reason: port_not_allowed`）；无隧道池 → `403
  tunnel_not_configured`；建连成功回 `200 Connection Established` 后双向裸字节中继。
- **allowlist 是提示不是门**：命中只记日志 tag，未命中同样建连（出口调序由 `pproxy-transport` 负责）。

## 限制

- **只有 CONNECT（443）**：不提供明文 HTTP 正向代理；absolute-form 请求不属于本面。
- **没有管理面**：token/路由/配额的增删改在 `crates/server`（管理 REST）或 `crates/cli`。
- **不做持久化**：库、令牌服务、路由表都由宿主注入；引擎重启不保留任何自有状态。
- **不起 TLS**：监听是裸 TCP，TLS 由客户端起的 CONNECT 隧道端到端承载。
- **进程内计数**：Gatekeeper 失败计数与并发上限是单实例内存态，多实例部署时各自计。

## 扩展点

- 新数据面端点：`gateway.rs` 的 router 构建处 + （若需鉴权）在 `auth.rs` 的凭据提取里加分支。
- 新鉴权方式：`auth.rs::extract_credentials` + `verify_connect_credentials`；两者都只在 engine 内实现一次，
  server 侧的对应逻辑走同一份 `pproxy-core` 原语（别复制第二份判据）。
- 新上游形态：`upstream.rs` 的 `UpstreamMode`（`Direct{edges}` / `Chained{config,client}`）加变体。
- 新隧道调度策略：`connect.rs` 的 `TunnelPool` 使用点与 `pproxy-transport` 的路由函数，不要在引擎里重写排序。
