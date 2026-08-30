//! 用户管理与 Basic Auth 凭据服务（UserService）。
//!
//! 提供加盐哈希密码存储、恒定时间比对、内存缓存与最近使用时间节流更新。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::store::{Store, StoreError, UserRow};

/// 恒定时间字节切片比对，杜绝时序侧信道攻击。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

const HASH_ITERATIONS: u32 = 10_000;

fn compute_iterated_hash(salt: &str, password: &str) -> String {
    let mut current = Sha256::digest(format!("{salt}:{password}").as_bytes());
    for _ in 1..HASH_ITERATIONS {
        let mut hasher = Sha256::new();
        hasher.update(&current);
        hasher.update(salt.as_bytes());
        hasher.update(password.as_bytes());
        current = hasher.finalize();
    }
    hex::encode(current)
}

/// 生成增强加盐密码哈希：`salt$iterated_sha256(10000)`
pub fn hash_password(password: &str) -> String {
    let mut salt_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt_bytes);
    let salt = hex::encode(salt_bytes);
    let hash = compute_iterated_hash(&salt, password);
    format!("{salt}${hash}")
}

/// 校验密码与加盐哈希是否匹配（恒定时间比较）。
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    let parts: Vec<&str> = stored_hash.split('$').collect();
    if parts.len() != 2 {
        return false;
    }
    let salt = parts[0];
    let expected_hash = parts[1];

    let calculated_hash = compute_iterated_hash(salt, password);
    constant_time_eq(expected_hash.as_bytes(), calculated_hash.as_bytes())
}

#[derive(Debug)]
pub enum UserError {
    Store(StoreError),
    InvalidUsername(&'static str),
    InvalidPassword(&'static str),
    UserAlreadyExists(String),
    UserNotFound(String),
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "store error: {e}"),
            Self::InvalidUsername(s) => write!(f, "invalid username: {s}"),
            Self::InvalidPassword(s) => write!(f, "invalid password: {s}"),
            Self::UserAlreadyExists(u) => write!(f, "user already exists: {u}"),
            Self::UserNotFound(u) => write!(f, "user not found: {u}"),
        }
    }
}

impl std::error::Error for UserError {}

impl From<StoreError> for UserError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// 用户服务：缓存 + 校验 + 节流。
#[derive(Clone)]
pub struct UserService {
    store: Arc<Store>,
    cache: Arc<DashMap<String, UserRow>>,
    last_touched: Arc<DashMap<i64, Arc<AtomicI64>>>,
}

impl UserService {
    pub fn new(store: Arc<Store>) -> Result<Self, StoreError> {
        let service = Self {
            store,
            cache: Arc::new(DashMap::new()),
            last_touched: Arc::new(DashMap::new()),
        };
        service.refresh_cache()?;
        Ok(service)
    }

    pub fn refresh_cache(&self) -> Result<(), StoreError> {
        let users = self.store.list_users()?;
        self.cache.clear();
        for u in users {
            self.cache.insert(u.username.clone(), u);
        }
        Ok(())
    }

