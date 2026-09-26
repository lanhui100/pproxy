# 当前系统架构（现状）

> 更新: 2026-09-25 | 状态: 生产运行中（CLI / dev 服务器 + Windows 桌面端 v0.3.5）

## 拓扑

```
[开发终端 / CLI (pproxy on)]       [手机客户端 (Wi-Fi / Clash Meta)]       [Windows 桌面端 (Tauri 2)]
         │                                    │                                    │
         └────────────────────────────────────┼────────────────────────────────────┘
                                              ▼
                    ┌──────────────────────────────────────────────────┐
                    │ Local HA Forwarder (127.0.0.1:8899 本地高可用入口)  │
                    │ ├─ Zero-Wait Prober 零等待探活 (300ms 探测/80ms 超时) │
                    │ ├─ 故障无感漂移 (本地优先 / 熔断秒级切远程备灾候选)   │
                    │ └─ X-Pony-Cluster-Ticket 权威票证注入与清洗        │
                    └─────────┬──────────────────────────────┬─────────┘
                              │ (本地优先: 0ms 路由)            │ (故障漂移: 注入集群票证)
                              ▼                              ▼
     ┌───────────────────────────────────┐        ┌───────────────────────────────────┐
     │ dev 节点 (本地 pproxy-engine)      │        │ preprod / tencent 对等节点         │
     │ ├─ 回环免认证 (杜绝 XFF 穿透)        │◄──────►│ ├─ X-Pony-Cluster-Ticket 验签     │
     │ ├─ Basic / Token 认证 + Gatekeeper│集群对等│ ├─ pproxy-engine 数据面           │
     │ ├─ SQLite 路由与用量统计           │互联通道│ └─ 独立出海隧道与备灾中继          │
     └─────────┬─────────────────────────┘        └───────────────────────────────────┘
               │
               ├──────────────────────────────────────────────┐
               │ (1. 正向 CONNECT 隧道)                        │ (2. 反向 API 网关 /{token}/{route}/*)
               ▼                                              ▼
┌─────────────────────────────────┐            ┌───────────────────────────────────┐
│ TunnelPool 待命 WS 隧道 (1 RTT)  │            │ EdgeClient 协议转发 (?url=...)    │
└────────────────┬────────────────┘            └─────────┬─────────────────────────┘
                 │ 携带 Ed25519 User Token               ├─ CF Worker (edge.example.com)
                 ▼                                       │   (anthropic/google/github/x)
┌─────────────────────────────────┐                      └─ Vercel 函数 (vedge.example.com)
│ gate-server / gate-worker       │                          (openai/opencode, AWS IP)
│ ├─ Ed25519 Token 非对称公钥验签  │
│ ├─ 握手配额拦截 (HTTP 402)       │
│ ├─ 双向流式实时熔断 (WS 4402)    │
│ └─ cloudflare:sockets / WS↔TCP  │
└────────────────┬────────────────┘
                 ▼
      [海外目标站点 (TCP:443)]
```

## 数据面协议

### 1. 正向出海代理 (Forward CONNECT Proxy)
1. 客户端发起 `CONNECT host:443 HTTP/1.1` 请求并携带 `Proxy-Authorization: Basic <base64>` 或 `X-Pony-Token`。
2. `pproxy-engine` 校验凭据与 Gatekeeper 防爆破门禁；未通过返回 `407 / 429`。
3. 校验目标 host 命中 Allowlist（默认 AI/开发站点，支持自定义扩展）。
4. 从 `TunnelPool` 连接池中取出预建的 WebSocket 会话（或新建连），发送首帧 JSON 声明目标。
5. 返回 `HTTP/1.1 200 Connection Established`，进入高吞吐双向透传（支持 Ping/Pong 保活与半关闭）。

