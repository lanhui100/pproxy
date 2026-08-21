# T2 — Token 模块（含 T7 admin 引导）

> 依赖: T1 | 并行波次 2（与 T4/T5 并行，仅新增 token.rs） | 上游: M1 spec §3.1 / §3.2

## 1. 目标

实现 token 全生命周期：生成（数据 token + admin token）、SHA-256 哈希、校验（未撤销/未过期）、CRUD、进程内缓存（启动全量 + 变更刷新）、last_used_at 节流写库、admin token 首启引导（T7 归并于此，见第 8 节）。

## 2. 文件位置

- 新增 `crates/core/src/token.rs`；`lib.rs` 追加 `pub mod token;`（仅此一行，不碰其他模块）。
- `crates/core/Cargo.toml`：无新增依赖（sha2/rand/hex 已由 T1 加入）。

## 3. 数据结构与公开接口（实现不得偏离）

```rust
// crates/core/src/token.rs
use crate::store::{Store, TokenRow, StoreError, RevokeOutcome, now_unix};

pub const TOKEN_PREFIX: &str = "pony_";
pub const ADMIN_PREFIX: &str = "pony_admin_";
/// last_used_at 写库节流窗口（节流判断一律引用此常量，禁止字面量 60）
pub const LAST_USED_THROTTLE_SECS: u64 = 60;
/// token name 白名单（S-P2-3）：字母/数字/点/下划线/连字符，1-64 字符
pub const NAME_PATTERN: &str = "^[a-zA-Z0-9._-]{1,64}$";

#[derive(Debug)]
pub enum TokenError {
    NotFound,
    Revoked,
    Expired,
    Duplicate,          // name 重复（创建时，唯一索引冲突）
    InvalidName,        // name 不匹配 NAME_PATTERN 白名单（S-P2-3）
    InvalidExpiry,      // expires_days 超出 1..=3650（S-P2-4）
    Store(StoreError),
}
impl Display + Error + From<StoreError>;

pub struct TokenService { ... }   // 内部字段见 §5

impl TokenService {
    /// 打开 Store、全量加载缓存、执行 admin 引导（§8，含 PPROXY_ADMIN_TOKEN 注入）
    pub fn new(store: Arc<Store>) -> Result<Self, TokenError>;

    /// 生成明文 token（pony_<32hex>，rand 16 字节 → hex）
    pub fn generate_token(&self) -> String;

    /// 生成 admin 明文 token（pony_admin_<48hex>，rand 24 字节 → hex）
    pub fn generate_admin_token(&self) -> String;

    pub fn sha256_hex(&self, token: &str) -> String;   // 64 小写 hex；内部用，pub 便于测试

    /// 创建数据 token；返回 (id, 明文)。明文仅在此时存在，永不落库/打日志。
    /// name 必须匹配 NAME_PATTERN 白名单且 != "__admin__"；
    /// expires_days 提供时必须在 1..=3650，过期时刻用 checked_add 计算（溢出 → InvalidExpiry）。
    pub fn create_token(&self, name: &str, expires_days: Option<u64>)
        -> Result<(i64, String), TokenError>;

    pub fn list_tokens(&self) -> Result<Vec<TokenRow>, TokenError>;

    /// 撤销（软删 revoked_at=now）；成功后刷新缓存（失败 fail-closed，见 §5.1）。
    /// 三态透传 Store 的 RevokeOutcome（C-P1-4）。
    pub fn revoke_token(&self, id: i64) -> Result<RevokeOutcome, TokenError>;

    /// 数据面校验入口：哈希比对 + 未撤销 + 未过期 + admin 隔离（S-P2-1）。
    /// 通过 → 返回 TokenRow 并处理 last_used_at 节流；失败 → 对应错误变体。
    pub fn verify(&self, plaintext: &str) -> Result<TokenRow, TokenError>;

    /// admin token 校验（供 T6 管理中间件）：同 verify，但仅匹配
    /// name == "__admin__" 的行；数据 token 不能过 admin 校验。
    pub fn verify_admin(&self, plaintext: &str) -> Result<TokenRow, TokenError>;

    /// admin token 是否已存在（引导判断用）
    pub fn admin_exists(&self) -> bool;
}
```

### 3.1 校验语义（精确）

