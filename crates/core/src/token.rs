//! Token 全生命周期（M1 spec §3.1/§3.2）：生成（数据/admin）、SHA-256 哈希、
//! 校验（未撤销/未过期 + admin 隔离）、CRUD、进程内缓存（启动全量 + 写后刷新）、
//! last_used_at 节流写库、admin token 首启引导（T7 归并于此，见 `bootstrap_admin`）。
//!
//! 一致性边界（裁决 #3，单一写路径）：tokens 表的全部写操作（create / revoke /
//! admin 引导插入 / touch）只发生在 `TokenService` 方法内，api.rs 与 gateway
//! 一律经由本服务，禁止绕过直写 Store。每个写方法在写库成功后同步
//! `reload_cache()` 全量重建缓存（个人网关 <100 行，成本可忽略），换取无失效竞态。
//!
//! 竞态窗口一致性（S-P2-额外）：verify 读缓存、revoke 写库后重载缓存，两操作
//! 并发时存在"verify 已过校验、撤销尚未对其生效"的窗口（重载完成前，在途请求
//! 仍可通过）。这是最终一致级别：窗口上限 = 一次 `reload_cache()`（全量
//! list_tokens，毫秒级）。撤销的防护目标是"此后不再可用"，非"中断在途请求"，
//! 不做每请求回库强一致（代价不匹配个人网关威胁模型）。
//!
//! 同步边界（§5）：公开方法全同步（与 Store 一致，C-P1-9），`spawn_blocking`
//! 由调用方负责——T6 对 create/revoke/list 等触库方法包裹；数据面 verify 热路径
//! 只读内存缓存不触库（throttle 触发的 touch 除外，见 `throttle_touch`）。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::store::{now_unix, RevokeOutcome, Store, StoreError, TokenRow};

pub const TOKEN_PREFIX: &str = "pony_";
pub const ADMIN_PREFIX: &str = "pony_admin_";
/// last_used_at 写库节流窗口（C-P2-13：节流判断一律引用此常量，禁止字面量 60）
pub const LAST_USED_THROTTLE_SECS: u64 = 60;
/// token name 白名单（S-P2-3）：字母/数字/点/下划线/连字符，1-64 字符。
/// 无 regex 依赖，`valid_name` 按此模式手工实现。
pub const NAME_PATTERN: &str = "^[a-zA-Z0-9._-]{1,64}$";
/// admin 保留名（C-P1-3：insert_token 无 is_admin 参数，admin 语义仅由 name 约定）
pub const ADMIN_NAME: &str = "__admin__";

/// admin token 注入环境变量（S-P1-3）
const ADMIN_TOKEN_ENV: &str = "PPROXY_ADMIN_TOKEN";
/// expires_days 合法区间（S-P2-4）：1..=MAX_EXPIRES_DAYS
const MAX_EXPIRES_DAYS: u64 = 3650;
const SECS_PER_DAY: u64 = 86_400;

