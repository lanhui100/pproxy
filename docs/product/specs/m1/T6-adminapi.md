# T6 — 管理 API（api.rs）

> 依赖: T2 + T4 + T5 | 波次 3 后半（与 T3 串行，后执行——两者都改 main.rs）| 上游: M1 spec §3.5

## 1. 目标

实现 :8900 管理面全部端点（M1 spec §3.5 表）、admin Bearer 中间件、axum Router 组装；处置旧 /stats /refresh。

## 2. 文件位置

- 新增 `crates/server/src/api.rs`。
- 改造 `crates/server/src/main.rs`：挂载 admin Router 到 `PPROXY_LISTEN_ADMIN`（默认 127.0.0.1:8900）。
- `crates/server/Cargo.toml`：无新增（axum/serde_json 已有）。

## 3. 公开接口

```rust
// crates/server/src/api.rs
use axum::{Router, extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminState {
    pub tokens: Arc<TokenService>,
    pub routes: Arc<RouteTable>,
    pub usage: Arc<UsageTracker>,
}

/// 组装管理面 Router（main.rs 调用，绑定 127.0.0.1:8900）
pub fn admin_router(state: AdminState) -> Router;
```

### 3.1 admin Bearer 中间件（精确）

- 提取 `Authorization: Bearer <token>`；缺失/格式错 → `401 {"error":"unauthorized"}`。
- `token_service.verify_admin(token)` 失败 → 同 401（不区分原因，防探测）。
- 校验通过 → 放行。实现为 `axum::middleware::from_fn_with_state`，作用于 `/api/*` 全部路由。

### 3.2 统一错误响应约定（C-P1-7 裁决：以 §4 各端点固定文案为准，本表为汇总）

| 条件 | 状态码 | body |
|------|--------|------|
| 鉴权失败（缺失/错误/非 admin） | 401 | `{"error":"unauthorized"}` |
| 资源不存在（token id / route name） | 404 | `{"error":"not_found"}` |
| 请求体非法 / 校验失败 | 400 | `{"error":"<§4 各端点规定的固定文案>"}` |
| Store/内部错误 | 500 | `{"error":"internal"}`（不外泄内部细节） |

**硬性纪律（C-P1-7）**：400/401/404/500 的 `error` 字段一律为 §4 规定的**固定文案字符串常量**，**禁止**将 `RouteError`/`TokenError`/`StoreError` 的 `Display` 文本透传给客户端（内部错误文本可能含 SQL 细节/路径信息）。错误变体 → 固定文案的映射集中在单一函数（如 `fn error_body(e: &AppError) -> (StatusCode, Json)`），handler 不各自拼写。

## 4. 端点契约（逐个精确）

### POST /api/tokens
- 请求：`{"name": "...", "expires_days": 30}`（`expires_days` 可省略=永不过期）。
- name 校验：非空、长度 ≤64、**不等于 `"__admin__"`**（保留名，400）、不匹配 T2 `NAME_PATTERN` 白名单（400）、`pony_` 前缀（400，同白名单拒绝路径）。
- expires_days 校验：提供时必须 1..=3650，否则 400（S-P2-4）。
- 成功 `201`：`{"id": 1, "name": "...", "token": "pony_<32hex>", "expires_at": null|u64}`——**明文仅此一次**。
- name 重复 → 400 `{"error":"name already exists"}`（Duplicate）。

### GET /api/tokens
- `200`：`{"tokens": [{"id","name","created_at","expires_at","revoked_at","last_used_at","status"}]}`
- `status` 派生：revoked→`"revoked"`；expires_at < now→`"expired"`；否则 `"active"`。
- **脱敏**：不返回 token_hash（列表无任何 token 材料字段）。
- **偏离声明（C-P1-8）**：上游 M1 spec §3.5 原文为"列表含 token 前 8 位便于辨认"——本 spec **不返回**前 8 位（前 8 位 + 可枚举 name 已构成针对性重放的材料；个人单用户场景 name 即足够辨认）。归档任务（T8）同步修正上游 M1 spec §3.5，消除文档间矛盾。

### DELETE /api/tokens/{id}
- 存在且未撤销 → 撤销，`200 {"revoked": true}`（`RevokeOutcome::Revoked`）；已撤销 → `200 {"revoked": true}`（幂等，`RevokeOutcome::AlreadyRevoked`，C-P1-4）；不存在 → 404 `{"error":"not_found"}`（`RevokeOutcome::NotFound`）。
- **禁止撤销 `__admin__`**（id 为 admin 行时 400 `{"error":"cannot revoke admin"}`）——防自锁。

