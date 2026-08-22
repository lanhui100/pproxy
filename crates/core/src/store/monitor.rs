//! 监控存储方法（M3）：quota_snapshots / alerts 读写 + 保留策略清理。
//!
//! 表 DDL 已在 mod.rs SCHEMA_SQL 建好（M0），本文件只加方法不改 DDL。
//! 全部同步签名 + 固定 SQL 参数绑定（S-P2-10：禁止 format! 拼 SQL）。

use rusqlite::params;

use super::{Store, StoreError};

/// quota_snapshots 行（spec §5：ts = 采样时刻 UTC 小时地板）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuotaSnapshotRow {
    pub ts: u64,
    pub upstream: String,
    pub metric: String,
    pub used: i64,
    pub quota: i64,
    pub pct: f64,
}

/// alerts 行。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlertRow {
    pub id: i64,
    pub ts: u64,
    pub level: String,
    pub message: String,
    pub read_at: Option<u64>,
}

/// 已读标记三态幂等（R4，对齐 RevokeOutcome 先例）：
/// API 层映射 Marked/AlreadyRead → 200，NotFound → 404。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkReadOutcome {
    Marked,
    AlreadyRead,
    NotFound,
}

impl Store {
    /// INSERT OR REPLACE（PK (ts, upstream, metric)：同小时同键重采覆盖）。
    pub fn upsert_quota_snapshot(&self, row: &QuotaSnapshotRow) -> Result<(), StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT OR REPLACE INTO quota_snapshots (ts, upstream, metric, used, quota, pct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.ts as i64,
                row.upstream,
                row.metric,
                row.used,
                row.quota,
                row.pct
            ],
        )?;
        Ok(())
    }

    /// 每 (upstream, metric) 取最新一条（spec §5 /api/quota snapshots 口径）。
    /// 关联子查询取 MAX(ts)，固定 SQL 无拼接。
    pub fn latest_quota_snapshots(&self) -> Result<Vec<QuotaSnapshotRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT q.ts, q.upstream, q.metric, q.used, q.quota, q.pct
             FROM quota_snapshots AS q
             WHERE q.ts = (
                 SELECT MAX(q2.ts) FROM quota_snapshots AS q2
                 WHERE q2.upstream = q.upstream AND q2.metric = q.metric
             )
             ORDER BY q.upstream, q.metric",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(QuotaSnapshotRow {
                ts: row.get::<_, i64>("ts")? as u64,
                upstream: row.get("upstream")?,
                metric: row.get("metric")?,
                used: row.get("used")?,
                quota: row.get("quota")?,
                pct: row.get("pct")?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 落一条告警，返回自增 id（webhook 投递与已读标记都需要）。
    pub fn insert_alert(&self, ts: u64, level: &str, message: &str) -> Result<i64, StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO alerts (ts, level, message) VALUES (?1, ?2, ?3)",
            params![ts as i64, level, message],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 倒序列表；unread_only=true 仅未读；limit 由调用方钳制（API 层 ≤500）。
    pub fn list_alerts(&self, unread_only: bool, limit: u32) -> Result<Vec<AlertRow>, StoreError> {
        let conn = self.lock_conn();
        // 条件分支走两条固定 SQL，避免动态拼 WHERE
        let sql = if unread_only {
            "SELECT id, ts, level, message, read_at FROM alerts
             WHERE read_at IS NULL ORDER BY id DESC LIMIT ?1"
        } else {
            "SELECT id, ts, level, message, read_at FROM alerts
             ORDER BY id DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![i64::from(limit)], map_alert)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 标记已读三态（R4）：先条件更新（仅未读行命中），
    /// 零行命中再查存在性区分 AlreadyRead / NotFound。
    pub fn mark_alert_read(&self, id: i64) -> Result<MarkReadOutcome, StoreError> {
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE alerts SET read_at = ?1 WHERE id = ?2 AND read_at IS NULL",
            params![super::now_unix() as i64, id],
        )?;
        if changed > 0 {
            return Ok(MarkReadOutcome::Marked);
        }
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM alerts WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(if exists > 0 { MarkReadOutcome::AlreadyRead } else { MarkReadOutcome::NotFound })
    }

    /// M1 债务清理：usage_hourly 保留 30 天——删除 ts_hour < cutoff_hour 的行。
    pub fn prune_usage_before(&self, cutoff_hour: u64) -> Result<usize, StoreError> {
        let conn = self.lock_conn();
        Ok(conn.execute(
            "DELETE FROM usage_hourly WHERE ts_hour < ?1",
            params![cutoff_hour as i64],
        )?)
    }

    /// quota_snapshots 保留 90 天——删除 ts < cutoff_ts 的行。
    pub fn prune_quota_before(&self, cutoff_ts: u64) -> Result<usize, StoreError> {
        let conn = self.lock_conn();
        Ok(conn.execute(
            "DELETE FROM quota_snapshots WHERE ts < ?1",
            params![cutoff_ts as i64],
        )?)
    }
}

fn map_alert(row: &rusqlite::Row<'_>) -> rusqlite::Result<AlertRow> {
    Ok(AlertRow {
        id: row.get("id")?,
        ts: row.get::<_, i64>("ts")? as u64,
        level: row.get("level")?,
        message: row.get("message")?,
        read_at: row.get::<_, Option<i64>>("read_at")?.map(|v| v as u64),
    })
}
