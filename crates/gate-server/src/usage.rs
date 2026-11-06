use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// VPS 默认每月总流量基线（500GB）
pub const VPS_TOTAL_QUOTA_BYTES: u64 = 500 * 1024 * 1024 * 1024; // 536,870,912,000 bytes

/// 计算指定时间戳（秒）按每月指定 billing_day 划定的账单周期。
/// 返回 (cycle_key, next_reset_ts, days_left)。
/// cycle_key 格式为 "YYYY-MM"，代表该账单所属起始年和月。
pub fn compute_billing_cycle(now_ts: u64, billing_day: u32) -> (String, u64, u64) {
    let b_day = billing_day.clamp(1, 28);
    let (year, month, day) = civil_from_days((now_ts / 86_400) as i64);

    // 账单周期的起始年月与结束年月
    let (start_y, start_m, end_y, end_m) = if day >= b_day {
        let (next_y, next_m) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        (year, month, next_y, next_m)
    } else {
        let (prev_y, prev_m) = if month == 1 {
            (year - 1, 12)
        } else {
            (year, month - 1)
        };
        (prev_y, prev_m, year, month)
    };

    let cycle_key = format!("{start_y:04}-{start_m:02}");
    let reset_days = days_from_civil(end_y, end_m, b_day);
    let next_reset_ts = (reset_days as u64) * 86_400;
    let days_left = if next_reset_ts > now_ts {
        (next_reset_ts - now_ts) / 86_400
    } else {
        0
    };

    (cycle_key, next_reset_ts, days_left)
}

/// 天数计算算法（Howard Hinnant 逆向与正向算法）
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageRecord {
    pub cycle_key: String,
    pub billing_day: u32,
    #[serde(default)]
    pub users: BTreeMap<String, u64>,
    #[serde(default)]
    pub updated_at: u64,
}

pub struct UsageManager {
    path: PathBuf,
    billing_day: u32,
    current_cycle: RwLock<String>,
    map: Arc<DashMap<String, AtomicU64>>,
    /// 最近一次成功落盘的账本快照（用于跳过无变化的周期性刷盘）
    last_persisted: RwLock<String>,
}

impl UsageManager {
    pub fn new(path: PathBuf, billing_day: u32, map: Arc<DashMap<String, AtomicU64>>) -> Self {
        let (cycle, _, _) = compute_billing_cycle(now_secs(), billing_day);
        Self {
            path,
            billing_day,
            current_cycle: RwLock::new(cycle),
            map,
            last_persisted: RwLock::new(String::new()),
        }
    }

    pub fn billing_day(&self) -> u32 {
        self.billing_day
    }

    pub fn current_cycle(&self) -> String {
        self.current_cycle.read().unwrap().clone()
    }

    /// 汇总当前周期内全量用户累计消耗字节数
    pub fn total_used_bytes(&self) -> u64 {
        let mut sum: u64 = 0;
        for item in self.map.iter() {
            sum = sum.saturating_add(item.value().load(Ordering::Relaxed));
        }
        sum
    }