### GET /api/routes
- `200`：`{"routes": [{"name","target_host","upstream","override_upstream","enabled","created_at","effective_upstream"}]}`
- `effective_upstream`：实时决策结果（override 优先，否则 pick_upstream）——GUI 展示用，来自 RouteTable。

### POST /api/routes
- 请求：`{"name","target_host","override_upstream"?}`（override 可省略；提供时必须 `"worker"|"vercel"`，否则 400，S-P2-7）。
- 校验链：RouteError::InvalidName/InvalidHost/InvalidUpstream/Duplicate → 400（映射 §3.2 固定文案）。
- 成功 `201`：`{"name": "...", "upstream": "worker"|"vercel"}`（创建时的自动选择结果）。

### PATCH /api/routes/{name}
- 请求：`{"override_upstream"?,"enabled"?}`（**C-P2-9 裁决：无 `upstream` 字段**——`upstream` 列是创建时的自动选择快照，仅展示用途，不可 PATCH 修改）。
- **三态语义（C-P0-2 裁决，serde `#[serde(default, with = "double_option")]`）**：
  - 字段缺席 → 不改该列；
  - `"override_upstream": null` → 清除 override（置 NULL）；
  - `"override_upstream": "worker"|"vercel"` → 设置（其他值 → 400，S-P2-7）。
- `enabled` 同理：缺席不改；`true`/`false` 设置。
- 不存在 → 404。成功 `200 {"name":..., "enabled":...}`。热生效（RouteTable 写路径保证）。

### DELETE /api/routes/{name}
- 存在 → 删除 `200 {"deleted": true}`；不存在 → 404。

### POST /api/routes/{name}/test
- 不存在 → 404。存在 → `RouteTable::test_route`（经上游请求 `https://{target}/` 首页；独立 10s 超时 client，S-P2-6；handler 内 `spawn_blocking` 包裹——reqwest 调用本身 async，但 RouteTable 同步方法签名统一走 blocking 线程，C-P1-9）。
- `200`：`{"ok": bool, "status": u16|null, "latency_ms": u64|null, "error": str|null}`。

### GET /api/usage
- Query：`hours`（默认 24，范围 1..=720，非法→400）、`route`（可选）、`token_id`（可选）。
- `since_hour = hour_floor(now) - hours*3600`（查询区间**含 since_hour 当小时**，C-P2-11）。
- `200`：`{"hours": 24, "since_hour": u64, "rows": [{"route","token_id","requests","bytes_in","bytes_out"}], "total": {"requests","bytes_in","bytes_out"}}`
- `rows` 来自 `UsageTracker::query`（内存+库合并）；`total` 为 rows 求和。
- **响应 DTO（C-P2-10 裁决）**：`rows` 用**独立响应结构体**（`UsageRowDto { route, token_id, requests, bytes_in, bytes_out }`），**不含 `ts_hour` 字段**——`UsageTracker::query` 返回的 `UsageRow.ts_hour` 是聚合哨兵值，禁止透出（避免客户端误读为逐小时序列）。

### GET /api/health
- `200`：`{"status": "ok", "routes": {"<name>": {"enabled": bool, "upstream": "worker"|"vercel"}}, "tokens_active": u64, "db": "ok"}`
- `routes.<name>.upstream` 取 `RouteTable::effective_upstream(name)`（C-P1-5）。
- `tokens_active`：缓存中未撤销未过期计数；`db`：`store.ping()`（`SELECT 1`，C-P1-11）成功为 "ok"（失败 500）。**不做**逐上游探测（慢；连通性用 /test 按需）。

### GET /api/alerts
- M3 预留：`200 {"alerts": []}`（常量空数组 + 注释 `// M3: 读取 alerts 表`）。

## 5. Router 组装

```text
Router（admin_router）
  /api/tokens          POST GET
  /api/tokens/{id}     DELETE        （id: u64 Path 解析失败→400）
  /api/routes          GET POST
  /api/routes/{name}   PATCH DELETE
  /api/routes/{name}/test POST
  /api/usage           GET
  /api/health          GET
  /api/alerts          GET
  layer: Bearer admin 中间件（全部端点，无例外——health 也要鉴权，
         理由：管理面仅绑 127.0.0.1，但 CF Tunnel 误配时暴露面最小化）
```

## 6. main.rs 集成