/// 全模块统一错误（README §3.2），api.rs 据此映射 HTTP 状态码。
/// 任何变体均不携带明文 token（明文永不进错误信息）。
#[derive(Debug)]
pub enum TokenError {
    NotFound,
    Revoked,
    Expired,
    /// name 重复（创建时，部分唯一索引冲突，C-P1-10）
    Duplicate,
    /// name 不匹配 NAME_PATTERN 白名单 / 保留名 / 保留前缀（S-P2-3）
    InvalidName,
    /// expires_days 超出 1..=3650 或过期时刻计算溢出（S-P2-4）
    InvalidExpiry,
    Store(StoreError),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "token not found"),
            Self::Revoked => write!(f, "token revoked"),
            Self::Expired => write!(f, "token expired"),
            Self::Duplicate => write!(f, "token name already exists"),
            Self::InvalidName => write!(f, "invalid token name"),
            Self::InvalidExpiry => write!(f, "invalid expires_days"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for TokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for TokenError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

pub struct TokenService {
    store: Arc<Store>,
    /// key = token_hash（小写 hex）。缓存即全量权威快照，verify 只读缓存不回库（§5）
    cache: RwLock<HashMap<String, TokenRow>>,
    /// token_id -> 上次写库的 ts（节流用，§6）
    last_used: Mutex<HashMap<i64, u64>>,
}

impl TokenService {
    /// 打开即完成 admin 引导（§8）+ 全量加载缓存。
    pub fn new(store: Arc<Store>) -> Result<Self, TokenError> {
        let svc = TokenService {
            store,
            cache: RwLock::new(HashMap::new()),
            last_used: Mutex::new(HashMap::new()),
        };
        svc.bootstrap_admin()?;
        svc.reload_cache()?;
        Ok(svc)
    }

    /// 生成明文数据 token：`pony_` + 16 字节随机 → 32 hex（§3.2）。
    /// 明文仅经返回值流出，永不落库/打日志。
    pub fn generate_token(&self) -> String {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
    }

    /// 生成明文 admin token：`pony_admin_` + 24 字节随机 → 48 hex（§3.2）。
    pub fn generate_admin_token(&self) -> String {
        let mut bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut bytes);
        format!("{ADMIN_PREFIX}{}", hex::encode(bytes))
    }

    /// SHA-256 → 64 小写 hex。哈希输入为完整明文（含前缀）。
    /// 内部用，pub 便于测试（§3）。
    pub fn sha256_hex(&self, token: &str) -> String {
        hex::encode(Sha256::digest(token.as_bytes()))
    }

    /// 创建数据 token；返回 (id, 明文)。明文仅在此时存在，永不落库/打日志。
    ///
    /// name 必须匹配 NAME_PATTERN 白名单，且不为保留名 `__admin__`、不以保留
    /// 前缀 `pony_` 开头（S-P2-3，防与明文 token 格式混淆）；
    /// expires_days 提供时必须在 1..=3650，过期时刻用 checked 算术计算
    /// （溢出 → InvalidExpiry）。
    pub fn create_token(
        &self,
        name: &str,
        expires_days: Option<u64>,
    ) -> Result<(i64, String), TokenError> {
        if !valid_name(name) {
            return Err(TokenError::InvalidName);
        }
        let expires_at = match expires_days {
            None => None,
            Some(d) => {
                if !(1..=MAX_EXPIRES_DAYS).contains(&d) {
                    return Err(TokenError::InvalidExpiry);
                }
                // checked 算术：当前上限下不可达溢出，仍防御将来放宽上限（S-P2-4）
                match d
                    .checked_mul(SECS_PER_DAY)
                    .and_then(|s| now_unix().checked_add(s))
                {
                    Some(ts) => Some(ts),
                    None => return Err(TokenError::InvalidExpiry),
                }
            }
        };
        let plaintext = self.generate_token();
        let hash = self.sha256_hex(&plaintext);
        let id = self.store.insert_token(name, &hash, expires_at).map_err(|e| {
            if is_unique_violation(&e) {
                // 部分唯一索引冲突（C-P1-10）
                TokenError::Duplicate
            } else {
                TokenError::Store(e)
            }
        })?;
        // 单一写路径：写库成功后同步重载；失败则新 token 尚不可用，
        // 宁可管理面报错也不让缓存与库不一致（§5.1 同一原则）
        self.reload_cache().map_err(|e| {
            tracing::error!(error = %e, "create_token 后缓存重载失败");
            TokenError::Store(e)
        })?;
        Ok((id, plaintext))
    }

    pub fn list_tokens(&self) -> Result<Vec<TokenRow>, TokenError> {
        Ok(self.store.list_tokens()?)
    }

    /// 撤销（软删 revoked_at=now）；成功后刷新缓存（失败 fail-closed，§5.1）。
    /// 三态透传 Store 的 RevokeOutcome（C-P1-4）。
    pub fn revoke_token(&self, id: i64) -> Result<RevokeOutcome, TokenError> {
        let outcome = self.store.revoke_token(id)?;
        if outcome == RevokeOutcome::Revoked {
            // §5.1 fail-closed：重载失败时缓存中已撤销行仍可见，数据面会继续
            // 放行——不得返回成功。下次任意写操作或重启自愈。
            self.reload_cache().map_err(|e| {
                tracing::error!(token_id = id, error = %e, "revoke 后缓存重载失败，fail-closed");
                TokenError::Store(e)
            })?;
        }
        Ok(outcome)
    }

    /// 数据面校验入口：哈希比对 + 未撤销 + 未过期 + admin 隔离（S-P2-1）。
    pub fn verify(&self, plaintext: &str) -> Result<TokenRow, TokenError> {
        let row = self.lookup_verified(plaintext)?;
        // S-P2-1 admin 隔离：admin token 不得作数据 token 使用；
        // 与 verify_admin 的拒绝同为 NotFound，不泄露区分信息
        if row.name == ADMIN_NAME {
            return Err(TokenError::NotFound);
        }
        self.throttle_touch(&row);
        Ok(row)
    }

    /// admin token 校验（供 T6 管理中间件）：仅匹配 name == `__admin__` 的行；
    /// 数据 token 不能过 admin 校验（拒绝同为 NotFound，不泄露"这是数据 token"）。
    pub fn verify_admin(&self, plaintext: &str) -> Result<TokenRow, TokenError> {
        let row = self.lookup_verified(plaintext)?;
        if row.name != ADMIN_NAME {
            return Err(TokenError::NotFound);
        }
        self.throttle_touch(&row);
        Ok(row)
    }

    /// admin token 是否已存在（引导判断用）：存在未撤销的 `__admin__` 行。
    pub fn admin_exists(&self) -> bool {
        let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
        cache
            .values()
            .any(|r| r.name == ADMIN_NAME && r.revoked_at.is_none())
    }

    /// 校验公共前段（§3.1 步骤 1-3）：哈希查缓存 → 未撤销 → 未过期。
    /// 缓存 miss 不回库——缓存即全量权威快照（§5）。
    fn lookup_verified(&self, plaintext: &str) -> Result<TokenRow, TokenError> {
        let hash = self.sha256_hex(plaintext);
        let row = {
            let cache = self.cache.read().unwrap_or_else(|p| p.into_inner());
            cache.get(&hash).cloned()
        };
        let row = row.ok_or(TokenError::NotFound)?;
        if row.revoked_at.is_some() {
            return Err(TokenError::Revoked);
        }
        // 严格小于：expires_at == now 视为未过期
        if let Some(exp) = row.expires_at {
            if exp < now_unix() {
                return Err(TokenError::Expired);
            }
        }
        Ok(row)
    }

    /// verify 通过后的 last_used_at 节流写库（§6）。
    ///
    /// 同步调用。实现者取舍（§6 二选一）：本模块不自行 spawn；T6 数据面在纯
    /// async 上下文调用 verify 时，须将 verify 整体经 `spawn_blocking` 包裹
    /// （touch 随之离开 runtime 线程，不阻塞 runtime）。
    fn throttle_touch(&self, row: &TokenRow) {
        let now = now_unix();
        let should_touch = {
            let mut map = self.last_used.lock().unwrap_or_else(|p| p.into_inner());
            match map.get(&row.id) {
                // 窗口内跳过（C-P2-13：窗口一律引用常量）
                Some(prev) if now.saturating_sub(*prev) < LAST_USED_THROTTLE_SECS => false,
                _ => {
                    map.insert(row.id, now);
                    true
                }
            }
        };
        if should_touch {
            // 观测字段，非安全字段：写库失败仅告警，不影响校验结果
            if let Err(e) = self.store.touch_token(row.id, now) {
                tracing::warn!(token_id = row.id, error = %e, "last_used_at 写库失败（忽略）");
            }
        }
    }

    /// 全量重建缓存（§5）：`store.list_tokens()` → HashMap。
    fn reload_cache(&self) -> Result<(), StoreError> {
        let rows = self.store.list_tokens()?;
        let mut map = HashMap::with_capacity(rows.len());
        for r in rows {
            map.insert(r.token_hash.clone(), r);
        }
        *self.cache.write().unwrap_or_else(|p| p.into_inner()) = map;
        Ok(())
    }

    /// admin token 首启引导（§8，T7 归并）：
    /// 1. 已有未撤销 `__admin__` 行 → 跳过（重启不重复生成、不重复打印）；
    /// 2. 否则先查注入 `PPROXY_ADMIN_TOKEN`（S-P1-3）：非空 → 以其哈希落库，
    ///    不校验前缀格式、不打印明文（用户已知自己的 token）；
    /// 3. 未注入 → 生成并落库，`tracing::warn!` 打印明文一次（warn 级别便于
    ///    `journalctl -u pproxy | grep ADMIN_TOKEN` 检索，不被 info 淹没）。
    fn bootstrap_admin(&self) -> Result<(), TokenError> {
        let has_admin = self
            .store
            .list_tokens()?
            .iter()
            .any(|r| r.name == ADMIN_NAME && r.revoked_at.is_none());
        if has_admin {
            return Ok(());
        }
        if let Ok(injected) = std::env::var(ADMIN_TOKEN_ENV) {
            if !injected.is_empty() {
                let hash = self.sha256_hex(&injected);
                self.store.insert_token(ADMIN_NAME, &hash, None)?;
                return Ok(());
            }
        }
        let plaintext = self.generate_admin_token();
        let hash = self.sha256_hex(&plaintext);
        self.store.insert_token(ADMIN_NAME, &hash, None)?;
        // 仅此一次打印明文（§8.3）
        tracing::warn!("ADMIN_TOKEN (仅此一次，请立即保存): {}", plaintext);
        Ok(())
    }
}

