# pproxy-core

`pproxy` 的共享领域层：存储、令牌与口令认证、配额采集、路由表、集群成员与集群票证、本地高可用
转发器、底层 relay。**不做监听**——监听与协议面在 `crates/engine`（嵌入式）与 `crates/server`（守护进程）。

## 配置项

本 crate 基本不读环境变量，配置以**构造参数**注入（这也是它可被两个宿主共用的原因）。仅两个例外：

- `PPROXY_DB`：SQLite 库路径；未设则落 `$HOME/.pony/state.db`（`store::default_db_path`）。
- `PPROXY_ADMIN_TOKEN`：注入式 admin token（`token.rs`），存在时跳过"首启生成并打印一次"的路径。

内建策略常量（改这些 = 改契约，须同提交改本条与相关测试）：

| 常量 | 值 | 语义 |
|---|---|---|
| `gatekeeper::GatekeeperConfig` 默认 | 5 次失败 / 300 s 起 | 连续失败锁定 + 指数退避上限 |
| `cluster_ticket::TICKET_MAX_AGE_SECS` | 120 | 集群票证有效期 |
| `CLOCK_SKEW_TOLERANCE_SECS` | 60 | 票证验签的时钟容差 |
| `LAST_USED_THROTTLE_SECS` | 60 | token `last_used` 落库节流 |
| `user::HASH_ITERATIONS` | 10 000 | 用户口令 `salt$iterated_sha256` 迭代次数 |
| `quota::CF_DAILY_REQUEST_QUOTA` | 100 000 | CF Workers 日请求配额默认值 |

## 语义

- **令牌**（`token.rs`）：数据面 `pony_` + 16 字节随机 → 32 hex；管理面 `pony_admin_` + 24 字节 → 48 hex。
  库里只存 `sha256_hex(明文)`，明文仅在创建时返回一次；`verify_admin` / `verify` 走常量时间比较。
- **用户口令**（`user.rs`）：`salt$iterated_sha256(10k)`，用于 Basic Auth 的 username/password 面。
- **存储**（`store/`）：单文件 SQLite，六张表 `tokens` / `users` / `routes` / `usage_hourly` /
  `quota_snapshots` / `alerts`；迁移为幂等 `CREATE TABLE IF NOT EXISTS` + 缺列补齐（`store/migrate.rs`）。
- **路由与出口**（`route.rs`、`edge.rs`）：`RouteTable` 解析 `/{token}/{route}/*path` 到上游；`EdgeClient`
  封装对 CF Worker / Vercel 的转发请求。
- **计量与配额**（`usage.rs`、`quota.rs`、`alert.rs`、`store/monitor.rs`）：请求/字节计数 + 每小时落库、
  CF GraphQL 与 Vercel usage 采集、80% / 95% 越线告警与已读幂等。
- **集群**（`cluster.rs`、`cluster_ticket.rs`）：加入令牌（签名 + 过期）、心跳与 tombstone 成员视图、
  节点间票证（`X-Pony-Cluster-Ticket`，HMAC）。`ha_forwarder.rs` 的 `LocalHaForwarder` 把客户端数据面
  令牌与集群票证**分开**：令牌管外部客户端，票证只管节点之间；回环免令牌必须显式开启。
- **relay**（`relay.rs`）：`http_connect_ext` / `http_connect` / `socks5_connect` / `relay`——CONNECT 与
  SOCKS5 两种上游形态的实现，供 engine/server/server 三处复用。
- 对外 API 面 = `lib.rs` 的 re-export（`Store`/`TokenService`/`UserService`/`UsageTracker`/`RouteTable`/
  `AuthGatekeeper`/`TokenSigner`/`TokenVerifier`/`ClusterManager`/`LocalHaForwarder` 等）。

## 限制

- **单机单写者**：SQLite 文件 + 进程内计数；同一库被多进程同时写不在支持范围。
- **配额是节点本地口径**：跨节点的用量不聚合（集群不是共享账本），多出口部署时各节点各记一份。
- **集群为最终一致**：心跳/墓碑式成员视图，无选主、无强一致仲裁；`ClusterJoinToken` 过期即失效，不续期。
- **不含网络监听与协议解析**：CONNECT/HTTP 报文解析在 engine/server，本 crate 只提供原始 TCP/relay 原语。

## 扩展点

- 新表：`store/mod.rs` 建表语句 + `store/migrate.rs` 幂等补列 + 对应子模块（照 `tokens.rs` 的形状）。
- 新令牌类型：`auth.rs` 的 `TokenSigner`/`TokenVerifier`（Ed25519 `usr_live_` 一类）+ 前缀常量 + 负样本测试。
- 新配额源：`quota.rs` 加 collector（解析函数与 `CfUsageOutcome` 同款"上游失败也要有明确态"，不许静默降级）。
- 新集群能力：`cluster.rs` 的成员视图与 `cluster_ticket.rs` 的票证面上扩展，别在宿主里重写。