`verify(plaintext)` 步骤：
1. `sha256_hex(plaintext)` → 缓存 HashMap 查找；未命中 → `Err(NotFound)`（缓存 miss 不回库——缓存即全量，见 §5）。
2. `revoked_at.is_some()` → `Err(Revoked)`。
3. `expires_at < now_unix()`（严格小于；`expires_at == now` 视为未过期）→ `Err(Expired)`。
4. **admin 隔离（S-P2-1）**：`row.name == "__admin__"` → `Err(NotFound)`（admin token 不得作数据 token 使用；与 verify_admin 的 NotFound 同文案，不泄露区分信息）。
5. 通过 → 节流更新 last_used_at（§6）→ `Ok(row)`。

`verify_admin(plaintext)`：先走步骤 1-3（**不含**步骤 4），再要求 `row.name == "__admin__"`，否则 `Err(NotFound)`（不泄露"这是数据 token"信息）。

**竞态窗口一致性说明（S-P2-额外）**：verify 读缓存、revoke 写库后重载缓存，两操作并发时存在"verify 已过校验、撤销尚未对其生效"的窗口（重载完成前，在途请求仍可通过）。这是**最终一致**级别：窗口上限 = 一次 `reload_cache()`（全量 list_tokens，<100 行，毫秒级）。对个人网关可接受，不做每请求回库强一致（代价不匹配威胁模型：撤销的防护目标是"此后不再可用"，非"中断在途请求"）。注释写入代码。

### 3.2 生成格式（精确）

- 数据 token：`pony_` + 16 字节 `rand::thread_rng().fill_bytes` → `hex::encode` = 32 hex 字符，总长 37。
- admin token：`pony_admin_` + 24 字节 → 48 hex，总长 60。
- 哈希输入为完整明文（含前缀），SHA-256 输出小写 hex 落库。

## 4. 依赖任务

T1（Store、TokenRow、StoreError、now_unix）。

## 5. 缓存与一致性（裁决 #3）

```rust
pub struct TokenService {
    store: Arc<Store>,
    cache: RwLock<HashMap<String, TokenRow>>,  // key = token_hash（小写 hex）
    last_used: Mutex<HashMap<i64, u64>>,       // token_id -> 上次写库的 ts（节流用）
}
```

**一致性边界（单一写路径）**：所有对 tokens 表的写操作（create / revoke / admin 引导插入 / touch）**只发生在 TokenService 方法内**，api.rs 与 gateway 一律经由 TokenService，禁止绕过直写 Store。每个写方法在写库成功后**同步**重载缓存（`reload_cache()`：`store.list_tokens()` 全量重建 HashMap）。个人网关 token 量级 <100，全量重载成本可忽略，换取无失效竞态。

- Store 方法为同步签名（README §3.4，C-P1-9），**TokenService 公开方法同样全同步**（§3 签名为准）。阻塞规避由调用方负责：T6 handler 对 create/revoke/list 等**触库**方法经 `tokio::task::spawn_blocking` 包裹；数据面 verify 热路径只读内存缓存，不触库、无需包裹。
- 读路径（verify）**只读缓存，不回库**：缓存即权威快照。DB 与缓存不一致窗口 = 写库事务内（重载在同一方法内完成，方法返回时必然一致）。
- `last_used_at` 是唯一允许"缓存落后于库"的字段（节流写库，不回写缓存——缓存中该字段仅展示用途，允许陈旧）。

### 5.1 revoke 后 reload 失败 → fail-closed（S-P2-5）

`revoke_token` 写库成功但 `reload_cache()` 失败（StoreError）时，**不得**返回成功：缓存中已撤销行仍可见，数据面会继续放行。处理：返回 `Err(TokenError::Store(e))`（T6 映射 500），并 `tracing::error!`；下次任意写操作或重启自愈。原则：**缓存与库不一致时，宁可管理面报错，不可让已撤销 token 继续可用**。

## 6. last_used_at 节流写库

`verify` 通过后：
1. 锁 `last_used`，取 `prev = last_used.get(id)`。
2. `prev.is_none() || now - prev >= LAST_USED_THROTTLE_SECS` → `store.touch_token(id, now)`（同步调用；verify 本身在调用方的 spawn_blocking 上下文中执行时自然不阻塞 runtime——数据面 accept 后的连接处理若为纯 async 上下文，touch 经 `spawn_blocking` 包裹，实现者二选一并在代码注释注明），更新 map；否则跳过（C-P2-13：窗口一律引用常量，禁止字面量）。
3. 写库失败仅 `tracing::warn!`，**不影响校验结果**（观测字段，非安全字段）。

## 7. 单元测试清单

