//! SQLite users 存储：CRUD、密码哈希校验、用量关联。

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::{Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
    pub disabled: bool,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

fn map_user_row(row: &Row<'_>) -> rusqlite::Result<UserRow> {
    let disabled_int: i64 = row.get(4)?;
    Ok(UserRow {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        created_at: row.get(3)?,
        disabled: disabled_int != 0,
        expires_at: row.get(5)?,
        last_used_at: row.get(6)?,
    })
}

impl Store {
    pub fn insert_user(
        &self,
        username: &str,
        password_hash: &str,
        created_at: i64,
        expires_at: Option<i64>,
    ) -> Result<UserRow, StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO users (username, password_hash, created_at, disabled, expires_at, last_used_at)
             VALUES (?1, ?2, ?3, 0, ?4, NULL)",
            params![username, password_hash, created_at, expires_at],
        )?;
        let id = conn.last_insert_rowid();
        Ok(UserRow {
            id,
            username: username.to_string(),
            password_hash: password_hash.to_string(),
            created_at,
            disabled: false,
            expires_at,
            last_used_at: None,
        })
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, created_at, disabled, expires_at, last_used_at
             FROM users WHERE username = ?1",
        )?;
        let row = stmt.query_row(params![username], map_user_row).optional()?;
        Ok(row)
    }

    pub fn get_user_by_id(&self, id: i64) -> Result<Option<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, created_at, disabled, expires_at, last_used_at
             FROM users WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], map_user_row).optional()?;
        Ok(row)
    }

    pub fn list_users(&self) -> Result<Vec<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, created_at, disabled, expires_at, last_used_at
             FROM users ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], map_user_row)?;
        let mut users = Vec::new();
        for r in rows {
            users.push(r?);
        }
        Ok(users)
    }

    pub fn delete_user(&self, username: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn();
        let affected = conn.execute("DELETE FROM users WHERE username = ?1", params![username])?;
        Ok(affected > 0)
    }

    pub fn set_user_disabled(&self, username: &str, disabled: bool) -> Result<bool, StoreError> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE users SET disabled = ?1 WHERE username = ?2",
            params![if disabled { 1 } else { 0 }, username],
        )?;
        Ok(affected > 0)
    }

    pub fn update_user_password(&self, username: &str, password_hash: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE users SET password_hash = ?1 WHERE username = ?2",
            params![password_hash, username],
        )?;
        Ok(affected > 0)
    }

    pub fn touch_user_last_used(&self, user_id: i64, ts: i64) -> Result<(), StoreError> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE users SET last_used_at = ?1 WHERE id = ?2",
            params![ts, user_id],
        )?;
        Ok(())
    }
}
