# 当前系统架构（现状）

> 更新: 2026-08-22 | 状态: 生产运行中（dev 服务器）

## 拓扑

```
[本机 SDK]
   │ http://127.0.0.1:8899/{route}/...
[手机 4G SDK]（M4 公网入口）
   │ https://access.ponyjob.top/{token}/{route}/...  ← CF 边缘 TLS 终结
   ▼ CF Tunnel（cloudflared, systemd: pony-tunnel.service，出站 http2，零入站端口）
   ▼
pproxy-server (Rust, systemd: pproxy.service)
   ├─ 数据面 :8899（网关分发）
   ├─ 管理面 :8900（/stats /refresh）
   │
   ├─[Worker 上游]──> https://edge.ponyjob.top/?url=<target>
   │                    └─ CF Worker: 剥离 geo/hop-by-hop 头 → fetch 目标 → 流式回传
   │                        覆盖: anthropic / google / github / x / facebook
   │
   └─[Vercel 上游]──> https://vedge.ponyjob.top/api/proxy?url=<target>
                        └─ Vercel Function (Node, maxDuration 300s):
                            出口 AWS us-east 真实 IP
                            覆盖: openai / opencode（对 CF 数据中心 IP 敏感的服务）
```

## 数据面协议

1. 客户端请求 `http://127.0.0.1:8899/{route}/{path}?{query}`
2. server 查路由表 → 目标 `https://{target_host}/{path}?{query}`
3. 经上游 `?url=` 转发：透传 method/body/业务 header，剥离 hop-by-hop 与 geo 头
4. 响应流式透传（SSE 兼容，`resp.chunk()` 循环）
5. CONNECT 请求：池空时直连目标（兜底，当前池已停用）

## 关键组件

| 组件 | 位置 | 职责 |
|------|------|------|
| EdgeClient | crates/core/src/edge.rs | 上游转发协议（URL 拼装、secret 注入、header 过滤） |
| 网关分发 | crates/server/src/main.rs | CONNECT/HTTP 分流、路由解析、流式回写 |
| Pool | crates/core/src/pool.rs | 免费代理池（已停用：countries=[] 时跳过） |
| CF Worker | deploy/cf-worker/worker.js | 公网出口 1（geo 头剥离防泄露真实 IP） |
| Vercel 函数 | deploy/vercel/api/proxy.js | 公网出口 2（AWS IP，300s） |

## 配置

- `config.json`：监听地址、路由表、上游（worker_url + upstreams）、密钥
- `systemd/pproxy.service`：User=dm，Restart=always，RUST_LOG=info
- 凭据：`.secrets.env`（600），运行时不依赖

## 已知限制

- 上游为 `?url=` 明文转发模式，无法承载加密 CONNECT 隧道 → 不支持系统级全局代理
- Vercel Hobby 函数上限 300s（超长 LLM 响应会被掐断）
- CF Worker 出口被 OpenAI/zen 地区策略拦截（已由 Vercel 上游规避）
- 国内 DNS 过滤含 "proxy" 的子域名 → 子域命名避开该词（edge/vedge/access）
