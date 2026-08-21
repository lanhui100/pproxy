//! config.json 旧格式（含 routes 键）→ routes 表 一次性迁移（T1 §6.1）。
//!
//! WHY main.rs 必须先读完整 config 构造 EdgeClient 再调本方法：
//! 迁移会重写 config.json（仅删 routes 键），先读后写规避读写竞态（T1 §6.1.8）。

use std::path::Path;

use rusqlite::params;
use serde_json::Value;

use super::{now_unix, MigrationOutcome, Store, StoreError};

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
        for (name, target) in routes {
            let Some(target_host) = target.as_str() else {
                return Err(StoreError::Migration(format!("route {name}: target_host 非字符串")));
            };
            tx.execute(
                "INSERT INTO routes (name, target_host, upstream, override_upstream, enabled, created_at)
                 VALUES (?1, ?2, ?3, NULL, 1, ?4)",
                params![
                    name,
                    target_host,
                    route_upstreams.get(name),
                    now_unix() as i64
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
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