/// name 白名单校验（S-P2-3）：NAME_PATTERN 语义手工实现 + 保留名 + 保留前缀。
fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    if n == 0 || n > 64 {
        return false;
    }
    if name == ADMIN_NAME || name.starts_with(TOKEN_PREFIX) {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// 唯一约束冲突判定（→ Duplicate，C-P1-10）
fn is_unique_violation(e: &StoreError) -> bool {
    matches!(
        e,
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// PPROXY_ADMIN_TOKEN 是进程级全局：所有 service 构造经此锁与注入测试互斥。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn open_service(db: &Path) -> TokenService {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(ADMIN_TOKEN_ENV);
        let (store, _) = Store::open(db).unwrap();
        TokenService::new(Arc::new(store)).unwrap()
    }

    /// 注入式 admin 引导（测试 8/9/16 共用）；返回前恢复环境。
    fn service_with_injected_admin(admin: &str, db: &Path) -> TokenService {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(ADMIN_TOKEN_ENV, admin);
        let (store, _) = Store::open(db).unwrap();
        let svc = TokenService::new(Arc::new(store)).unwrap();
        std::env::remove_var(ADMIN_TOKEN_ENV);
        svc
    }

    fn setup() -> (TokenService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let svc = open_service(&dir.path().join("state.db"));
        (svc, dir)
    }

    fn setup_with_token(name: &str) -> (TokenService, tempfile::TempDir, i64, String) {
        let (svc, dir) = setup();
        let (id, plaintext) = svc.create_token(name, None).unwrap();
        (svc, dir, id, plaintext)
    }

    // ---- 1. generate_token 格式与唯一性 ----
    // 无 regex 依赖：以等价的字符级断言覆盖 NAME 式模式（前缀 + 全 hex + 定长）
    #[test]
    fn generate_token_format_and_uniqueness() {
        let (svc, _dir) = setup();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let t = svc.generate_token();
            assert!(t.starts_with(TOKEN_PREFIX));
            let body = &t[TOKEN_PREFIX.len()..];
            assert_eq!(body.len(), 32, "16 字节 → 32 hex");
            assert!(body.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(seen.insert(t), "100 次生成无重复");
        }
    }

    // ---- 2. generate_admin_token 格式 ----
    #[test]
    fn generate_admin_token_format() {
        let (svc, _dir) = setup();
        for _ in 0..100 {
            let t = svc.generate_admin_token();
            assert!(t.starts_with(ADMIN_PREFIX));
            let body = &t[ADMIN_PREFIX.len()..];
            assert_eq!(body.len(), 48, "24 字节 → 48 hex");
            assert!(body.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    // ---- 3. sha256_hex 固定向量（sha256sum 手工计算） ----
    #[test]
    fn sha256_hex_fixed_vector() {
        let (svc, _dir) = setup();
        assert_eq!(
            svc.sha256_hex("pony_abc"),
            "2fcedf685d6273d88a47d151db1b50ca612943c076355b7589a42172bd14da84"
        );
    }

    // ---- 4. create → verify 往返 ----
    #[test]
    fn create_then_verify_roundtrip() {
        let (svc, _dir, _id, plaintext) = setup_with_token("roundtrip");
        assert!(svc.verify(&plaintext).is_ok());
        assert!(matches!(
            svc.verify("pony_deadbeefdeadbeefdeadbeefdeadbeef"),
            Err(TokenError::NotFound)
        ));
    }

    // ---- 5. revoke 三态 + verify 拒绝 ----
    #[test]
    fn revoke_then_verify_revoked_and_second_revoke_already() {
        let (svc, _dir, id, plaintext) = setup_with_token("victim");
        assert_eq!(svc.revoke_token(id).unwrap(), RevokeOutcome::Revoked);
        assert!(matches!(svc.verify(&plaintext), Err(TokenError::Revoked)));
        // C-P1-4 三态：二次撤销 → AlreadyRevoked，不存在 → NotFound
        assert_eq!(
            svc.revoke_token(id).unwrap(),
            RevokeOutcome::AlreadyRevoked
        );
        assert_eq!(svc.revoke_token(id + 9_999).unwrap(), RevokeOutcome::NotFound);
    }

    // ---- 6. 过期边界：== now 不过期，now-1 过期 ----
    // 最小侵入：同模块测试直接改缓存行（spec §7.6 允许手工构造行）
    #[test]
    fn expiry_boundaries_eq_now_passes_minus_one_fails() {
        let (svc, _dir, id, plaintext) = setup_with_token("exp");
        let hash = svc.sha256_hex(&plaintext);
        let now = now_unix();
        {
            let mut cache = svc.cache.write().unwrap_or_else(|p| p.into_inner());
            cache.get_mut(&hash).unwrap().expires_at = Some(now);
        }
        assert!(svc.verify(&plaintext).is_ok(), "expires_at == now 未过期");
        {
            let mut cache = svc.cache.write().unwrap_or_else(|p| p.into_inner());
            cache.get_mut(&hash).unwrap().expires_at = Some(now - 1);
        }
        assert!(matches!(svc.verify(&plaintext), Err(TokenError::Expired)));
        assert_ne!(id, i64::MIN);
    }

    // ---- 7. name 重复 → Duplicate；撤销后同名可复用 ----
    #[test]
    fn duplicate_name_rejected_and_reusable_after_revoke() {
        let (svc, _dir, id, _pt) = setup_with_token("dup");
        // 部分唯一索引冲突（C-P1-10）
        assert!(matches!(
            svc.create_token("dup", None),
            Err(TokenError::Duplicate)
        ));
        assert_eq!(svc.revoke_token(id).unwrap(), RevokeOutcome::Revoked);
        assert!(svc.create_token("dup", None).is_ok(), "撤销后同名可复用");
    }

    // ---- 8. verify_admin：admin Ok，数据 token → NotFound ----
    #[test]
    fn verify_admin_accepts_admin_rejects_data_token() {
        let dir = tempfile::tempdir().unwrap();
        let admin = "pony_admin_injected_admin_plain_for_test_8";
        let svc = service_with_injected_admin(admin, &dir.path().join("state.db"));
        assert!(svc.verify_admin(admin).is_ok());
        let (_id, data_plain) = svc.create_token("data8", None).unwrap();
        // 不泄露"这是数据 token"信息：同为 NotFound
        assert!(matches!(
            svc.verify_admin(&data_plain),
            Err(TokenError::NotFound)
        ));
    }

    // ---- 9. verify 隔离 admin（S-P2-1）：数据面不认 admin token ----
    #[test]
    fn verify_rejects_admin_token_on_data_plane() {
        let dir = tempfile::tempdir().unwrap();
        let admin = "pony_admin_injected_admin_plain_for_test_9";
        let svc = service_with_injected_admin(admin, &dir.path().join("state.db"));
        assert!(matches!(
            svc.verify(admin),
            Err(TokenError::NotFound)
        ));
    }

    // ---- 10. name 白名单（S-P2-3） ----
    #[test]
    fn name_whitelist() {
        let (svc, _dir) = setup();
        assert!(svc.create_token("dev-1.key_2", None).is_ok());
        let s65 = "a".repeat(65);
        let bad = [
            "pony_x",      // 保留前缀
            "__admin__",   // 保留名
            "",            // 空串
            s65.as_str(),  // 65 字符
            "a/b",         // 含 /
            "a b",         // 含空格
            "令牌",        // 含中文
        ];
        for name in bad {
            assert!(
                matches!(svc.create_token(name, None), Err(TokenError::InvalidName)),
                "name {name:?} 应被拒绝"
            );
        }
    }

    // ---- 11. expires_days 校验（S-P2-4） ----
    #[test]
    fn expires_days_validation() {
        let (svc, _dir) = setup();
        assert!(matches!(
            svc.create_token("e0", Some(0)),
            Err(TokenError::InvalidExpiry)
        ));
        assert!(matches!(
            svc.create_token("e3651", Some(3651)),
            Err(TokenError::InvalidExpiry)
        ));
        // checked_add 溢出路径
        assert!(matches!(
            svc.create_token("emax", Some(u64::MAX)),
            Err(TokenError::InvalidExpiry)
        ));
        let before = now_unix();
        let (id, _pt) = svc.create_token("e3650", Some(3650)).unwrap();
        let after = now_unix();
        let row = svc
            .list_tokens()
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap();
        let exp = row.expires_at.unwrap();
        let lower = before + 3650 * SECS_PER_DAY;
        let upper = after + 3650 * SECS_PER_DAY;
        assert!(
            (lower..=upper + 60).contains(&exp),
            "expires_at 应为 now + 3650d（±60s）：{exp} 不在 [{lower}, {}]",
            upper + 60
        );
    }

    // ---- 12. last_used_at 节流：窗口内仅 touch 一次 ----
    #[test]
    fn last_used_throttle_single_touch_within_window() {
        let (svc, _dir, _id, plaintext) = setup_with_token("throttle");
        let row = svc.verify(&plaintext).unwrap();
        let map = svc.last_used.lock().unwrap_or_else(|p| p.into_inner());
        let first = *map.get(&row.id).expect("首次 verify 必须记录 last_used");
        drop(map);
        // 同一窗口内第二次 verify：不重复写库
        let row2 = svc.verify(&plaintext).unwrap();
        let map = svc.last_used.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            map.get(&row2.id),
            Some(&first),
            "窗口内（< LAST_USED_THROTTLE_SECS = {LAST_USED_THROTTLE_SECS}s）不得重复 touch"
        );
        drop(map);
        // 窗口经过（回拨 prev 一个窗口）：下次 verify 重新 touch
        svc.last_used
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(row.id, first - LAST_USED_THROTTLE_SECS);
        svc.verify(&plaintext).unwrap();
        let map = svc.last_used.lock().unwrap_or_else(|p| p.into_inner());
        let third = *map.get(&row.id).unwrap();
        assert!(third > first - LAST_USED_THROTTLE_SECS, "窗口过后必须重新 touch");
        // 库侧同步（观测字段）
        let stored = svc.store.get_token_by_id(row.id).unwrap().unwrap();
        assert_eq!(stored.last_used_at, Some(third));
    }

    // ---- 13. 缓存刷新：create 后新 token 立即可 verify ----
    #[test]
    fn cache_refresh_immediate_verify_after_create() {
        let (svc, _dir) = setup();
        let (_id, plaintext) = svc.create_token("fresh", None).unwrap();
        assert!(
            svc.verify(&plaintext).is_ok(),
            "create 后无需重启即可 verify（写库成功即同步重载缓存）"
        );
    }

    // ---- 14. fail-closed（S-P2-5）：revoke 写库成功但 reload 失败 → Err ----
    #[test]
    fn revoke_fail_closed_when_reload_fails() {
        let (svc, _dir, id, _pt) = setup_with_token("victim");
        // 经独立连接插入 created_at 为非整数文本的脏行：list_tokens 的
        // map_token 解析该行失败 → revoke 后的 reload_cache 必然失败，
        // 而 revoke 的 UPDATE 不受影响（写库成功）
        let conn = rusqlite::Connection::open(_dir.path().join("state.db")).unwrap();
        conn.execute(
            "INSERT INTO tokens (name, token_hash, created_at) VALUES ('bad', 'bad_hash', 'not_a_number')",
            [],
        )
        .unwrap();
        let result = svc.revoke_token(id);
        assert!(
            matches!(result, Err(TokenError::Store(_))),
            "reload 失败必须 fail-closed 返回 Err，而非 Ok"
        );
        // 库中确实已撤销（写库成功在先）
        let revoked: Option<i64> = conn
            .query_row(
                "SELECT revoked_at FROM tokens WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(revoked.is_some());
    }

    // ---- §8 引导：全新 DB 首启生成一次，重启不重复 ----
    #[test]
    fn bootstrap_generates_admin_once_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let svc = open_service(&db);
        assert!(svc.admin_exists());
        // 重启（同一 DB 新 service）：不重复生成
        let svc2 = open_service(&db);
        assert!(svc2.admin_exists());
        let admins = svc2
            .list_tokens()
            .unwrap()
            .into_iter()
            .filter(|r| r.name == ADMIN_NAME && r.revoked_at.is_none())
            .count();
        assert_eq!(admins, 1, "重启后不重复生成 admin");
    }

    // ---- §8 引导：PPROXY_ADMIN_TOKEN 注入（S-P1-3），不校验前缀 ----
    #[test]
    fn bootstrap_uses_injected_env_token_without_prefix_check() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        // 无 pony_admin_ 前缀的任意字符串：哈希落库即用
        let injected = "plain-injected-secret";
        let svc = service_with_injected_admin(injected, &db);
        assert!(svc.verify_admin(injected).is_ok(), "注入值可过管理面鉴权");
        assert!(!svc.admin_exists() || {
            // admin_exists 为 true 且仅此一行
            svc.list_tokens()
                .unwrap()
                .into_iter()
                .filter(|r| r.name == ADMIN_NAME && r.revoked_at.is_none())
                .count()
                == 1
        });
    }
}
