# pproxy-transport

`pproxy` 的传输层：WS 隧道协议（客户端侧升级 + 首帧握手）、待命连接池、双向字节中继、
按目标 host 的出口路由判据，以及 1 RTT 探活。消费者是 `crates/server` 与 `crates/engine`
（`crates/gate-server` 是服务端桥，不依赖本 crate）——**分流判据只在这里实现一次**。

## 配置项

本 crate 无环境变量，行为由常量与构造参数决定：

| 常量 / 参数 | 值 | 语义 |
|---|---|---|
| `proto::DIAL_TIMEOUT` | 8 s（`cfg(test)` 800 ms） | WS 拨号上限；跨国链路 + TLS 握手需足够窗口 |
| `proto::FIRST_FRAME_TIMEOUT` | 8 s（`cfg(test)` 600 ms） | 升级后等首帧的上限 |
| `pool` 缺省节奏 | idle TTL 25 s / refill 2 s / backoff 5 s | 待命池的保活、补池、失败退避 |
| `pool::TunnelPool::with_size` / `with_timing` | 构造参数 | 池容量与节奏可按宿主覆盖 |
| `route::COMPLIANT_EGRESS_SUFFIXES` | 3 项（Cloud Code 系 `googleapis.com`） | 必须走合规机房出口的 host |
| `route::YOUTUBE_SUFFIXES` / `AI_TARGET_SUFFIXES` / `GOOGLE_BASE_SUFFIXES` | 见 `route.rs` | 分流与出口调序的判据表 |

## 语义

- **协议**（`proto.rs`）：`connect_ws` 只做"建立热态 WS 管道"（不声明目标），供池预建复用；
  升级时 `Authorization: Bearer <token>`，token 先 trim + 非空校验（粘贴带入的尾换行不该变成难查的
  HeaderValue 错误）。两个稳定错误标记供上层自愈：`AUTH_401_MARKER`、`QUOTA_402_MARKER`。
- **待命池**（`pool.rs`）：`TunnelPool` 维持若干预建 idle 会话，`checkout` 取用、`invalidate` 作废、
  `pool_stats` / `idle_total` 观测；按 `token_fp8_of`（令牌指纹前 8 位）区分不同租约的池，便于排障而不泄明文。
- **中继**（`relay.rs`）：`relay_bidir_ws` / `relay_bidir_ws_bounded` 双向搬运 WS 帧与 TCP 字节，
  经 `TrafficCounter` 计数（计量归属由宿主传入）。
- **路由判据**（`route.rs`）：`classify_egress` / `is_youtube_host` / `requires_compliant_egress` /
  `is_google_host` / `is_strict_ai_host` / `ordered_gate_urls` / `order_endpoints` /
  `compliant_egress_endpoints` / `is_vercel_endpoint` / `is_native_vps_endpoint`。出口顺序按目标 host 决定
  （例如 Cloud Code 系必须优先合规物理出口，否则 Google 按来源 IP 判地区返回 `400 FAILED_PRECONDITION`）。
- **探活**（`probe.rs`）：`probe_gate_rtt` 在热态 WS 上 Ping，剥离冷建连开销，得到真实 1 RTT 延迟
  （仪表盘"接口状态"用）。

## 限制

- **只有 WS 传输**：不实现 HTTP/1.1 代理语义，也不做 TLS 终止（端到端由 CONNECT 隧道承载）。
- **客户端侧协议**：服务端 WS（`/ws` 升级与帧分发）在 `crates/server`/`crates/gate-server` 的 axum 层，
  不在本 crate。
- **池是尽力而为**：idle 超过 TTL 未取用即废弃，`checkout` 失败要由调用方走"冷建连"分支，不保证命中。
- **超时是编译期常量**：靠 `cfg(test)` 分支缩短测试预算，运行时不可调（要调就是改契约）。
- **回退语义在调用方**：本 crate 只报错，不做"换出口重试"——重试/回落策略在 engine/server 的 CONNECT 路径。

## 扩展点

- 新传输形态：`proto.rs` + `lib.rs` 导出；WS 之外（如纯 TCP 中继）应复用 `relay.rs` 的双向搬运骨架。
- **新路由判据**：`route.rs` 的判据表；改 `COMPLIANT_EGRESS_SUFFIXES` 必须**同一提交**同步
  `deploy/cf-gate-worker/gate-policy.mjs`，并跑 `bash scripts/check-egress-parity.sh`（否则 CI 红）。
- 新池策略：`pool.rs` 的 `with_timing` 参数化能力，不要把节奏硬编码进宿主。
- 新探活口径：`probe.rs`，保持"只测热态 1 RTT、不含冷建连"的语义，否则延迟数字不可比。
