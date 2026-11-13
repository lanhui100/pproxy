//! 订阅短码存储：把长订阅 URL（`/clash?token=usr_live_...`，约 390 字节）压缩成
//! 8 字符短码，让终端二维码从版本 13（约 71 列 × 35 行）降到版本 3（约 37 列 × 19 行）。
//!
//! 动机与取舍见决策 `2026-11-13-subscription-short-code-for-qr`：
//! - 终端 Unicode 二维码的宽度 = 模块数，模块数只由负载长度决定，
//!   因此"把二维码缩小"唯一的物理手段就是**缩短负载**；
//! - 短码映射必须落在网关侧（客户端拿到的仍是可被 `/clash` 消费的 URL）；
//! - 短码与令牌等权（都是 bearer 凭据），因此吊销必须级联、过期必须随令牌过期。
//!
//! 存储口径与撤销表/用量账本一致：`$HOME/.pony/short_links.json`（可用
//! `GATE_SHORTLINK_FILE` 覆盖），原子写（临时文件 + rename），进程内加锁。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 短码字母表：去掉易混字符（0/o、1/i/l、u），30 个符号。
/// 8 位短码空间约 30^8 ≈ 6.5e11（≈2^39），线上盲猜不可行。
const CODE_ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";

/// 短码长度。
pub const CODE_LEN: usize = 8;

/// 短码条目。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShortLinkEntry {
    /// 原始订阅令牌（`usr_live_...`）：`/clash` 用它合成 YAML 与订阅头。
    pub token: String,
    /// 租户标识（令牌 `sub`）：撤销时按 uid 级联。
    pub uid: String,
    /// 令牌 `jti`：撤销时按 jti 精确级联。
    pub jti: String,
    /// 令牌到期时间（unix 秒）：短码不得比它活得更久。
    pub exp: u64,
}

/// 短码存储（进程内表 + JSON 持久化）。
#[derive(Debug)]
pub struct ShortLinkStore {
    /// `None` 表示纯内存（单测用），不落盘。
    path: Option<PathBuf>,
    map: Mutex<BTreeMap<String, ShortLinkEntry>>,
}

impl ShortLinkStore {
    /// 默认路径：`GATE_SHORTLINK_FILE` → `$HOME/.pony/short_links.json`。
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("GATE_SHORTLINK_FILE") {
            let p = p.trim();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        let home = std::env::var("HOME").ok()?;
        Some(Path::new(&home).join(".pony").join("short_links.json"))
    }

    /// 打开（或初始化）存储：文件缺失、损坏一律视为空表，绝不因历史文件阻塞网关启动。
    pub fn open(path: Option<PathBuf>) -> Self {
        let map = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<BTreeMap<String, ShortLinkEntry>>(&text).ok())
            .unwrap_or_default();
        let n = map.len();
        if let Some(p) = path.as_deref() {
            tracing::info!(loaded = n, path = %p.display(), "short link store loaded");
        }
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    /// 纯内存存储（单测与不落盘场景）。
    pub fn in_memory() -> Self {
        Self {
            path: None,
            map: Mutex::new(BTreeMap::new()),
        }
    }

    /// 签发短码：同一令牌（uid + jti）幂等复用既有短码，避免重复开通产生多份凭据。
    pub fn issue(&self, token: &str, uid: &str, jti: &str, exp: u64) -> Result<String, String> {
        let now = now_unix();
        let mut map = self.map.lock().map_err(|_| "short link lock poisoned".to_string())?;
        if let Some((code, _)) = map
            .iter()
            .find(|(_, e)| e.jti == jti && e.uid == uid && e.exp > now)
        {
            return Ok(code.clone());
        }
        for _ in 0..8 {
            let code = random_code();
            if map.contains_key(&code) {
                continue;
            }
            map.insert(
                code.clone(),
                ShortLinkEntry {
                    token: token.to_string(),
                    uid: uid.to_string(),
                    jti: jti.to_string(),
                    exp,
                },
            );
            self.persist_locked(&map);
            return Ok(code);
        }
        Err("无法分配唯一短码".to_string())
    }

