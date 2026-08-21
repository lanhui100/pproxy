//! routes 表 CRUD。upstream 列可空：NULL=自动选择。

use super::{query_opt, now_unix, NewRoute, RouteRow, Store, StoreError};
use rusqlite::params;

impl Store {
    pub fn insert_route(&self, r: &NewRoute) -> Result<(), StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO routes (name, target_host, override_upstream, enabled, created_at)
             VALUES (?1, ?2, ?3, 1, ?4)",
            params![r.name, r.target_host, r.override_upstream, now_unix() as i64],
        )?;
        Ok(())
    }

    pub fn list_routes(&self) -> Result<Vec<RouteRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare("SELECT * FROM routes ORDER BY name")?;
        let rows = stmt.query_map([], map_route)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_route(&self, name: &str) -> Result<Option<RouteRow>, StoreError> {
        let conn = self.lock_conn();
        query_opt(&conn, "SELECT * FROM routes WHERE name = ?1", params![name], map_route)
    }

    /// 三态参数（C-P0-2，配合 serde double_option）：
    /// None=不改该列；Some(None)=清除（置 NULL）；Some(Some(v))=设置。
    /// 返回是否命中（false=路由不存在）。
    pub fn update_route(
        &self,
        name: &str,
        override_upstream: Option<Option<String>>,
        enabled: Option<bool>,
    ) -> Result<bool, StoreError> {
        if override_upstream.is_none() && enabled.is_none() {
            // 两参数均 None：无列可改，仅报告存在性
            return self.get_route(name).map(|r| r.is_some());
        }
        let conn = self.lock_conn();
        let n = conn.execute(
            "UPDATE routes SET
               override_upstream = CASE WHEN ?2 THEN ?3 ELSE override_upstream END,
               enabled = CASE WHEN ?4 THEN ?5 ELSE enabled END
             WHERE name = ?1",
            params![
                name,
                override_upstream.is_some(),
                override_upstream.clone().flatten(),
                enabled.is_some(),
                enabled,
            ],
        )?;
        Ok(n > 0)
    }

    pub fn delete_route(&self, name: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn();
        let n = conn.execute("DELETE FROM routes WHERE name = ?1", params![name])?;
        Ok(n > 0)
    }
}

fn map_route(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouteRow> {
    Ok(RouteRow {
        name: row.get("name")?,
        target_host: row.get("target_host")?,
        upstream: row.get("upstream")?,
        override_upstream: row.get("override_upstream")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        created_at: row.get::<_, Option<i64>>("created_at")?.unwrap_or(0) as u64,
    })
}
