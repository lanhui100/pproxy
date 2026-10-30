# pproxy-gate-server

独立部署的轻量代理服务端与 Edge 转发节点组件。

## 核心职责

- 作为服务端或边缘反代运行。
- 接收客户端 CONNECT/WS 隧道流量并建立远端出海连接。
- 鉴权验证与出口 IP 路由中继。

## 配置项（环境变量）

| 变量 | 作用 | 缺省 |
|---|---|---|
| `PORT` / `HOST` | axum 路由面（`/clash`、`/ws`、`/proxy`、管理端点）监听地址 | `3101` / `0.0.0.0` |
| `TUNNEL_TOKEN_HASH` | 单口令模式的隧道口令哈希（`sha256(token)`） | 空 = 该模式全部拒绝 |
| `PROXY_SECRET` | `/proxy` 反代上游共享密钥（缺失即 fail-closed） | 空 = 拒绝 |
| `GATE_ADMIN_TOKEN` | 管理端点口令 | 空 = 管理端点不可用 |
| `USER_VERIFYING_KEY` | 多租户 Ed25519 验签公钥（hex）；存在则启用 `usr_live_` 令牌校验 | 未设 = 仅单口令模式 |
| `GATE_CONNECT_LISTEN` | 公网 CONNECT 数据面监听地址（如 `0.0.0.0:8443`） | **未设/空 = 不监听**（fail-closed，只打一行 info） |
| `GATE_CONNECT_MAX_LIFETIME_SECS` | 单连接绝对生命周期上限 | `86400`（缺失/非法回落） |

## 公网 CONNECT 数据面

正向出海的公网入口是**裸 TCP 上的标准 HTTP/1.1 CONNECT**（模块 `src/connect_proxy.rs`），
由 `main.rs` 与 `run_server` 两条启动路径共用，独立于 axum 路由面监听。

**为什么单独一条裸 TCP 通路**：`/ws` 是 WS↔TCP 桥，客户端要实现帧协议；而 Clash Meta 的
`type: http` + `tls: true` 节点只会说标准 `CONNECT host:443`。CONNECT 200 之后流动的是对端
TLS 密文而非 HTTP 报文，因此该面不走 hyper，直接在 `TcpStream` 上完成
「首部解析 → 鉴权 → ACL → 建连 → `200` → 裸字节中继」。

**令牌语义（与 `/ws` 严格同源，不另造第二套）**：

- `usr_live_` 前缀 → `pproxy_core::TokenVerifier` Ed25519 验签，并查 `revoked_tokens` 撤销表
  （按 `jti` 与 `sub` 双键）；
- 其它 → `sha256(token) == TUNNEL_TOKEN_HASH`（回退单口令模式，常量时间比较）；
- 凭据只从 `Proxy-Authorization: Basic` 取（订阅下发的 Clash 节点把令牌同时用作
  username/password）。`Authorization: Bearer` **不被接受**——该头属于 `/proxy` 与管理面契约。

**ACL 与 SSRF 防线（先解析再判定，不看字符串前缀）**：

- 目标端口**仅允许 443**，其余一律 `403`；
- authority 先判是否 IP 字面量（含十进制/十六进制/八进制 IPv4、IPv4-mapped IPv6、`::1` 完整写法、
  `::` 未指定地址），是则对解析结果做公网判定；
- 非字面量则 `lookup_host` 解析出全部地址，**任一**非公网即拒；
- 最终 `TcpStream::connect` 只用已校验的 `IpAddr`，不对域名二次解析——封掉 DNS rebinding 时间窗。

**响应矩阵（机器可判，配合 `X-Pony-Reason`）**：

| 情形 | 状态码 | 头 |
|---|---|---|
| 隧道建立成功 | `200` | `Proxy-Agent: pproxy-gate` |
| 首部/目标非法、非 CONNECT | `400` | `X-Pony-Reason: bad_request` |
| ACL 拒绝（非 443 / 非公网目标） | `403` | `X-Pony-Reason: acl_denied` |
| 超出租户配额（存量或传输中） | `403` | `X-Pony-Reason: quota_exceeded` |
| 凭据缺失/错误 | `407` | `Proxy-Authenticate: Basic realm="pproxy"` |
| 目标建连失败/超时 | `502` | `X-Pony-Reason: connect_failed` |

**时间预算（硬编码常量，改需同改契约）**：首部上限 16 KiB；首字节等待 30 s；首部读取 30 s
（自首字节起算）；空闲 300 s（任一方向无字节即断）；目标建连（含解析）15 s；
单连接绝对生命周期 86400 s（`GATE_CONNECT_MAX_LIFETIME_SECS` 可覆盖）。

**计量**：多租户模式下按 `claims.sub` 双向计量字节数并落配额；管理员 `claims` 不受配额熔断
（与 `lib.rs::quota_limit` 同口径）。传输中越过配额会立即断开中继。

**部署前置**：公网入口由 nginx `stream` 模块在 `:8443` 上做四层转发到 `GATE_CONNECT_LISTEN`，
并放行防火墙（`ufw allow 8443`）。步骤见 `docs/ops/DEPLOY.md` §0.1，客户端侧下发形状见
`docs/ops/API.md`「移动端订阅与 CONNECT 数据面」。

## 限制（明确不支持）

- **只支持 CONNECT（HTTPS/443）**，不提供明文 HTTP 正向代理（absolute-form）——收到此类请求按
  协议边界直接拒绝，而不是尝试解析。
- CONNECT 面不提供 `Authorization: Bearer` 鉴权（见上）；需要 Bearer 的场景走 `/proxy` 反代。
- `GATE_CONNECT_LISTEN` 未配置时该面**静默不启用**（fail-closed 而非回退到明文无鉴权），
  部署后必须显式设置并验证 `407`（无凭据）与 `200`（有凭据）。

## 相关文档

- `docs/ops/API.md` —— 数据面/管理面协议与错误码
- `docs/ops/DEPLOY.md` §0.1 —— nginx stream / 防火墙 / 监听配置
