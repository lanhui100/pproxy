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
    /// 周期总配额（字节）。None = 未开通会员（老库 / 普通 Basic 用户）。
    #[serde(default)]
    pub quota_bytes: Option<i64>,
    /// 签发令牌的唯一 ID（gate 侧用量与撤销据此关联）。None = 未签发过令牌。
    #[serde(default)]
    pub jti: Option<String>,
    /// 最大并发长连接数。None = 未设置。
    #[serde(default)]
    pub max_conns: Option<i64>,
    /// 角色：admin / user。None = 未设置（按普通用户处理）。
    #[serde(default)]
    pub role: Option<String>,
}

/// 统一的 users 列清单：所有 SELECT 与 `map_user_row` 索引共用，
/// 避免新增列时某处 SELECT 漏列导致索引错位（D3）。
const USER_COLUMNS: &str = "id, username, password_hash, created_at, disabled, expires_at, \
                            last_used_at, quota_bytes, jti, max_conns, role";

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
        quota_bytes: row.get(7)?,
        jti: row.get(8)?,
        max_conns: row.get(9)?,
        role: row.get(10)?,
    })
}

impl Store {
    /// 基础用户插入（兼容旧调用方）：不携带会员字段。
    pub fn insert_user(
        &self,
        username: &str,
        password_hash: &str,
        created_at: i64,
        expires_at: Option<i64>,
    ) -> Result<UserRow, StoreError> {
        self.insert_user_ex(
            username,
            password_hash,
            created_at,
            expires_at,
            None,
            None,
            None,
            None,
        )
    }

    /// 带商业化会员字段的用户插入。
    #[allow(clippy::too_many_arguments)]
    pub fn insert_user_ex(
        &self,
        username: &str,
        password_hash: &str,
        created_at: i64,
        expires_at: Option<i64>,
        quota_bytes: Option<u64>,
        jti: Option<&str>,
        max_conns: Option<u32>,
        role: Option<&str>,
    ) -> Result<UserRow, StoreError> {
        // SQLite INTEGER 为有符号 64 位：超范围宁可报错，也不静默截断成负数配额
        let quota_i64 = quota_bytes
            .map(i64::try_from)
            .transpose()
            .map_err(|_| StoreError::Migration("quota_bytes 超出 i64 范围".into()))?;
        let max_conns_i64 = max_conns.map(i64::from);
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO users (username, password_hash, created_at, disabled, expires_at, \
             last_used_at, quota_bytes, jti, max_conns, role)
             VALUES (?1, ?2, ?3, 0, ?4, NULL, ?5, ?6, ?7, ?8)",
            params![
                username,
                password_hash,
                created_at,
                expires_at,
                quota_i64,
                jti,
                max_conns_i64,
                role
            ],
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
            quota_bytes: quota_i64,
            jti: jti.map(str::to_string),
            max_conns: max_conns_i64,
            role: role.map(str::to_string),
        })
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE username = ?1"
        ))?;
        let row = stmt.query_row(params![username], map_user_row).optional()?;
        Ok(row)
    }

    pub fn get_user_by_id(&self, id: i64) -> Result<Option<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"))?;
        let row = stmt.query_row(params![id], map_user_row).optional()?;
        Ok(row)
    }

    pub fn list_users(&self) -> Result<Vec<UserRow>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt =
            conn.prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY id ASC"))?;
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