### 2. 反向 API 网关 (Reverse API Gateway)
1. 客户端请求 `http://127.0.0.1:8899/{token}/{route}/{path}?{query}`。
2. server 查 SQLite 路由表 $\to$ 目标 `https://{target_host}/{path}?{query}`。
3. 经上游 `?url=` 转发：透传 method/body/业务 header，剥离 hop-by-hop 与 geo 头。
4. 响应流式透传（SSE 兼容，`resp.chunk()` 循环）。

### 3. Local HA Forwarder 零等待探活与故障无感漂移 (Zero-Wait Prober)
1. **后台主动探活**：`LocalHaForwarder` 运行独立的后台异步探活循环，每 300ms 对本地主引擎执行 80ms 超时的 TCP 探测，维护原子标志位 `local_healthy`。
2. **零等待关键路径路由**：
   - 当 `local_healthy` 为 true 时，请求以 0 附加开销直接连接本地主引擎；
   - 连续 2 次探测失败或建连超时触发熔断，`local_healthy` 置为 false；后续请求直接跳过本地连接尝试（0ms 串行等待），立即转向远程对等节点候选列表（dev / preprod / tencent），实现客户端故障无感漂移；本地主引擎恢复后自动复位。
3. **会话生命周期保护**：双向流量中继建立后通过零拷贝流转发 (`copy_bidirectional`) 透传，受 300 秒会话超时守护防止连接与 FD 泄漏。

### 4. 分布式集群对等互联与机器互信中继 (X-Pony-Cluster-Ticket)
1. **机器互信凭据**：集群对等节点（dev / preprod / tencent）之间通过 `X-Pony-Cluster-Ticket` 头部建立机器互信中继，票证格式为 `node_id|timestamp|hmac_sha256`，基于全集群共享的 `cluster_auth_key` 生成。
2. **严格有效期与恒定时间比对**：节点接收票证时比对时间戳，限制最大有效期为 120 秒（`TICKET_MAX_AGE_SECS`），超时或时钟偏差直接拒绝；摘要比对采用恒定时间比对算法（Constant-Time Comparison），杜绝时序侧信道嗅探。
3. **伪造清洗与中继注入**：`LocalHaForwarder` 在转发前于字节切片层严格清洗客户端原始请求中伪造或携带的 `X-Pony-Cluster-Ticket` 头部；仅在将流量中继至远程对等节点时，由 Forwarder 权威注入新签发的集群票证。
4. **上下文身份隔离**：对等节点验签通过后，将请求上下文标记为受信任的 `AuthSubject::ClusterNode` 身份并直接放行，与终端用户鉴权流程物理隔离。

### 5. 多租户 Ed25519 自包含 User Token 验签与流式双向熔断协同 (402 / 4402)
1. **自包含非对称凭据**：多租户令牌基于 Ed25519 签名，载荷自包含 `jti`（唯一标识）、`sub`（租户 UID）、`quota_bytes`（总周期配额）、`exp`/`iat`（生命周期）及 `max_conns`（并发连接数）。私钥保留在管理签发端，所有出海节点仅持公钥即可完成独立毫秒级验签。
2. **握手阶段配额门禁 (HTTP 402)**：客户端发起连接或 WebSocket 握手时，出海网关验签 Claims 并核对内存计数器 `user_used_bytes`。若当前已用字节达到或超出 `quota_bytes`，直接返回 `HTTP 402 Payment Required` 拒绝建连；并发连接数达到 `max_conns` 时返回 `HTTP 429 Too Many Requests`，通过原子槽位预占防止并发突发穿越。
3. **流式传输阶段双向实时熔断 (WS 4402)**：在已建立的出海隧道内，网关实时并发统计上行 (`ws_to_tcp`) 与下行 (`tcp_to_ws`) 传输字节，并原子累加至租户用量。一旦任一传输方向导致累计用量达到或突破配额限制，网关立即触发取消信号中断底层 TCP 连接，并向客户端推送携带 Code `4402`（Reason: `"Quota Exceeded"`）的标准 WebSocket CloseFrame 帧，实现连接双向秒级硬切断。