    /// 从磁盘加载持久化用量。若磁盘记录周期与当前账单周期不匹配，自动归档并开启新周期。
    pub fn load_or_init(&self) {
        let (expected_cycle, _, _) = compute_billing_cycle(now_secs(), self.billing_day);
        *self.current_cycle.write().unwrap() = expected_cycle.clone();

        if !self.path.exists() {
            tracing::info!(path = %self.path.display(), cycle = %expected_cycle, "gate_usage.json does not exist, initializing empty");
            self.persist();
            return;
        }

        match std::fs::read_to_string(&self.path) {
            Ok(content) => match serde_json::from_str::<UsageRecord>(&content) {
                Ok(rec) => {
                    if rec.cycle_key == expected_cycle {
                        for (uid, bytes) in rec.users {
                            self.map.insert(uid, AtomicU64::new(bytes));
                        }
                        tracing::info!(path = %self.path.display(), cycle = %expected_cycle, users = self.map.len(), "restored usage from disk");
                    } else {
                        tracing::info!(old_cycle = %rec.cycle_key, new_cycle = %expected_cycle, "billing cycle rollover detected, archiving previous usage");
                        self.archive_old_cycle(&rec);
                        self.map.clear();
                        self.persist();
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse gate_usage.json, starting with fresh cycle");
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "failed to read gate_usage.json");
            }
        }
    }

    /// 检查并按需推进账单周期（跨月/跨账单日时自动归零当前周期并归档）
    pub fn check_and_rollover(&self) {
        let (expected_cycle, _, _) = compute_billing_cycle(now_secs(), self.billing_day);
        if self.current_cycle() == expected_cycle {
            return;
        }
        let mut w = self.current_cycle.write().unwrap();
        if *w == expected_cycle {
            return;
        }
        tracing::info!(old_cycle = %*w, new_cycle = %expected_cycle, "rolling over billing cycle");
        let rec = UsageRecord {
            cycle_key: w.clone(),
            billing_day: self.billing_day,
            users: self.snapshot_users(),
            updated_at: now_secs(),
        };
        self.archive_old_cycle(&rec);
        *w = expected_cycle;
        drop(w);
        self.subtract_archived(&rec.users);
        self.persist();
    }

    /// 从现网水位扣除已归档部分：归档快照之后新流入的字节保留到新周期，避免跨周期窗口丢账。
    /// 扣到 0 的条目随即移除（且仅在仍为 0 时移除），否则账本会随周期数堆积空用户。
    fn subtract_archived(&self, users: &BTreeMap<String, u64>) {
        for (uid, archived) in users {
            let should_prune = match self.map.get(uid) {
                Some(entry) => {
                    entry
                        .value()
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                            Some(cur.saturating_sub(*archived))
                        })
                        .is_ok()
                        && entry.value().load(Ordering::Relaxed) == 0
                }
                None => false,
            };
            if should_prune {
                self.map
                    .remove_if(uid, |_, v| v.load(Ordering::Relaxed) == 0);
            }
        }
    }

    /// 进程退出前调用：先检查账单周期滚动，再强制落盘（跳过 dirty-check）。
    pub fn flush_now(&self) {
        self.check_and_rollover();
        *self.last_persisted.write().unwrap() = String::new();
        self.persist();
    }

    fn snapshot_users(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        for item in self.map.iter() {
            out.insert(item.key().clone(), item.value().load(Ordering::Relaxed));
        }
        out
    }

    fn archive_old_cycle(&self, rec: &UsageRecord) {
        if let Some(parent) = self.path.parent() {
            let archive_name = format!("gate_usage_{}.json", rec.cycle_key);
            let archive_path = parent.join(archive_name);
            if let Ok(data) = serde_json::to_string_pretty(rec) {
                let _ = std::fs::write(&archive_path, data);
            }
        }
    }

    /// 将当前用量原子刷盘（临时文件写入 + rename）。账本无变化时跳过写入，避免空转磁盘 IO。
    pub fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let cycle = self.current_cycle.read().unwrap().clone();
        let rec = UsageRecord {
            cycle_key: cycle,
            billing_day: self.billing_day,
            users: self.snapshot_users(),
            updated_at: now_secs(),
        };

        if let Ok(data) = serde_json::to_string_pretty(&rec) {
            let fingerprint = format!(
                "{}|{}",
                rec.cycle_key,
                serde_json::to_string(&rec.users).unwrap_or_default()
            );
            if *self.last_persisted.read().unwrap() == fingerprint {
                return;
            }
            let tmp_path = self
                .path
                .with_extension(format!("tmp.{}", std::process::id()));
            if std::fs::write(&tmp_path, &data).is_ok()
                && std::fs::rename(&tmp_path, &self.path).is_ok()
            {
                *self.last_persisted.write().unwrap() = fingerprint;
            }
        }
    }

    /// 后台启动定期持久化任务（每 10 秒刷新并检查 rollover）
    pub fn spawn_periodic_sync(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                self.check_and_rollover();
                self.persist();
            }
        });
    }
}

/// 等待进程退出信号（Ctrl-C 或 SIGTERM），供服务端在主循环退出前落盘用量。
pub async fn wait_for_exit_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

pub fn default_usage_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    Path::new(&home).join(".pony").join("gate_usage.json")
}

