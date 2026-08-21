# M1 任务级 Specs — 总纲

> 上游: [M1-backend-hardening.md](../M1-backend-hardening.md) | 状态: 待实现 | 日期: 2026-08-21
>
> 本目录 8 份任务 spec（T1-T8）精确到开发智能体无需再做设计决策。实现代码前必读本 README 的共享约定。

## 1. 任务依赖图

```
T1 (git 基线 + 存储层 store.rs)
├── T2 (token.rs)  ──────────┬── T7 (admin 引导, 并入 T2 实现、独立验收)
│   └── T3 (数据面鉴权) ──────┤
├── T4 (route.rs) ───────────┼── T6 (管理 API api.rs)
├── T5 (usage.rs) ───────────┘
│        T3 + T4 + T6 ────── T8 (集成回归验证)
```

- **T1 串行先行**（其余全部依赖 Store 类型与 workspace 依赖）。
- **T2 / T4 / T5 三路并行**（互不依赖，只依赖 T1 的 store.rs）。
- **T3 依赖 T2**；**T6 依赖 T2+T4+T5**（组装 Router 需要 TokenStore / RouteTable / UsageTracker 的公开接口）。
- **T7 无独立代码**，验收点并入 T2/T6/T8（见 T2 spec 第 8 节）。
- **T8 最后串行**（依赖 T3+T4+T6 全部合入）。

## 2. 并行策略

| 波次 | 任务 | 说明 |
|------|------|------|
| 波次 1 | T1 | 含 git init + `pre-m1` 基线提交，必须最先完成 |
| 波次 2 | T2 / T4 / T5 | 并行，各自只新增 `crates/core/src/{token,route,usage}.rs` + 对应 Cargo 依赖；**禁止改动他人文件**；对 lib.rs 的 `pub mod` 追加行合并时无冲突（各自独立一行） |
| 波次 3 | T3（main.rs 数据面）+ T6（api.rs 管理面） | T3 与 T6 都改 main.rs，**串行执行**：先 T3 后 T6 |
| 波次 4 | T8 | 集成测试脚本 + 全量回归 |

冲突规避：T2/T4/T5 只允许新增文件与追加 `pub mod` 行；T3/T6 串行改 main.rs。

## 3. 共享约定（全部任务强制遵守）

### 3.1 模块归属

| 模块 | 位置 | 职责 | 禁止 |
|------|------|------|------|
| store | `crates/core/src/store.rs` | SQLite 连接、DDL、迁移、CRUD | 不含业务校验（如 token 过期判断） |
| token | `crates/core/src/token.rs` | 生成/哈希/校验/CRUD/缓存 | 不碰 HTTP |
| route | `crates/core/src/route.rs` | 路由表、上游选择、SSRF 校验 | 不碰 HTTP server |
| usage | `crates/core/src/usage.rs` | 计数、聚合、落库 | 不碰 HTTP |
| api | `crates/server/src/api.rs` | 管理 REST API（axum） | 不直接写 SQL（经 Store） |
| 数据面 | `crates/server/src/gateway.rs`（新）+ `main.rs` | 路径解析、鉴权、转发 | — |

依赖方向单向：`server → core`；core 内部 `token/route/usage → store`，互相之间不依赖（usage 的 token_id/route 名只是 u64/String 值，不引用 token/route 模块类型）。

### 3.2 错误处理（裁决 #8）

- **core 模块**：各模块定义具体错误枚举，`impl std::error::Error + Display`，方法返回 `Result<T, XxxError>`。理由：调用方（api.rs 需映射 HTTP 状态码）需要区分错误类别，anyhow 无法 match。
- **server 层**：handler 内部用 anyhow 串联，在 handler 边界统一映射为 `StatusCode` + JSON 错误体。
- 统一枚举模式（各模块照抄结构）：

```rust
#[derive(Debug)]
pub enum TokenError {
    NotFound,            // 查无此 token
    Revoked,             // 已撤销
    Expired,             // 已过期
    Duplicate,           // name 或 hash 冲突
    Store(StoreError),   // 底层存储错误
}
```

- `StoreError` 定义于 store.rs，全部模块复用：`pub enum StoreError { Sqlite(rusqlite::Error), Io(std::io::Error), Json(serde_json::Error), Migration(String) }`。
- 所有错误实现 `Display`；`From<rusqlite::Error> for StoreError` 必须实现。

### 3.3 时间处理（裁决 #7）

- 唯一时间源：`std::time::SystemTime`，统一封装为 core 内自由函数（放 `store.rs`，全模块共用）：

```rust
pub fn now_unix() -> u64;                 // UTC 秒
pub fn now_unix_ms() -> u64;              // UTC 毫秒（仅 usage_hourly.ts_hour 用秒，此函数备用）
pub fn hour_floor(ts: u64) -> u64;        // ts 向下取整到小时（ts - ts % 3600）
```

- 数据库所有时间列存 **UTC Unix 秒（INTEGER）**。禁止 `chrono`（YAGNI，展示层格式化由 CLI/GUI 做）。
- 禁止 `Instant::now()` 参与任何落库数据（仅允许用于进程内耗时测量，如 /api/routes/{name}/test 的耗时）。