    /// 校验用户名合法性（2-32 字符，字母/数字/下划线/短横线）。
    pub fn validate_username(username: &str) -> Result<(), UserError> {
        if username.len() < 2 || username.len() > 32 {
            return Err(UserError::InvalidUsername("username length must be between 2 and 32 characters"));
        }
        if !username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(UserError::InvalidUsername("username can only contain alphanumeric characters, underscores, and hyphens"));
        }
        Ok(())
    }

    /// 创建用户。
    pub fn create_user(
        &self,
        username: &str,
        password: &str,
        expires_days: Option<u32>,
    ) -> Result<UserRow, UserError> {
        Self::validate_username(username)?;
        if password.len() < 4 {
            return Err(UserError::InvalidPassword("password must be at least 4 characters"));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let expires_at = expires_days.map(|days| now + (days as i64) * 86400);
        let password_hash = hash_password(password);

        let row = self
            .store
            .insert_user(username, &password_hash, now, expires_at)
            .map_err(|e| match &e {
                StoreError::Sqlite(sqlite_err) if sqlite_err.to_string().contains("UNIQUE constraint failed") => {
                    UserError::UserAlreadyExists(username.to_string())
                }
                _ => UserError::Store(e),
            })?;

        self.cache.insert(username.to_string(), row.clone());
        Ok(row)
    }

    /// 校验用户名与密码，若合法且未过期未禁用，返回 UserRow。
    pub fn verify_user(&self, username: &str, password: &str) -> Option<UserRow> {
        let user = self.cache.get(username).map(|r| r.clone())?;

        if user.disabled {
            return None;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        if let Some(exp) = user.expires_at {
            if now > exp {
                return None;
            }
        }

        if !verify_password(password, &user.password_hash) {
            return None;
        }

        self.touch_last_used(user.id, now);
        Some(user)
    }

    /// 节流更新 last_used_at（60 秒内最多写一次库）。
    fn touch_last_used(&self, user_id: i64, now: i64) {
        let entry = self
            .last_touched
            .entry(user_id)
            .or_insert_with(|| Arc::new(AtomicI64::new(0)));
        let last = entry.load(Ordering::Relaxed);
        if now - last > 60 {
            entry.store(now, Ordering::Relaxed);
            let store = self.store.clone();
            tokio::spawn(async move {
                let _ = store.touch_user_last_used(user_id, now);
            });
        }
    }

    pub fn list_users(&self) -> Vec<UserRow> {
        self.cache.iter().map(|r| r.value().clone()).collect()
    }

    pub fn delete_user(&self, username: &str) -> Result<bool, UserError> {
        let deleted = self.store.delete_user(username)?;
        if deleted {
            self.cache.remove(username);
        }
        Ok(deleted)
    }

    pub fn set_disabled(&self, username: &str, disabled: bool) -> Result<bool, UserError> {
        let updated = self.store.set_user_disabled(username, disabled)?;
        if updated {
            if let Some(mut entry) = self.cache.get_mut(username) {
                entry.disabled = disabled;
            }
        }
        Ok(updated)
    }

    pub fn update_password(&self, username: &str, new_password: &str) -> Result<bool, UserError> {
        if new_password.len() < 4 {
            return Err(UserError::InvalidPassword("password must be at least 4 characters"));
        }
        let password_hash = hash_password(new_password);
        let updated = self.store.update_user_password(username, &password_hash)?;
        if updated {
            if let Some(mut entry) = self.cache.get_mut(username) {
                entry.password_hash = password_hash;
            }
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_and_verify() {
        let pass = "CorrectHorseBatteryStaple!";
        let hash = hash_password(pass);
        assert!(verify_password(pass, &hash));
        assert!(!verify_password("WrongPassword", &hash));
        assert!(!verify_password("", &hash));
    }

    #[test]
    fn constant_time_eq_vectors() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"hellp"));
        assert!(!constant_time_eq(b"hello", b"hell"));
    }

    #[tokio::test]
    async fn user_service_crud_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.db");
        let (store, _) = Store::open(&db_path).unwrap();
        let store = Arc::new(store);
        let service = UserService::new(store).unwrap();

        // 1. 创建用户
        let user = service
            .create_user("alice_dev", "AliceSecretPass123", Some(30))
            .unwrap();
        assert_eq!(user.username, "alice_dev");
        assert!(!user.disabled);

        // 2. 验证密码成功
        let verified = service.verify_user("alice_dev", "AliceSecretPass123");
        assert!(verified.is_some());
        assert_eq!(verified.unwrap().id, user.id);

        // 3. 错误密码失败
        assert!(service.verify_user("alice_dev", "WrongPass").is_none());

        // 4. 重复用户名拒绝
        assert!(matches!(
            service.create_user("alice_dev", "another_pass", None),
            Err(UserError::UserAlreadyExists(_))
        ));

        // 5. 禁用用户
        service.set_disabled("alice_dev", true).unwrap();
        assert!(service.verify_user("alice_dev", "AliceSecretPass123").is_none());
        service.set_disabled("alice_dev", false).unwrap();
        assert!(service.verify_user("alice_dev", "AliceSecretPass123").is_some());

        // 6. 修改密码
        service.update_password("alice_dev", "NewPassword456").unwrap();
        assert!(service.verify_user("alice_dev", "AliceSecretPass123").is_none());
        assert!(service.verify_user("alice_dev", "NewPassword456").is_some());

        // 7. 删除用户
        assert!(service.delete_user("alice_dev").unwrap());
        assert!(service.verify_user("alice_dev", "NewPassword456").is_none());
    }
}