- `AdminState { tokens, routes, usage }` 构造后 `admin_router(state)`，绑定 `PPROXY_LISTEN_ADMIN`（默认 `127.0.0.1:8900`）。
- 与数据面 serve 并存：两个 `TcpListener` 各自 `tokio::spawn(axum::serve(...))`（数据面 CONNECT 分流见 T3）。

## 7. :8900 旧端点处置（裁决 #2）

**裁决：移除 /stats 与 /refresh，不做兼容层。**

理由：
1. 二者服务对象是已停用的免费代理池（`countries=[]` 时 Pool 跳过全部逻辑，/stats 恒返回空池、/refresh 无效果）——保留即撒谎。
2. 管理面新协议以 `/api/*` 为命名空间，路径不冲突，无客户端兼容负担（唯一已知消费者是人工 curl 排查）。
3. M1 spec §2 明确池不在范围内；Pool 代码本身保留在 core（不删，M3 评估），仅管理端点下线。

**迁移说明（写入 docs/ops/API.md 的变更由 T8 归档任务执行）**：`GET :8900/stats` → 移除（池停用，无替代）；`POST :8900/refresh` → 移除；服务健康改用 `GET :8900/api/health`（需 admin Bearer）。

## 8. 依赖任务

T2（TokenService: create/list/revoke/verify_admin；RevokeOutcome 三态）、T4（RouteTable CRUD/test/pick_upstream pub/effective_upstream）、T5（UsageTracker::query）。

## 9. 单元测试清单（axum oneshot，临时 DB 全套服务）

1. 无 Authorization → 401；`Bearer` 格式错（无空格/非 Bearer scheme）→ 401。
2. 数据 token 过 admin 校验 → 401（verify_admin 隔离）。
3. POST /api/tokens 正常 → 201，token 匹配 `^pony_[0-9a-f]{32}$`，expires_at=now+30d±60s；expires_days=0 / 3651 → 400（S-P2-4）。
4. POST /api/tokens name=`__admin__` → 400；name 重复 → 400；name=`pony_x` / 含非法字符 → 400（S-P2-3 白名单）。
5. GET /api/tokens → 无 hash 字段、无 token 前 8 位字段（C-P1-8）、status 派生正确（active/revoked）。
6. DELETE /api/tokens/{id}：正常 200；二次 200 幂等（RevokeOutcome::AlreadyRevoked，C-P1-4）；不存在 404（NotFound）；admin id → 400。
7. POST /api/routes 合法 → 201 且 openai host 自动选 vercel；`validate_host` 拒绝项 → 400。
8. PATCH（C-P0-2/C-P2-9）：改 enabled → 200 且 GET 反映；`override_upstream: null` 清除；字段缺席时 GET 不变；请求体含 `upstream` 字段 → 400（不可改列）；`override_upstream: "bogus"` → 400（S-P2-7）；不存在 404。
9. DELETE /api/routes/{name} → 200/404。
10. POST /api/routes/{name}/test：不存在 404；存在时返回结构含 ok/status/latency_ms（网络调用标 `#[ignore]`，同 T4 策略）。
11. GET /api/usage：造 2 次 record + 1 次 flush → rows/total 正确；**rows 元素无 ts_hour 键**（C-P2-10）；hours=0 → 400；hours=1000 → 400。
12. GET /api/health → 200，`db:"ok"`，tokens_active 正确，routes 的 upstream 为 effective_upstream。
13. GET /api/alerts → `{"alerts":[]}`。
14. 中间件覆盖：/api/health 无 token 也 401。
15. **错误契约（C-P1-7）**：全部 4xx/5xx 响应的 `error` 字段 ∈ §3.2/§4 固定文案集合；构造一个 Store 错误路径（如 DB 文件删除后操作）断言 500 body 恰为 `{"error":"internal"}`，无 rusqlite 文本。

## 10. 验收标准

- `cargo test -p pproxy-server api::` 全绿（网络用例除外）。
- M1 spec §3.5 表 9 个端点全部实现且契约一致（逐条对照）。
- :8900 上 /stats /refresh 返回 404（axum 默认 fallback）。
- 错误响应体全部符合 §3.2 表（审查项：无内部错误细节外泄、无 Display 透传、无 token 材料出现在任何响应）。
- 500 响应不含 rusqlite 错误文本（统一映射 internal）。
- GET /api/tokens 不含 token 前 8 位（偏离声明已同步上游 spec，C-P1-8）。