1. `generate_token` 格式：`pony_` 前缀 + 32 hex（正则断言），100 次生成无重复。
2. `generate_admin_token` 格式：`pony_admin_` + 48 hex。
3. `sha256_hex("pony_abc")` 与 `sha2` 手工计算一致（固定向量）。
4. create → verify 往返：正确明文 Ok；错误明文 NotFound。
5. revoke 后 verify → Revoked；`revoke_token` 二次调用返回 `Ok(AlreadyRevoked)`（C-P1-4 三态）。
6. 过期：`create_token` 后直接 `store` 层把 expires_at 改为 now-1（测试内经 Store 修改 + `reload`，或 create 后手工构造行插入缓存——实现者选择最小侵入方式，但断言必须覆盖 `expires_at == now` 不过期、`now-1` 过期两个边界）。
7. name 重复创建（未撤销）→ Duplicate（部分唯一索引冲突，C-P1-10）；**撤销后同名可复用**：revoke 旧 token 后同 name create 成功。
8. verify_admin：admin token Ok；数据 token → NotFound。
9. **verify 隔离 admin（S-P2-1）**：用 admin 明文走 `verify()` → NotFound（数据面不认 admin token）。
10. name 白名单（S-P2-3）：`dev-1.key_2` 通过；`pony_x`（保留前缀）→ InvalidName；`__admin__` → InvalidName（保留名）；空串、65 字符、含 `/`、含空格、含中文 → InvalidName。
11. expires_days（S-P2-4）：`Some(0)` → InvalidExpiry；`Some(3651)` → InvalidExpiry；`Some(3650)` 通过且 expires_at = now + 3650d（±60s）；`Some(u64::MAX)`（checked_add 溢出路径）→ InvalidExpiry。
12. 节流：同一 token 1 秒内 verify 两次，`touch_token` 仅一次（用带注入时钟的测试构造或检查 store 内 last_used_at 未变；允许测试通过 `last_used` map 断言；窗口值断言引用 `LAST_USED_THROTTLE_SECS`）。
13. 缓存刷新：create 后新 token 立即可 verify（无需重启）。
14. fail-closed（S-P2-5）：revoke 写库成功后模拟 reload 失败（临时 DB 删除或注入错误 Store）→ `revoke_token` 返回 Err 而非 Ok。

## 8. T7 归并 — admin token 首启引导

归属裁决：**T7 不产生独立代码文件，引导逻辑实现在 `TokenService::new` 内，独立 spec 文件 T7-bootstrap.md 仅记录行为契约与验收点**（避免为 20 行代码单开任务）。

行为契约（`TokenService::new` 内执行）：
1. `store.list_tokens()` 中存在 `name == "__admin__"` 且未撤销的行 → 跳过（重启不重复生成、不重复打印）。
2. 不存在时**先查注入（S-P1-3）**：读环境变量 `PPROXY_ADMIN_TOKEN`——已设置且非空 → 跳过生成，直接以该明文的 SHA-256 哈希 `insert_token(name="__admin__", hash, expires_at=None)`（**不打印明文**——用户已知自己的 token），刷新缓存。注入值不校验前缀格式（用户可用任意字符串，哈希落库即用）。
3. 未注入 → `generate_admin_token()` → `store.insert_token(name="__admin__", hash, expires_at=None)`（C-P1-3：insert_token 无 is_admin 参数，admin 语义仅由 name 约定）→ 刷新缓存 → **`tracing::warn!` 打印明文一次**：

```
ADMIN_TOKEN (仅此一次，请立即保存): pony_admin_<48hex>
```

4. 用 warn 级别理由：systemd journal 中可 `journalctl -u pproxy | grep ADMIN_TOKEN` 检索，且不会被 info 级日志淹没。**运维提示（归档时写入 API.md）**：journald 持久化场景清理该明文用 `journalctl --vacuum-time=1s` 前先导出，或直接采用 `PPROXY_ADMIN_TOKEN` 注入方式规避打印（systemd unit `Environment=` / `EnvironmentFile=`）。

**验收点（并入 T8 集成测试第 1 步）**：全新 DB 首启日志出现 ADMIN_TOKEN；重启后日志不再出现。`PPROXY_ADMIN_TOKEN` 注入时（T8 可选步骤）：日志无 ADMIN_TOKEN 且注入值可过管理面鉴权。

## 9. 验收标准

- `cargo test -p pproxy-core token::` 全绿（第 7 节 14 项）。
- 明文 token 不出现在任何日志/DB/错误信息中（代码审查项：grep 确认 generate 结果仅经 create_token 返回值与 §8 打印路径流出）。
- lib.rs 改动仅一行 `pub mod token;`。
