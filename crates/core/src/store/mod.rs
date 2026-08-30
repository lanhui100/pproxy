//! 存储层：SQLite 连接、DDL、config.json 迁移导入、CRUD。
//!
//! WHY 不做 trait 抽象（裁决 #6）：单进程单存储，trait 会迫使方法
//! async-object-safe 化，徒增复杂度；api.rs 直接持有 `Store` 具体类型。
//!
//! WHY 全部方法为同步签名（C-P1-9）：`spawn_blocking` 由调用方负责，
//! 数据面热路径不触 Store（走内存缓存）。

mod migrate;
mod monitor;
mod routes;
#[cfg(test)]
mod tests;
mod tokens;
mod usage;
mod users;

pub use users::UserRow;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::MutexGuard;

use rusqlite::Connection;
use rusqlite::Row;

/// WHY upstream 列可空：NULL=自动选择（RouteRow.upstream 为 Option、
/// 迁移规则"route_upstreams 无映射则 NULL"）。任务 spec §4 DDL 草稿的
/// NOT NULL 与 §6/§6.1.6 的 Option 语义矛盾，按数据模型语义取可空。
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER,
  revoked_at   INTEGER,
  last_used_at INTEGER
);

CREATE TABLE IF NOT EXISTS users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  username      TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  created_at    INTEGER NOT NULL,
  disabled      INTEGER NOT NULL DEFAULT 0,
  expires_at    INTEGER,
  last_used_at  INTEGER
);

CREATE TABLE IF NOT EXISTS routes (
  name              TEXT PRIMARY KEY,
  target_host       TEXT NOT NULL,
  upstream          TEXT,
  override_upstream TEXT,
  enabled           INTEGER NOT NULL DEFAULT 1,
  created_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_hourly (
  ts_hour   INTEGER NOT NULL,
  route     TEXT NOT NULL,
  token_id  INTEGER NOT NULL,
  requests  INTEGER NOT NULL,
  bytes_in  INTEGER NOT NULL,
  bytes_out INTEGER NOT NULL,
  PRIMARY KEY (ts_hour, route, token_id)
);

CREATE TABLE IF NOT EXISTS quota_snapshots (
  ts       INTEGER NOT NULL,
  upstream TEXT NOT NULL,
  metric   TEXT NOT NULL,
  used     INTEGER NOT NULL,
  quota    INTEGER NOT NULL,
  pct      REAL NOT NULL,
  PRIMARY KEY (ts, upstream, metric)
);

CREATE TABLE IF NOT EXISTS alerts (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts      INTEGER NOT NULL,
  level   TEXT NOT NULL,
  message TEXT NOT NULL,
  read_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_usage_route_ts ON usage_hourly(route, ts_hour);
CREATE INDEX IF NOT EXISTS idx_usage_token_ts ON usage_hourly(token_id, ts_hour);
CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);

-- name 唯一性（C-P1-10）：部分唯一索引仅约束未撤销行——撤销后同名可复用
CREATE UNIQUE INDEX IF NOT EXISTS idx_tokens_name ON tokens(name) WHERE revoked_at IS NULL;
";

/// 全模块统一错误（README §3.2），api.rs 据此映射 HTTP 状态码。
#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Migration(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::Migration(s) => write!(f, "migration failed: {s}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Migration(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ---- 时间函数（全模块共用，README §3.3：唯一时间源 SystemTime，UTC 秒）----

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn hour_floor(ts: u64) -> u64 {
    ts - ts % 3600
}

/// DB 默认路径解析（T1 §5）：PPROXY_DB 环境变量优先，其次 $HOME/.pony/state.db。
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("PPROXY_DB") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".pony").join("state.db")
}

// ---- 数据结构 ----

pub use monitor::{AlertRow, MarkReadOutcome, QuotaSnapshotRow};

#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub token_hash: String,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub revoked_at: Option<u64>,
    pub last_used_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteRow {
    pub name: String,
    pub target_host: String,
    pub upstream: Option<String>,
    pub override_upstream: Option<String>,
    pub enabled: bool,
    pub created_at: u64,
}

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

/// 撤销结果三态（C-P1-4）：T2/T6 据此映射 404 / 200。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeOutcome {
    Revoked,
    AlreadyRevoked,
    NotFound,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MigrationOutcome {
    Imported(usize),
    AlreadyMigrated,
    NoLegacyRoutes,
}

pub struct Store {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl Store {
    /// 打开/创建 DB，执行 DDL，返回 (Store, 是否为全新库)。
    /// bool 仅用于首启日志（C-P2-4），无其他逻辑分支依赖。
    pub fn open(path: &Path) -> Result<(Self, bool), StoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
                harden_dir(parent);
            }
        }
        let fresh = !path.exists();
        let conn = Connection::open(path)?;
        harden_file(path);
        // WAL + busy_timeout + NORMAL：个人网关低并发下的持久化/性能平衡（T1 §5）
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL;",
        )?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok((
            Store {
                conn: Mutex::new(conn),
                db_path: path.to_path_buf(),
            },
            fresh,
        ))
    }

    /// 健康检查（C-P1-11）：/api/health 的 db 探测用。
    pub fn ping(&self) -> Result<(), StoreError> {
        let conn = self.lock_conn();
        conn.query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
    }

    pub(crate) fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        // WHY unwrap_or_else：锁中毒（持锁线程 panic）时取回连接继续服务，
        // 个人网关下宁可降级也不整体拒绝
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn db_path_str(&self) -> String {
        self.db_path.display().to_string()
    }
}

/// 行查询的 Option 包装（查无 → None，其余错误透传），tokens/routes 共用。
pub(crate) fn query_opt<T, P, F>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: F,
) -> Result<Option<T>, StoreError>
where
    P: rusqlite::Params,
    F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
{
    match conn.query_row(sql, params, map) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(unix)]
fn harden_dir(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 0700：DB 目录仅属主可入，连带保护同目录 -wal/-shm 文件（S-P2-2）
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn harden_dir(_p: &Path) {}

#[cfg(unix)]
fn harden_file(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 0600：DB 文件仅属主可读写（S-P2-2）
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn harden_file(_p: &Path) {}