    /// 解析短码：过期条目顺手清理（短码不得比令牌活得久）。
    pub fn resolve(&self, code: &str) -> Option<String> {
        let now = now_unix();
        let mut map = self.map.lock().ok()?;
        let before = map.len();
        map.retain(|_, e| e.exp > now);
        if map.len() != before {
            self.persist_locked(&map);
        }
        map.get(code).map(|e| e.token.clone())
    }

    /// 撤销级联：按 jti 或 uid 命中即删除。返回删除条数。
    pub fn revoke(&self, identifier: &str) -> usize {
        let mut map = match self.map.lock() {
            Ok(m) => m,
            Err(_) => return 0,
        };
        let before = map.len();
        map.retain(|_, e| e.uid != identifier && e.jti != identifier);
        let removed = before - map.len();
        if removed > 0 {
            self.persist_locked(&map);
        }
        removed
    }

    /// 当前条目数（观测/测试用）。
    pub fn len(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 落盘（调用方已持锁）：临时文件 + rename，避免半截 JSON。
    fn persist_locked(&self, map: &BTreeMap<String, ShortLinkEntry>) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(map) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_err() {
            tracing::warn!(path = %path.display(), "short link store rename failed");
        }
    }
}

/// 判断形如短码的字符串（长度 + 字母表），用于和 `usr_live_...` 令牌区分。
pub fn looks_like_code(s: &str) -> bool {
    s.len() == CODE_LEN && s.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

/// 生成随机短码（`/dev/urandom` 取熵；读取失败退化为时间 + 计数器）。
fn random_code() -> String {
    use std::io::Read;
    let mut buf = [0u8; CODE_LEN];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !ok {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut x = nanos ^ (std::process::id() as u64).rotate_left(17);
        for slot in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *slot = x as u8;
        }
    }
    buf.iter()
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect()
}

/// 当前 unix 秒。
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_and_resolve_roundtrip() {
        let store = ShortLinkStore::in_memory();
        let code = store.issue("usr_live_abc", "usr_alice", "tok-1", now_unix() + 3600).unwrap();
        assert!(looks_like_code(&code), "短码形态不符: {code}");
        assert_eq!(store.resolve(&code).as_deref(), Some("usr_live_abc"));
    }

    #[test]
    fn issue_is_idempotent_per_jti() {
        let store = ShortLinkStore::in_memory();
        let exp = now_unix() + 3600;
        let a = store.issue("t", "usr_a", "tok-1", exp).unwrap();
        let b = store.issue("t", "usr_a", "tok-1", exp).unwrap();
        assert_eq!(a, b);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn expired_code_is_unresolvable_and_purged() {
        let store = ShortLinkStore::in_memory();
        let code = store.issue("t", "usr_a", "tok-1", now_unix() - 1).unwrap();
        assert!(store.resolve(&code).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn revoke_by_jti_and_uid() {
        let store = ShortLinkStore::in_memory();
        let exp = now_unix() + 3600;
        let a = store.issue("t1", "usr_a", "tok-1", exp).unwrap();
        let b = store.issue("t2", "usr_b", "tok-2", exp).unwrap();
        assert_eq!(store.revoke("tok-1"), 1);
        assert!(store.resolve(&a).is_none());
        assert_eq!(store.revoke("usr_b"), 1);
        assert!(store.resolve(&b).is_none());
        assert_eq!(store.revoke("usr_b"), 0);
    }

    #[test]
    fn non_code_strings_are_rejected() {
        assert!(!looks_like_code("usr_live_abcdefgh"));
        assert!(!looks_like_code("abcd"));
        assert!(!looks_like_code("abcdefgO"), "含易混字符 O 不应视为短码");
    }

    #[test]
    fn persisted_store_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short_links.json");
        let code = {
            let store = ShortLinkStore::open(Some(path.clone()));
            store.issue("usr_live_x", "usr_x", "tok-x", now_unix() + 60).unwrap()
        };
        let reopened = ShortLinkStore::open(Some(path));
        assert_eq!(reopened.resolve(&code).as_deref(), Some("usr_live_x"));
    }
}
