# T3 — 数据面改造（axum 化 + 路径鉴权）

> 依赖: T2（TokenService）| 波次 3 前半（与 T6 串行，先执行）| 上游: M1 spec §3.2

## 1. 目标

1. 数据面从裸 TCP 手写 HTTP 解析迁移到 **axum 0.7**（裁决 #1，理由与风险见 §2）。
2. 实现路径 token 鉴权：`/{token}/{route}/{path}?{query}` 主模式 + `X-Pony-Token` header 辅模式 + 401 语义。
3. 根路径信息端点；**CONNECT 直接禁用（403，P0-1 裁决）**。
4. main.rs 支持环境变量覆盖（T8 集成测试依赖）。

## 2. 设计裁决

### 2.1 数据面迁 axum（裁决 #1）

**裁决：迁移。**

理由：
1. 现有手写解析（main.rs `read_request_head`/`parse_headers`/`read_body`）存在已知边界缺陷：不处理 `Transfer-Encoding: chunked` 请求体、不处理 HTTP/1.0 无 Content-Length、header 重复键静默丢弃、pipelined 字节直接丢弃。SDK（openai-python 等）在重试/流式场景会产生这些形态。
2. axum 0.7 已在管理面使用（hyper 解析器），数据面复用同栈消除双解析器维护成本。
3. SSE：axum `Response<Body>` 直接透传 `reqwest` 的 `bytes_stream`，与现有 `resp.chunk()` 循环等价，无缓冲风险。

风险与对策：
- **风险 A：CONNECT 无法由 axum 表达**（hyper 会尝试解析 CONNECT 为普通请求）。对策见 §2.2——CONNECT 不再被承载，直接禁用。
- **风险 B：行为回归**（header 过滤、connection: close 语义）。对策：T8 集成测试覆盖 7 路由转发 + SSE；`EdgeClient` 不动，转发路径仅换"解析进/写出"外壳。
- **风险 C：绝对形式请求行**（`GET http://host/path`）。hyper 已将绝对形式解析为 `Uri`，axum 侧 `uri.path()+uri.query()` 即可，无需特判。

### 2.2 CONNECT 禁用（P0-1 裁决，安全）

**裁决：CONNECT 直接禁用，返回 `403 {"error":"connect_forbidden"}` 后关闭连接。**

理由：
1. 代理池已停用，CONNECT 仅剩"直连兜底"路径——而直连兜底正是攻击面：无鉴权的 CONNECT = 免认证任意 TCP 跳板（内网 SSRF、端口扫描）。
2. 上游 `?url=` 转发模式本就无法承载 CONNECT（无 TCP 隧道语义），CURRENT.md 确认无消费方。
3. M4 公网化后此洞为致命级。M1 直接拔除，不做"带鉴权的 CONNECT"（无需求，YAGNI）。

**实现约束（C-P2-14 裁决：唯一实现路径，无备选分支）**：手写 accept 循环 + `TcpStream::peek` 预读首行分流 + `hyper::server::conn::http1::Builder::serve_connection` 手动驱动每连接：

```
loop { listener.accept() }
  → tokio::spawn(handle_conn(stream))
      → stream.peek(首行缓冲)
          ├─ 以 "CONNECT" 开头 → 写 403 响应字节，flush，连接关闭
          └─ 其他 → hyper http1::Builder::new().serve_connection(stream, service)
                     （service = axum Router 经 `RouteService`/`TowerToHyperService` 适配）
```

**删除原 spec 中"axum::serve 优先、http1::Builder 兜底"的双分支表述**——`axum::serve` 不支持 per-connection 分流，直接采用 http1::Builder 单一路径。`Pool` / relay 模块（`parse_host_port` / `connect_with_failover`）不再被 server 引用，相关代码随手写解析一并从 main.rs 删除（Pool 本体留在 core 不删，M3 评估）。

### 2.3 全局并发连接上限（S-P1-额外 裁决，轻量）

- `tokio::sync::Semaphore`（全局 `Arc<Semaphore>`，permits = **256**，常量 `MAX_CONCURRENT_CONNECTIONS`）。
- accept 循环内 `Arc::clone(&sem).acquire_owned().await` 后才 spawn 连接处理；超限时 acquire 挂起（新连接排队等待）——**不做**主动拒绝（个人网关 256 并发已远超需求，排队即天然背压）。
- 连接处理结束时 permit 随 owned guard drop 自动释放。
- 这是**并发连接数上限**，非完整限速框架（无 QPS/令牌桶）；完整限速为后续里程碑项（README 已知债务）。

## 3. 文件位置