### 3.4 并发与阻塞（C-P1-9 裁决）

- SQLite：`Mutex<Connection>`（std Mutex，非 tokio Mutex——std Mutex 足够且避免跨 await 持锁）。**裁决**：单用户网关，不引入连接池/r2d2。
- **Store 全部公开方法为同步 `pub fn`（同步签名，唯一口径）**；`spawn_blocking` 由**调用方**负责：
  - T2：touch 路径（节流写库）经 `spawn_blocking`；
  - T5：flush 的 `upsert_usage` / query 经 `spawn_blocking`；
  - T6：管理端点 handler 内对 Store/TokenService/RouteTable 同步方法经 `spawn_blocking` 包裹。
  - 数据面热路径（verify/resolve）只读内存缓存，不触 Store。
- DashMap 用于 usage 计数；token 缓存用 `std::sync::RwLock<HashMap>`（读多写少）。
- **数据面全局并发连接上限（S-P1-额外 裁决）**：`tokio::sync::Semaphore`，常量 `MAX_CONCURRENT_CONNECTIONS = 256`，accept 后 acquire、连接结束自动释放；超限连接排队等待（天然背压）。此为并发连接数上限，非完整限速（见"已知债务"）。

### 3.5 日志

- tracing，级别：token 明文/哈希**永不**打日志；admin token 仅首启打印一次（T7；可用 `PPROXY_ADMIN_TOKEN` 环境变量注入已知 admin token 跳过生成与打印，S-P1-3）；其余按现有风格 info/warn。
- **错误日志禁记完整 URL**（S-P2-10：query 可能含上游 API key）——仅 route 名 + 目标 host + 状态码。

### 3.6 测试约定

- 单元测试与实现同文件 `#[cfg(test)] mod tests`。
- 集成测试用 `PPROXY_DB` / `PPROXY_CONFIG` / `PPROXY_LISTEN_DATA` / `PPROXY_LISTEN_ADMIN` 环境变量覆盖默认路径与端口（main.rs 必须支持，T3 spec 落实）。
- 默认值：DB=`~/.pony/state.db`，config=`/home/USER/pproxy/config.json`，数据面沿用 config.json 的 listen_host/listen_port（C-P2-12 裁决：无独立默认端口），管理面=127.0.0.1:8900。

## 4. 关键设计裁决索引

| # | 裁决 | 详见 |
|---|------|------|
| 1 | 数据面迁 axum 0.7；CONNECT 直接禁用 403（P0-1） | T3 §2.2 |
| 2 | :8900 管理 API 接管，旧 /stats /refresh 移除 | T6 §7 |
| 3 | token 缓存一致性：管理面写库后同步刷新缓存，单一写路径 | T2 §5 |
| 4 | usage 落库原子性：drain（swap 空表）而非清零 | T5 §4 |
| 5 | bytes_out SSE 统计：axum body stream chunk 累计 | T5 §5 |
| 6 | Store 不做 trait 抽象，具体类型 | T1 §5 |
| 7 | 时间：SystemTime → UTC Unix 秒，无 chrono | §3.3 |
| 8 | 错误：core 枚举 / server anyhow 边界映射 | §3.2 |
| 9 | CONNECT 直接禁用 403；数据面唯一实现路径 = accept 循环 + peek 分流 + http1::Builder | T3 §2.2 |
| 10 | 迁移重写 config.json 仅删 `routes` 键，凭据/上游项保留 | T1 §6.1 |

## 5. 已知债务（审核裁决登记，后续里程碑处理）

| 债务 | 现状 | 处理里程碑 |
|------|------|-----------|
| usage_hourly 无保留策略（数据无限增长） | M1 仅累加不清理 | M3（连同告警体系定保留窗口） |
| 管理操作无审计日志（create/revoke/route 变更不可追溯） | M1 无记录 | M3 一并考虑（S-风险 驳回登记） |
| 限速仅全局并发连接上限（Semaphore 256），无 per-token QPS/配额 | M1 无完整限速框架 | P1 后续迭代 |

## 6. 文件清单

| 文件 | 内容 |
|------|------|
| [T1-storage.md](T1-storage.md) | git 基线、依赖统一、store.rs（DDL/迁移导入/Store API） |
| [T2-token.md](T2-token.md) | token 生成/校验/CRUD/缓存 + admin 引导（含 T7） |
| [T3-dataauth.md](T3-dataauth.md) | 数据面 axum 改造 + 路径鉴权 + CONNECT 禁用 |
| [T4-route.md](T4-route.md) | 路由表动态化 + 上游选择 + SSRF 校验 |
| [T5-usage.md](T5-usage.md) | 用量计数 + 小时落库 + 查询聚合 |
| [T6-adminapi.md](T6-adminapi.md) | 管理 REST API 全端点 + :8900 处置 |
| [T7-bootstrap.md](T7-bootstrap.md) | admin token 首启引导（归属与验收说明） |
| [T8-verification.md](T8-verification.md) | scripts/m1_test.sh 集成回归 |
