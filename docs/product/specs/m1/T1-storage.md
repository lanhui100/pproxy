# T1 — 存储层（git 基线 + workspace 依赖 + store.rs）

> 依赖: 无（波次 1，串行先行） | 上游: M1 spec §3.6 / §3.3 迁移

## 1. 目标

1. 项目纳入 git 管理，打 `pre-m1` 基线 tag（回滚保障）。
2. workspace 依赖统一（rusqlite bundled / sha2 / rand / hex 等入 workspace.dependencies）。
3. 实现 `crates/core/src/store.rs`：SQLite schema、config.json 迁移导入、Store CRUD API。
4. 产出全部模块共用的 `StoreError` 与时间函数。

## 2. git 基线（实现第一步，先于任何代码改动）

```bash
cd /home/USER/pproxy
git init
# .gitignore 必须含: /target, .secrets.env, *.db, *.db-wal, *.db-shm, config.json, config.json.bak
git add -A
git commit -m "pre-m1 baseline"
git tag pre-m1
```

- `.secrets.env` 若存在必须进 .gitignore（600 凭据，禁止入库）。
- `target/` 禁止入库。
- **`config.json` 禁止入库**（含真实 worker_secret，S-P1-2 裁决）；同时新建 `config.example.json` 入库，结构与 config.json 一致，但 secret 类字段（worker_secret、upstreams[*].secret）一律为占位符 `"REPLACE_ME"`。
- 验收：`git tag` 输出含 `pre-m1`；`git status` 干净；`git check-ignore config.json` 命中；仓库内存在 `config.example.json` 且 `grep -c REPLACE_ME config.example.json` ≥ 2。

## 3. workspace 依赖统一

根 `Cargo.toml` `[workspace.dependencies]` 追加：

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
sha2 = "0.10"
rand = "0.8"
hex = "0.4"
dashmap = "6"
```

- `pproxy-core` 的 Cargo.toml 增加上述全部；`pproxy-server` 增 `dashmap`（若 T5 落在 core 则 server 不需要，按实际引用添加，禁止两边重复声明不同版本）。
- 版本锁定后 `cargo update` 提交 Cargo.lock。
- 理由：bundled 特性免系统 libsqlite 依赖；rand 0.8 API 稳定（`rand::Rng::gen` / `thread_rng`）。

## 4. Schema DDL（store.rs 内常量，启动时 `execute_batch`，`CREATE TABLE IF NOT EXISTS`）

```sql
CREATE TABLE IF NOT EXISTS tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER,
  revoked_at   INTEGER,
  last_used_at INTEGER
);