- 新增 `crates/server/src/gateway.rs`：accept 循环、CONNECT 拦截、数据面 Router、鉴权中间件、转发 handler、并发 Semaphore。
- 改造 `crates/server/src/main.rs`：装配（config 加载顺序、EdgeClient 构造、TokenService/RouteTable/UsageTracker 初始化、双端口 serve）。
- `crates/server/Cargo.toml`：axum 升级声明为 workspace 依赖 `axum = "0.7"`（与现有一致），新增 `futures = "0.3"`（bytes_stream 包装用）、`http-body-util`（http1::Builder serve 需要）、`hyper`（workspace 依赖，serve_connection 用）、`tokio` 已有。
- 删除 main.rs 中手写解析函数：`read_request_head` / `find_head_end` / `parse_headers` / `read_body` / `write_response` / `extract_path`，以及 CONNECT/池相关 `parse_host_port` / `connect_with_failover`（P0-1：不再有 relay 路径）。

## 4. 请求处理流程（精确）

```
TCP accept → Semaphore acquire（§2.3）→ spawn
  → peek 首行
      ├─ CONNECT → 403 {"error":"connect_forbidden"}，关闭连接
      └─ 其他 → http1::Builder serve_connection(axum Router)
            ├─ GET /                → info 端点（§4.3）
            └─ /{rest}              → auth 中间件 → 转发 handler
```

### 4.1 鉴权中间件（精确语义）

输入：`Request`。提取顺序：
1. **路径模式**：`/{token}/{route}/...` —— 首段匹配 `pony_` 前缀（`starts_with("pony_")`）时视为 token 段。剥离该段，剩余为 `{route}/{path}?{query}`。
2. **header 模式**：首段不匹配 `pony_` 前缀时，查 `X-Pony-Token` header；存在则首段即 route。
3. 两处均无 token → `401 {"error":"unauthorized"}`（content-type: application/json）。
4. `token_service.verify(plaintext)` 失败（NotFound/Revoked/Expired）→ 同样 `401 {"error":"unauthorized"}`（**三种失败不区分响应体**，防枚举探测；仅 tracing::info 记录原因，不打 token 明文）。
5. 校验通过：将 `TokenRow.id`、剥离后的 `{route}/{path}?{query}` 放入 request extensions，进入转发。
6. **header 剥离（S-P1-1 裁决，两种模式都执行）**：进入转发前从请求 header map 删除 `x-pony-token`（reqwest HeaderName 小写形式）——路径模式下该 header 不存在（删除为 no-op），header 模式下必须删除，否则明文 token 经 `EdgeClient::sanitize_headers`（其 HOP_BY_HOP 列表不含它）透传到上游第三方 API。删除动作放在 §4.2 转发 handler 第 1 步，先于 sanitize_headers。

注意：路径模式中 token 段与 route 段的歧义——route 名不允许以 `pony_` 开头（**T4 §6 显式校验**：`name.starts_with("pony_")` → InvalidName，非"正则天然排除"）。header 模式下若首段以 `pony_` 开头但无 header，按路径模式处理（verify 失败 → 401），不做二义猜测。

### 4.2 转发 handler（精确）

1. 从 extensions 取 token_id、route 名、剩余 path+query；**立即从 header map 移除 `x-pony-token`**（§4.1 第 6 步，防泄露）。
2. `route_table.resolve(route_name, path_query)` → `Result<(target_url, Upstream), RouteError>`（T4 接口；`RouteError::UnknownRoute` → `404 {"error":"unknown_route"}`；`RouteError::Disabled` → `404` 同体，不区分，防路由枚举）。
3. 读请求 body：`axum::body::to_bytes(body, 32MB)` 上限沿用 `MAX_BODY_SIZE = 32 * 1024 * 1024`，超限 `413`。
4. **用量计数时序（C-P2-5 裁决）**：`record_request(route, token_id, bytes_in)` 在 **body 读取完成后**调用（非请求开始时）——保证 bytes_in 与 requests 计数同帧、413/401 等失败路径不计 requests。`bytes_in = body.len()`（无 body 为 0）。响应阶段每收到 chunk 累加 `usage.record_bytes_out(route, token_id, n)`（接口见 T5 spec）。
5. 构造 `ForwardRequest { method, target_url, headers: EdgeClient::sanitize_headers(&req_headers), body }`，经对应 `EdgeClient.execute`。
6. 响应透传：状态码 + 过滤后响应头 + `Body::from_stream(resp.bytes_stream())`。**响应头过滤**沿用现逻辑（去 `transfer-encoding` / `connection` / `content-length`），并强制 `connection: close`。
7. 上游执行失败（EdgeClient Err）→ `502 {"error":"upstream_error"}`。
8. **错误日志纪律（S-P2-10 裁决）**：全部错误日志**禁止记录完整 URL**（query 可能含上游 API key 等敏感参数）。仅记录：route 名、目标 host（target_host，不含 path/query）、HTTP 状态码、错误类别。示例：`tracing::warn!(route = %route, host = %target_host, status = resp.status().as_u16(), "upstream error")`。

