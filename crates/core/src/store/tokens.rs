//! tokens 表 CRUD。业务校验（过期/撤销判断）不在本模块（README §3.1）。

use super::{query_opt, now_unix, Store, StoreError, TokenRow, RevokeOutcome};
use rusqlite::params;

impl Store {
    pub fn insert_token(
        &self,
        name: &str,
        token_hash: &str,
        expires_at: Option<u64>,
    ) -> Result<i64, StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO tokens (name, token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![name, token_hash, now_unix() as i64, expires_at.map(|v| v as i64)],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, StoreError> {
        let conn = self.lock_conn();
        query_opt(&conn, TOKEN_BY_HASH_SQL, params![hash], map_token)
    }

    pub fn get_token_by_id(&self, id: i64) -> Result<Option<TokenRow>, StoreError> {
        let conn = self.lock_conn();
        query_opt(&conn, TOKEN_BY_ID_SQL, params![id], map_token)
    }

    pub fn list_tokens(&self) -> Result<Vec<TokenRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare("SELECT * FROM tokens ORDER BY id")?;
        let rows = stmt.query_map([], map_token)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 软删（revoked_at=now）。三态语义（C-P1-4）。
    pub fn revoke_token(&self, id: i64) -> Result<RevokeOutcome, StoreError> {
        let conn = self.lock_conn();
        let n = conn.execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE id = ?2 AND revoked_at IS NULL",
            params![now_unix() as i64, id],
        )?;
        Ok(match n {
            1 => RevokeOutcome::Revoked,
            // 区分"已撤销"与"不存在"（C-P1-4：T2/T6 映射不同状态码）
            0 => match query_opt(&conn, TOKEN_BY_ID_SQL, params![id], map_token)? {
                Some(_) => RevokeOutcome::AlreadyRevoked,
                None => RevokeOutcome::NotFound,
            },
            _ => unreachable!("id 主键更新至多影响一行"),
        })
    }

    /// last_used_at 节流写库入口（T2 调用，节流判断在 token.rs）。
    pub fn touch_token(&self, id: i64, ts: u64) -> Result<(), StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE tokens SET last_used_at = ?1 WHERE id = ?2",
            params![ts as i64, id],
        )?;
        Ok(())
    }
}

const TOKEN_BY_HASH_SQL: &str = "SELECT * FROM tokens WHERE token_hash = ?1";
const TOKEN_BY_ID_SQL: &str = "SELECT * FROM tokens WHERE id = ?1";

fn map_token(row: &rusqlite::Row<'_>) -> rusqlite::Result<TokenRow> {
    Ok(TokenRow {
        id: row.get("id")?,
        name: row.get("name")?,
        token_hash: row.get("token_hash")?,
        created_at: row.get::<_, Option<i64>>("created_at")?.unwrap_or(0) as u64,
        expires_at: row.get::<_, Option<i64>>("expires_at")?.map(|v| v as u64),
        revoked_at: row.get::<_, Option<i64>>("revoked_at")?.map(|v| v as u64),
        last_used_at: row
            .get::<_, Option<i64>>("last_used_at")?
            .map(|v| v as u64),
    })
}