CREATE TABLE IF NOT EXISTS routes (
  name             TEXT PRIMARY KEY,
  target_host      TEXT NOT NULL,
  upstream         TEXT NOT NULL,             -- 'worker' | 'vercel'
  override_upstream TEXT,                      -- 非空时覆盖自动选择
  enabled          INTEGER NOT NULL DEFAULT 1,
  created_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_hourly (
  ts_hour  INTEGER NOT NULL,                  -- UTC 秒，已对齐小时
  route    TEXT NOT NULL,
  token_id INTEGER NOT NULL,
  requests INTEGER NOT NULL,
  bytes_in INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  PRIMARY KEY (ts_hour, route, token_id)
);

CREATE TABLE IF NOT EXISTS quota_snapshots (  -- 预留，M3
  ts INTEGER NOT NULL, upstream TEXT NOT NULL, metric TEXT NOT NULL,
  used INTEGER NOT NULL, quota INTEGER NOT NULL, pct REAL NOT NULL,
  PRIMARY KEY (ts, upstream, metric)
);

CREATE TABLE IF NOT EXISTS alerts (           -- 预留，M3
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts INTEGER NOT NULL, level TEXT NOT NULL,
  message TEXT NOT NULL, read_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_usage_route_ts ON usage_hourly(route, ts_hour);
CREATE INDEX IF NOT EXISTS idx_usage_token_ts ON usage_hourly(token_id, ts_hour);

-- name 唯一性（C-P1-10）：部分唯一索引，仅约束未撤销行——撤销后同名可复用
CREATE UNIQUE INDEX IF NOT EXISTS idx_tokens_name ON tokens(name) WHERE revoked_at IS NULL;
```

## 5. 设计裁决

- **Store 不做 trait 抽象**（裁决 #6）：单实现、单进程、无第二存储需求（YAGNI）；trait 会迫使所有方法 async-object-safe 化，徒增复杂度。api.rs 直接持有 `Store` 具体类型。
- DB 路径解析顺序：`PPROXY_DB` 环境变量 → `dirs` 不引入，直接 `std::env::var("HOME")` 拼 `$HOME/.pony/state.db`。目录不存在则 `create_dir_all`。
- **文件权限（S-P2-2）**：`create_dir_all` 后将 DB 目录 chmod `0700`（`std::fs::set_permissions` + `PermissionsExt::from_mode`）；DB 文件创建后将文件 chmod `0600`。仅 Unix 生效，`#[cfg(unix)]`。
- 连接参数：`Connection::open` 后立即 `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL;`。
- **SQL 纪律（S-P2-10）**：全部 SQL 一律使用 rusqlite 参数绑定（`params![]` / `named_params![]`），**禁止**任何 `format!` / 字符串拼接构造 SQL（含表名、列名——它们全部是编译期常量）；`query_usage` 的动态过滤条件用固定 SQL + 条件绑定参数表达。此为硬性代码审查项。

## 6. 公开接口（Rust 签名级，实现不得偏离）

**并发模型（C-P1-9）**：Store 全部方法为**同步 `pub fn`**（内部 `Mutex<Connection>`，std Mutex）。Store 不做 `spawn_blocking` 封装——由各调用方自行处理：T2 的 touch 路径、T5 的 flush、T6 的管理端点 handler 内自行 `tokio::task::spawn_blocking` 包裹；数据面热路径不直接调 Store（走内存缓存）。

```rust
// crates/core/src/store.rs
use std::sync::Mutex;
use rusqlite::Connection;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Migration(String),
}
impl std::fmt::Display for StoreError { /* ... */ }
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError { /* StoreError::Sqlite */ }

// ---- 时间函数（全模块共用，见 README §3.3）----
pub fn now_unix() -> u64;
pub fn hour_floor(ts: u64) -> u64;

// ---- 数据结构 ----
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub token_hash: String,       // 64 位小写 hex
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub revoked_at: Option<u64>,
    pub last_used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteRow {
    pub name: String,
    pub target_host: String,
    pub upstream: Option<String>,          // NULL=自动选择（展示用）
    pub override_upstream: Option<String>, // 非空覆盖自动选择
    pub enabled: bool,
    pub created_at: u64,
}

pub struct Store { conn: Mutex<Connection> }

impl Store {
    /// 打开/创建 DB，执行 DDL，返回 (Store, 是否为全新库)。
    /// bool 仅用于首启日志（main.rs 打 "created new db"，C-P2-4），无其他逻辑分支依赖。
    pub fn open(path: &Path) -> Result<(Self, bool), StoreError>;

    /// 健康检查：`SELECT 1`（C-P1-11，/api/health 的 db 探测用）
    pub fn ping(&self) -> Result<(), StoreError>;

    // ---- tokens ----
    pub fn insert_token(&self, name: &str, token_hash: &str,
        expires_at: Option<u64>) -> Result<i64, StoreError>;
    pub fn get_token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, StoreError>;
    pub fn get_token_by_id(&self, id: i64) -> Result<Option<TokenRow>, StoreError>;
    pub fn list_tokens(&self) -> Result<Vec<TokenRow>, StoreError>;
    pub fn revoke_token(&self, id: i64) -> Result<RevokeOutcome, StoreError>;
    pub fn touch_token(&self, id: i64, ts: u64) -> Result<(), StoreError>;

    // ---- routes ----
    pub fn insert_route(&self, r: &NewRoute) -> Result<(), StoreError>;
    pub fn list_routes(&self) -> Result<Vec<RouteRow>, StoreError>;
    pub fn get_route(&self, name: &str) -> Result<Option<RouteRow>, StoreError>;
    /// 三态参数（C-P0-2，serde double_option 配合）：
    /// None=不改该列；Some(None)=清除（置 NULL）；Some(Some(v))=设置。
    pub fn update_route(&self, name: &str,
        override_upstream: Option<Option<String>>,
        enabled: Option<bool>) -> Result<bool, StoreError>;
    pub fn delete_route(&self, name: &str) -> Result<bool, StoreError>;

    // ---- usage ----
    pub fn upsert_usage(&self, rows: &[UsageRow]) -> Result<(), StoreError>;
    /// 含 since_hour 当小时（闭区间 [since_hour, now]）
    pub fn query_usage(&self, since_hour: u64, route: Option<&str>,
        token_id: Option<i64>) -> Result<Vec<UsageRow>, StoreError>;

    // ---- config.json 迁移 ----
    /// 检测 routes 表为空且 config.json 含非空 routes → 导入。
    /// 导入前复制 config.json → config.json.bak（已存在 .bak 则跳过备份，防覆盖原始文件）。
    /// 幂等：routes 表非空即跳过。导入成功后将 config.json 重写为
    /// 删除 `routes` 键后的原对象（保留 listen_host/listen_port/worker_url/
    /// worker_secret/upstreams/route_upstreams/db_path，见 §6.1 第 7 步）。
    pub fn migrate_config_if_needed(&self, config_path: &Path)
        -> Result<MigrationOutcome, StoreError>;
}

/// 撤销结果三态（C-P1-4）：T2/T6 据此映射 404 / 200
pub enum RevokeOutcome { Revoked, AlreadyRevoked, NotFound }

pub enum MigrationOutcome { Imported(usize), AlreadyMigrated, NoLegacyRoutes }

#[derive(Debug, Clone)]
pub struct NewRoute {
    pub name: String,
    pub target_host: String,
    pub override_upstream: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageRow {
    pub ts_hour: u64,
    pub route: String,
    pub token_id: i64,
    pub requests: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}
```

### 6.1 迁移细节（必须精确实现）

1. 读 `config_path`（不存在 → `NoLegacyRoutes`）。
2. 反序列化为 `serde_json::Value`（宽松，容忍未知字段）。
3. `routes` 键缺失或空对象 → `NoLegacyRoutes`。
4. `list_routes()` 非空 → `AlreadyMigrated`（幂等保障，**不**重写 config.json——无论 config 是否已重写，均原样返回，T8 步骤 11 断言此态下服务正常）。
5. 备份：`config.json` → `config.json.bak`（已存在 .bak 则不覆盖——.bak 永远是原始文件）。
6. 事务内逐条 `insert_route`：`upstream` 取 `route_upstreams[route]`（无则 NULL），`override_upstream=NULL`，`enabled=1`，`created_at=now_unix()`。
7. 事务提交后原子重写 config.json（写 `.tmp` 再 `rename`）：**仅删除顶层 `routes` 键，其余键全部原样保留**（P0-2 裁决：listen_host/listen_port/worker_url/worker_secret/upstreams/route_upstreams 均为基础设施项，与 listen 同类，重启后仍需从 config 读出构造 EdgeClient）；另新增 `"db_path"` 字段记录实际 DB 路径（便于运维排查）。禁止把 config 重写为仅含 listen 的精简格式——那会导致重启后上游凭据丢失、全路由 503。
8. `worker_url` / `worker_secret` / `upstreams` / `route_upstreams` **不落库**，但**保留在 config.json 中**（含 secret 本就不应入 git，见第 2 节 .gitignore 联动）。迁移时由调用方（main.rs）从 config.json 读出后传入 EdgeClient 构造；main.rs 必须**先**读完整旧 config 构造 EdgeClient，**再**调 `migrate_config_if_needed`（顺序硬性约定，T3 spec 重复声明；重写后 config 仍含这些键，但先读后写可彻底规避读写竞态）。

## 7. 依赖任务

无（波次 1）。

## 8. 单元测试清单（store.rs `#[cfg(test)]`，用 `tempfile` 或 `std::env::temp_dir()+进程 id` 建临时 DB；tempfile 加入 dev-dependencies）

1. `open` 两次同一路径：第二次 `created=false`，数据保留。
2. token CRUD 往返：insert → by_hash/by_id/list 一致；revoke 后 `revoked_at` 非空且 `get_token_by_hash` 仍返回行（撤销判断在 token.rs，不在 store）。
3. `token_hash` UNIQUE：重复插入返回 `StoreError::Sqlite`（约束冲突）。
4. route CRUD：insert/get/list/update（部分列 None 不覆盖；`Some(None)` 置 NULL；`Some(Some(v))` 设置——C-P0-2 三态语义）/delete 往返。
5. `upsert_usage` 同主键二次写入数值累加（SQL `ON CONFLICT DO UPDATE ... excluded` 累加语义）。
6. `query_usage` 过滤：since_hour / route / token_id 组合；断言结果**含** since_hour 当小时的行（闭区间）。
7. 迁移：构造临时 config.json（含 7 路由）→ `Imported(7)` → routes 表 7 行且 openai/opencode 的 upstream=vercel；config.json 被重写为**删除 `routes` 键后的原对象**（断言 `worker_url`/`worker_secret`/`upstreams`/`route_upstreams`/listen 键仍在、`routes` 键不存在、新增 `db_path`）；`.bak` 存在且内容为原始。
8. 迁移幂等：同一 config 再次调用 → `AlreadyMigrated`，routes 仍 7 行。
9. 迁移无 routes 键 → `NoLegacyRoutes`，config.json 不动。
10. **迁移中断态（C-风险裁决）**：routes 表非空（已导入）+ config.json 仍为旧格式（含 routes 键）→ `AlreadyMigrated` 且 config.json **原样不动**（不删键、不备份）。
11. `hour_floor` 边界：`hour_floor(3600)=3600`、`hour_floor(3599)=0`。
12. **name 部分唯一索引（C-P1-10）**：同 name 二次 `insert_token`（均未撤销）→ `StoreError::Sqlite`（唯一冲突）；`revoke_token` 后同名再次 insert 成功（撤销后可复用名）；此时再插入同 name → 冲突。
13. `revoke_token` 三态：存在且未撤销 → `Revoked`；再次 → `AlreadyRevoked`；不存在 id → `NotFound`。
14. `ping()` → `Ok(())`。

## 9. 验收标准

- `cargo test -p pproxy-core` 全绿（第 8 节 14 项，含迁移中断态与 name 唯一索引用例）。
- `cargo build --release` 成功（server 仍可编译，行为未变）。
- `git log` 首提交为 pre-m1 基线，工作区干净；`config.json` 被 ignore、`config.example.json` 入库且含占位符（S-P1-2）。
- schema 与第 4 节 DDL 逐字段一致（含两个预留表与 `idx_tokens_name` 部分唯一索引）。
- DB 目录 0700 / 文件 600（S-P2-2）；全部 SQL 无 format! 拼接（S-P2-10，代码审查项）。