### 6. 本机回环免认证与安全硬化 (Anti-XFF 穿透)
1. **纯净回环放行判定**：当且仅当客户端物理连接来源为回环网络（IPv4 `127.0.0.1` 或 IPv6 `::1`），且请求头中**完全不存在**任何代理转发标记（严格匹配 `X-Forwarded-For`、`X-Real-IP`、`Forwarded`）时，系统视为本机受信进程发起，直接免认证放行。
2. **反代伪造强拦截**：一旦请求包含任一转发标记头，即使物理连接来自回环网络，系统判定可能存在未经授权的反向代理透传或穿透企图，立即吊销免认证特权，强制回退至标准 Basic Auth / Token 认证逻辑，未通过则返回 `407 Proxy Authentication Required` 或 `401 Unauthorized`。

## 关键组件

| 组件 | 位置 | 职责 |
|------|------|------|
| LocalHaForwarder | crates/core/src/ha_forwarder.rs | 本地高可用分发入口（Zero-Wait Prober 探活、故障无感漂移、票证清洗与注入） |
| cluster_ticket | crates/core/src/cluster_ticket.rs | 集群机器互信票证（HMAC-SHA256 票证生成、120s 窗口校验、恒定时间防篡改） |
| auth (Ed25519) | crates/core/src/auth.rs | 多租户自包含令牌体系（TokenSigner 签名、TokenVerifier 验签、Claims 配额与时间校验） |
| pproxy-gate-server | crates/gate-server/ | 原生出海网关服务（Ed25519 验签、HTTP 402/429 门禁、WS 4402 双向流式配额熔断、WS↔TCP 桥接） |
| pproxy-transport | crates/transport/ | 底层传输抽象（WS 隧道建立、待命池维护、Ping-Pong、双向 relay） |
| pproxy-engine | crates/engine/ | 嵌入式网关核心引擎（CONNECT 处理、Basic/Token 鉴权、Gatekeeper 门禁、回环免认证硬化、Axum 数据面） |
| pproxy-core | crates/core/ | SQLite 存储层（tokens/users/routes/usage）、EdgeClient 协议、集群模型 |
| pproxy-server | crates/server/ | 独立守护进程（网关分发 + 管理面 REST API） |
| pproxy-cli | crates/cli/ | CLI 客户端（`pproxy on/off/status/env`、cluster 对等互联、token/user 治理） |
| pony-desktop | desktop/ | Tauri 2 桌面端（系统托盘、代理开关、白名单配置、本地引擎） |
| gate Worker | deploy/cf-gate-worker/ | 出海正向隧道端点（Token 哈希验签 + ACL + WS↔TCP 密文透传） |
| edge Worker | deploy/cf-worker/ | 反向网关公网出口 1（CF 边缘，去 IP 标识） |
| Vercel 函数 | deploy/vercel/ | 反向网关公网出口 2（AWS 出口 IP，规避 CF 敏感服务） |

## 配置

- `config.json`：监听地址、路由表、上游（worker_url + upstreams）、密钥
- `~/.pony/cluster.json`：集群机器密钥（`cluster_auth_key`）、种子节点与节点标识
- `systemd/pproxy.service`：User=pproxy，Restart=always，RUST_LOG=info
- 凭据：`.secrets.env`（600），运行时不依赖

## 已知限制与运行边界

- **TCP Only 出口**：基于 WebSocket/Cloudflare Sockets 隧道，仅支持 TCP（HTTPS/HTTP）流量，不支持原生 UDP（如 UDP 游戏）。
- **Allowlist 策略**：正向代理模式受 Allowlist 控制（默认覆盖 OpenAI/Claude/Google/GitHub 等），需配置通配符 `*` 方可作为全网梯子使用。
- **Vercel 函数限制**：Hobby 计划单请求上限 300s（超长 LLM 响应会被掐断）。