pub fn get_billing_day() -> u32 {
    std::env::var("GATE_BILLING_DAY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_billing_cycle_on_first() {
        // 2026-11-05 00:00:00 UTC = 1793836800
        let ts = 1793836800; // 2026-11-05
        let (cycle, reset_ts, days_left) = compute_billing_cycle(ts, 1);
        assert_eq!(cycle, "2026-11");
        // next reset: 2026-12-01
        let (y, m, d) = civil_from_days((reset_ts / 86400) as i64);
        assert_eq!((y, m, d), (2026, 12, 1));
        assert_eq!(days_left, 26);
    }

    #[test]
    fn test_compute_billing_cycle_mid_month() {
        // 2026-11-05 UTC, billing day = 15
        // 当前在 15 号之前，属于 2026-10 周期，下次重置为 2026-11-15
        let ts = 1793836800; // 2026-11-05
        let (cycle, reset_ts, days_left) = compute_billing_cycle(ts, 15);
        assert_eq!(cycle, "2026-10");
        let (y, m, d) = civil_from_days((reset_ts / 86400) as i64);
        assert_eq!((y, m, d), (2026, 11, 15));
        assert_eq!(days_left, 10);
    }

    #[test]
    fn test_usage_manager_persistence_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate_usage.json");
        let map = Arc::new(DashMap::new());

        let mgr = UsageManager::new(path.clone(), 1, map.clone());
        mgr.load_or_init();

        map.insert("usr_admin".to_string(), AtomicU64::new(1024 * 1024));
        map.insert("usr_bob".to_string(), AtomicU64::new(2048));
        mgr.persist();

        // 模拟新进程启动恢复
        let map2 = Arc::new(DashMap::new());
        let mgr2 = UsageManager::new(path.clone(), 1, map2.clone());
        mgr2.load_or_init();

        assert_eq!(
            map2.get("usr_admin").unwrap().load(Ordering::Relaxed),
            1024 * 1024
        );
        assert_eq!(map2.get("usr_bob").unwrap().load(Ordering::Relaxed), 2048);
    }

    /// 跨账单周期启动：上一周期账本归档为 `gate_usage_<cycle>.json`，当前水位归零。
    #[test]
    fn test_load_archives_previous_cycle_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate_usage.json");
        let stale = UsageRecord {
            cycle_key: "2000-01".into(),
            billing_day: 1,
            users: BTreeMap::from([("usr_old".to_string(), 4096u64)]),
            updated_at: 0,
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();

        let map = Arc::new(DashMap::new());
        let mgr = UsageManager::new(path.clone(), 1, map.clone());
        mgr.load_or_init();

        assert_eq!(map.len(), 0, "跨账单周期必须归零当前水位");
        assert!(
            dir.path().join("gate_usage_2000-01.json").exists(),
            "上一周期账本必须归档"
        );
        let current: UsageRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_ne!(current.cycle_key, "2000-01", "落盘账本必须切到当前周期");
    }

    /// 跨周期滚动只扣除已归档水位，归档快照之后涌入的流量保留到新周期。
    #[test]
    fn test_rollover_subtraction_keeps_new_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let map = Arc::new(DashMap::new());
        let mgr = UsageManager::new(dir.path().join("gate_usage.json"), 1, map.clone());
        mgr.load_or_init();

        map.insert("usr_admin".to_string(), AtomicU64::new(1000));
        let archived = BTreeMap::from([("usr_admin".to_string(), 1000u64)]);
        // 快照之后又流入 500 字节
        map.get("usr_admin")
            .unwrap()
            .value()
            .fetch_add(500, Ordering::Relaxed);

        mgr.subtract_archived(&archived);

        assert_eq!(
            map.get("usr_admin").unwrap().load(Ordering::Relaxed),
            500,
            "新周期必须保留归档后新增流量"
        );
    }

    /// 归档后水位归零的用户必须从账本移除，否则每跨一次周期就沉淀一批 0 值条目。
    #[test]
    fn test_rollover_prunes_zeroed_users() {
        let dir = tempfile::tempdir().unwrap();
        let map = Arc::new(DashMap::new());
        let mgr = UsageManager::new(dir.path().join("gate_usage.json"), 1, map.clone());
        mgr.load_or_init();

        map.insert("usr_idle".to_string(), AtomicU64::new(4096));

        mgr.subtract_archived(&BTreeMap::from([("usr_idle".to_string(), 4096u64)]));

        assert!(
            map.get("usr_idle").is_none(),
            "归零用户必须从账本清理，落盘才是空用户表"
        );
        assert_eq!(mgr.total_used_bytes(), 0);
    }
}