### 4.3 根路径信息端点（GET /）

```json
{
  "service": "pony-proxy",
  "version": "m1",
  "auth": "GET /{token}/{route}/{path} 或 header X-Pony-Token",
  "admin": "http://127.0.0.1:8900/api/*"
}
```

无鉴权（仅本机绑定，泄露面为服务形态，无敏感数据；不列路由名——路由列表经管理面鉴权获取）。

## 5. main.rs 装配顺序（硬性约定，T1 §6.1 第 8 点）

1. 读环境变量覆盖：`PPROXY_CONFIG` / `PPROXY_DB` / `PPROXY_LISTEN_DATA`（格式 `host:port`，覆盖 config.json 的 listen_*）/ `PPROXY_LISTEN_ADMIN`（默认 `127.0.0.1:8900`）。
2. 读完整旧 config.json（`PoolConfig` 反序列化，容忍迁移后格式——无 `routes` 键但凭据/上游项保留，P0-2）。
3. 构造 EdgeClient map：`worker` ← worker_url/worker_secret；`upstreams` 各项。**先于迁移**。
4. `Store::open(db_path)` → `migrate_config_if_needed(config_path)`（此时 EdgeClient 已持有上游信息，config.json 重写仅删 routes 键、凭据保留，双保险）。
5. 构造 `TokenService`（含 admin 引导，日志打印 ADMIN_TOKEN）、`RouteTable`、`UsageTracker`。
6. 启动 usage 落库 interval task（T5）。
7. 数据面 listener + accept 循环（CONNECT 拦截 + http1::Builder，§2.2/§2.3）；管理面 serve（T6 的 Router）。
8. **管理面绑定非回环地址时 `tracing::warn!`**（S-P2-额外 裁决）：`PPROXY_LISTEN_ADMIN` 解析出的 host 非 `127.0.0.1`/`localhost`/`[::1]` 时打 warn（提示暴露面扩大，不阻止启动——测试场景需绑 0.0.0.0 之外的可达地址时不受阻）。

## 6. 依赖任务

T2（TokenService）。T4/T5 的类型在 handler 签名中引用，实现顺序上 T3 编码需 T4/T5 接口已定义（并行波次 2 产出接口、波次 3 联调）。

## 7. 单元测试清单（gateway.rs 内，axum `tower::ServiceExt::oneshot` 构造请求，TokenService 用临时 DB）

1. 无 token 无 header → 401，body 精确 `{"error":"unauthorized"}`。
2. 路径模式：`/{valid_token}/anthropic/v1/messages` → extensions 中 route="anthropic"、path="/v1/messages"、token_id 正确。
3. header 模式：`X-Pony-Token` + `/anthropic/v1/messages` → 同上。
4. 无效 token（伪造 32hex）→ 401。
5. 撤销后 token → 401。
6. 过期 token → 401。
7. token 段 + 未知 route → 404（先鉴权后路由：token 有效才 404；token 无效仍 401）。
8. 首段 `pony_xxx` 但无 header → 按路径模式 401（§4.1 歧义规则）。
9. 根路径 GET / → 200 JSON 含 `"service":"pony-proxy"`。
10. body 超 32MB → 413。
11. **x-pony-token 剥离（S-P1-1）**：header 模式请求经 handler 后，传给 EdgeClient 的 headers 不含 `x-pony-token`（对 ForwardRequest 构造逻辑断言，或经 mock 上游回显断言）。
12. **CONNECT 拦截（P0-1）**：对 accept 层分流逻辑发 `CONNECT host:443 HTTP/1.1` 字节流 → 收到 `403` 且连接关闭、无任何隧道字节转发。
13. **并发上限（S-P1-额外）**：Semaphore permits == 256（构造断言）；并发 300 个 oneshot 请求全部最终完成（排队语义，无丢失）。
14. record_request 时序：413（body 超限）路径不产生 requests 计数（C-P2-5）。

## 8. 验收标准

- `cargo test -p pproxy-server` 全绿。
- 手动：`cargo run` 后 `curl 127.0.0.1:8899/` 返回 info JSON；无 token 请求任意路由 401。
- main.rs 中不再存在 `read_request_head` 等手写解析函数，也不再存在 `parse_host_port` / `connect_with_failover`（CONNECT 已禁用，无 relay 路径）。
- SSE 冒烟：对 anthropic messages 端点（假 key）发 stream 请求，响应为流式（curl 无缓冲逐段输出）。
- `curl -X CONNECT --proxy 127.0.0.1:8899 https://example.com -v` → 403（T8 步骤覆盖）。
- `PPROXY_LISTEN_DATA=:18999 PPROXY_DB=/tmp/t.db` 可启动，不触碰生产 :8899 与 `~/.pony/state.db`。
