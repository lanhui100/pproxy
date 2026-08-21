//! usage_hourly 计数落库与查询。累加语义（T1 §8.5）：同主键二次写入数值累加。

use super::{Store, StoreError, UsageRow};
use rusqlite::params;

impl Store {
    /// 单事务批量 upsert；同主键累加（`excluded` 引用本次写入值）。
    pub fn upsert_usage(&self, rows: &[UsageRow]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        for r in rows {
            tx.execute(
                "INSERT INTO usage_hourly
                     (ts_hour, route, token_id, requests, bytes_in, bytes_out)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(ts_hour, route, token_id) DO UPDATE SET
                     requests  = requests  + excluded.requests,
                     bytes_in  = bytes_in  + excluded.bytes_in,
                     bytes_out = bytes_out + excluded.bytes_out",
                params![
                    r.ts_hour as i64,
                    r.route,
                    r.token_id,
                    r.requests as i64,
                    r.bytes_in as i64,
                    r.bytes_out as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 闭区间 [since_hour, now]（C-P2-11：含 since_hour 当小时）。
    /// 动态过滤用固定 SQL + 条件绑定（S-P2-10：禁止 format! 拼 SQL）。
    pub fn query_usage(
        &self,
        since_hour: u64,
        route: Option<&str>,
        token_id: Option<i64>,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM usage_hourly
             WHERE ts_hour >= ?1
               AND (?2 IS NULL OR route = ?2)
               AND (?3 IS NULL OR token_id = ?3)
             ORDER BY ts_hour, route, token_id",
        )?;
        let rows = stmt.query_map(
            params![since_hour as i64, route, token_id],
            map_usage,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn map_usage(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRow> {
    Ok(UsageRow {
        ts_hour: row.get::<_, i64>("ts_hour")? as u64,
        route: row.get("route")?,
        token_id: row.get("token_id")?,
        requests: row.get::<_, i64>("requests")? as u64,
        bytes_in: row.get::<_, i64>("bytes_in")? as u64,
        bytes_out: row.get::<_, i64>("bytes_out")? as u64,
    })
}
