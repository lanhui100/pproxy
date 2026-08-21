//! config.json 旧格式（含 routes 键）→ routes 表 一次性迁移（T1 §6.1）。
//!
//! WHY main.rs 必须先读完整 config 构造 EdgeClient 再调本方法：
//! 迁移会重写 config.json（仅删 routes 键），先读后写规避读写竞态（T1 §6.1.8）。

use std::path::Path;

use rusqlite::params;
use serde_json::Value;

use super::{now_unix, MigrationOutcome, Store, StoreError};

/// F8：迁移导入的 route_upstreams 绑定值白名单——"worker"|"vercel" 或
/// 任意非空、无空白/控制字符的上游名（与 config upstreams 键同域）。
/// 迁移期无法访问 edges 表（构造先于迁移），故做格式校验而非存在性校验；
/// 未知名在转发期由 gateway "upstream not configured" 兜底。
fn validate_upstream_binding(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Store {
    pub fn migrate_config_if_needed(&self, config_path: &Path)
        -> Result<MigrationOutcome, StoreError>
    {
        // 幂等保障先于 config 检查：config 已重写（无 routes 键）时
        // list_routes 非空仍须返回 AlreadyMigrated 而非 NoLegacyRoutes（T1 §6.1.4）
        if !self.list_routes()?.is_empty() {
            return Ok(MigrationOutcome::AlreadyMigrated);
        }
        let Ok(raw) = std::fs::read_to_string(config_path) else {
            // config 不存在 → 无旧格式可迁（T1 §6.1.1）
            return Ok(MigrationOutcome::NoLegacyRoutes);
        };
        let cfg: Value = serde_json::from_str(&raw)?;
        let Some(routes) = cfg.get("routes").and_then(Value::as_object) else {
            return Ok(MigrationOutcome::NoLegacyRoutes);
        };
        if routes.is_empty() {
            return Ok(MigrationOutcome::NoLegacyRoutes);
        }
        backup_config(config_path, &raw)?;
        let count = self.import_routes(routes, &cfg)?;
        rewrite_config(config_path, &cfg, &self.db_path_str())?;
        Ok(MigrationOutcome::Imported(count))
    }

    /// 事务内逐条导入；upstream 取 route_upstreams 映射（无则 NULL）。
    fn import_routes(
        &self,
        routes: &serde_json::Map<String, Value>,
        cfg: &Value,
    ) -> Result<usize, StoreError> {
        let route_upstreams: std::collections::HashMap<String, String> = cfg
            .get("route_upstreams")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut count = 0usize;
        let mut skipped = 0usize;
        for (name, target) in routes {
            let Some(target_host) = target.as_str() else {
                tracing::warn!(route = %name, "migration skip: target_host not a string");
                skipped += 1;
                continue;
            };
            // F4：旧 config 不经管理 API，入库前过与 create_route 相同的
            // name/host 校验；非法项跳过（迁移不因单条脏数据整体失败）。
            if crate::route::validate_name(name).is_err()
                || crate::route::validate_host(target_host).is_err()
            {
                tracing::warn!(route = %name, host_kind = "invalid", "migration skip: validation failed");
                skipped += 1;
                continue;
            }
            // F8：route_upstreams 绑定写入 override_upstream 列（resolve 的
            // 实际读取列）；upstream 列留 NULL（创建时自动选择快照，迁移无快照）。
            // 绑定值经 validate_upstream_binding 白名单校验，非法绑定忽略
            // （回退 host 规则，不阻断迁移）。
            let binding = route_upstreams.get(name).filter(|b| validate_upstream_binding(b));
            tx.execute(
                "INSERT INTO routes (name, target_host, upstream, override_upstream, enabled, created_at)
                 VALUES (?1, ?2, NULL, ?3, 1, ?4)",
                params![
                    name,
                    target_host,
                    binding,
                    now_unix() as i64
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        if skipped > 0 {
            tracing::warn!(skipped, "migration skipped invalid legacy routes");
        }
        Ok(count)
    }
}

/// 备份：config.json → config.json.bak；已存在则不覆盖（.bak 永远是原始文件）。
fn backup_config(config_path: &Path, raw: &str) -> Result<(), StoreError> {
    let bak = bak_path(config_path);
    if bak.exists() {
        return Ok(());
    }
    std::fs::write(&bak, raw)?;
    Ok(())
}

/// 原子重写：写 .tmp 再 rename；仅删顶层 routes 键，其余键原样保留，
/// 新增 db_path 字段记录实际 DB 路径（运维排查用，T1 §6.1.7）。
fn rewrite_config(config_path: &Path, cfg: &Value, db_path: &str) -> Result<(), StoreError> {
    let mut out = cfg.clone();
    if let Value::Object(ref mut m) = out {
        m.remove("routes");
        m.insert("db_path".into(), Value::String(db_path.to_string()));
    }
    let tmp = config_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&out)? + "\n")?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

fn bak_path(config_path: &Path) -> std::path::PathBuf {
    // config.json → config.json.bak（非 with_extension，避免吃掉多段扩展名歧义）
    let mut s = config_path.as_os_str().to_os_string();
    s.push(".bak");
    std::path::PathBuf::from(s)
}
