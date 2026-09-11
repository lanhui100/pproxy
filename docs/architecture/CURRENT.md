# 当前系统架构（现状）

> 更新: 2026-09-02 | 状态: 生产运行中（CLI / dev 服务器 + Windows 桌面端 v0.3.5）

## 拓扑

```
[开发终端 / CLI (pproxy on)]       [手机客户端 (Wi-Fi / Clash Meta)]       [Windows 桌面端 (Tauri 2)]
         │                                    │                                    │
         └────────────────────────────────────┼────────────────────────────────────┘
                                              ▼
                    ┌──────────────────────────────────────────────────┐
                    │ pproxy-server / pproxy-engine (127.0.0.1:8899)  │
                    │ ├─ 鉴权: Basic Auth / Token Auth / Gatekeeper 限流 │
                    │ ├─ 路由: SQLite 热加载 (state.db) / 用量统计      │
                    └─────────┬──────────────────────────────┬─────────┘
                              │ (1. 正向 CONNECT 隧道)        │ (2. 反向 API 网关 /{token}/{route}/*)
                              ▼                              ▼
             ┌─────────────────────────────────┐   ┌───────────────────────────────────┐
             │ TunnelPool 待命 WS 隧道 (1 RTT)  │   │ EdgeClient 协议转发 (?url=...)    │
             └────────────────┬────────────────┘   └─────────┬─────────────────────────┘
                              │                              ├─ CF Worker (edge.example.com)
                              ▼                              │   (anthropic/google/github/x)
             ┌─────────────────────────────────┐             └─ Vercel 函数 (vedge.example.com)
             │ CF gate-worker (gate.example.com)│                 (openai/opencode, AWS IP)
             │ ├─ Token 验签 + 443 ACL 门禁    │
             │ └─ cloudflare:sockets WS↔TCP透传│
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

## 关键组件

| 组件 | 位置 | 职责 |
|------|------|------|
| pproxy-transport | crates/transport/ | 底层传输抽象（WS 隧道建立、待命池维护、Ping-Pong、双向 relay） |
| pproxy-engine | crates/engine/ | 嵌入式网关核心引擎（CONNECT 处理、Basic/Token 鉴权、Gatekeeper 门禁、Axum 数据面） |
| pproxy-core | crates/core/ | SQLite 存储层（tokens/users/routes/usage）、EdgeClient 上游协议 |
| pproxy-server | crates/server/ | 独立守护进程（网关分发 + 管理面 REST API） |
| pproxy-cli | crates/cli/ | CLI 客户端（`pproxy on/off/status/env`、token/route 管理） |
| pony-desktop | desktop/ | Tauri 2 桌面端（系统托盘、代理开关、白名单配置、本地引擎） |
| gate Worker | deploy/cf-gate-worker/ | 出海正向隧道端点（Token 哈希验签 + ACL + WS↔TCP 密文透传） |
| edge Worker | deploy/cf-worker/ | 反向网关公网出口 1（CF 边缘，去 IP 标识） |
| Vercel 函数 | deploy/vercel/ | 反向网关公网出口 2（AWS 出口 IP，规避 CF 敏感服务） |

## 配置

- `config.json`：监听地址、路由表、上游（worker_url + upstreams）、密钥
- `systemd/pproxy.service`：User=pproxy，Restart=always，RUST_LOG=info
- 凭据：`.secrets.env`（600），运行时不依赖

## 已知限制与运行边界

- **TCP Only 出口**：基于 WebSocket/Cloudflare Sockets 隧道，仅支持 TCP（HTTPS/HTTP）流量，不支持原生 UDP（如 UDP 游戏）。
- **Allowlist 策略**：正向代理模式受 Allowlist 控制（默认覆盖 OpenAI/Claude/Google/GitHub 等），需配置通配符 `*` 方可作为全网梯子使用。
- **Vercel 函数限制**：Hobby 计划单请求上限 300s（超长 LLM 响应会被掐断）。

