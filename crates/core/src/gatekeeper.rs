//! 认证门禁与防爆破限流器（AuthGatekeeper）。
//!
//! 防御红队 High-01 隐患：Basic Auth 无状态接口容易遭受字典爆破与 CPU 耗尽 DoS 攻击。
//! 对连续认证失败的 IP 施加内存级滑动窗口惩罚与指数退避锁定。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 门禁配置。
#[derive(Debug, Clone)]
pub struct GatekeeperConfig {
    /// 触发锁定的最大连续失败次数（默认 5 次）
    pub max_failures: u32,
    /// 初始锁定时长（默认 300 秒 = 5 分钟）
    pub base_lockout: Duration,
    /// 最大指数退避倍数
    pub max_backoff_shift: u32,
}

impl Default for GatekeeperConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            base_lockout: Duration::from_secs(300),
            max_backoff_shift: 4, // 最大 300 * 16 = 4800 秒 (80分钟)
        }
    }
}

#[derive(Debug)]
struct FailRecord {
    count: u32,
    lockout_until: Option<Instant>,
    last_attempt: Instant,
}

/// 内存级 IP 认证防爆破限流器。
pub struct AuthGatekeeper {
    config: GatekeeperConfig,
    records: Mutex<HashMap<IpAddr, FailRecord>>,
}

impl AuthGatekeeper {
    pub fn new(config: GatekeeperConfig) -> Self {
        Self {
            config,
            records: Mutex::new(HashMap::new()),
        }
    }

    /// 检查该 IP 当前是否处于锁定状态。
    pub fn check(&self, ip: &IpAddr) -> Result<(), &'static str> {
        let mut map = self.records.lock().unwrap();
        let now = Instant::now();

        if let Some(record) = map.get(ip) {
            if let Some(until) = record.lockout_until {
                if now < until {
                    return Err("IP temporarily locked out due to repeated authentication failures");
                }
            }
        }
        // 如果已过锁定期但记录存在，清理锁定态
        if let Some(record) = map.get_mut(ip) {
            if record.lockout_until.is_some() && now >= record.lockout_until.unwrap() {
                record.lockout_until = None;
            }
        }
        Ok(())
    }

    /// 记录一次认证失败。
    pub fn record_failure(&self, ip: IpAddr) {
        let mut map = self.records.lock().unwrap();
        let now = Instant::now();
        let entry = map.entry(ip).or_insert_with(|| FailRecord {
            count: 0,
            lockout_until: None,
            last_attempt: now,
        });

        entry.count += 1;
        entry.last_attempt = now;

        if entry.count >= self.config.max_failures {
            let shift = (entry.count - self.config.max_failures).min(self.config.max_backoff_shift);
            let multiplier = 1u32 << shift;
            let duration = self.config.base_lockout * multiplier;
            entry.lockout_until = Some(now + duration);
            tracing::warn!(%ip, fails = entry.count, lockout_sec = duration.as_secs(), "AuthGatekeeper: IP locked out for brute-force defense");
        }
    }

    /// 记录一次认证成功，重置失败计数。
    pub fn record_success(&self, ip: &IpAddr) {
        let mut map = self.records.lock().unwrap();
        map.remove(ip);
    }

    /// 清理 1 小时前无活动的陈旧记录，防止内存泄漏。
    pub fn prune_stale(&self, max_idle: Duration) {
        let mut map = self.records.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, v| {
            if let Some(until) = v.lockout_until {
                if now < until {
                    return true;
                }
            }
            now.duration_since(v.last_attempt) < max_idle
        });
    }
}

impl Default for AuthGatekeeper {
    fn default() -> Self {
        Self::new(GatekeeperConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_requests_under_threshold() {
        let gk = AuthGatekeeper::new(GatekeeperConfig {
            max_failures: 3,
            base_lockout: Duration::from_secs(60),
            max_backoff_shift: 2,
        });
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

        assert!(gk.check(&ip).is_ok());
        gk.record_failure(ip);
        assert!(gk.check(&ip).is_ok());
        gk.record_failure(ip);
        assert!(gk.check(&ip).is_ok());
    }

    #[test]
    fn locks_out_on_reaching_threshold() {
        let gk = AuthGatekeeper::new(GatekeeperConfig {
            max_failures: 3,
            base_lockout: Duration::from_secs(60),
            max_backoff_shift: 2,
        });
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 101));

        gk.record_failure(ip);
        gk.record_failure(ip);
        gk.record_failure(ip); // 第 3 次失败，触发锁定

        assert!(gk.check(&ip).is_err());
    }

    #[test]
    fn success_clears_failure_count() {
        let gk = AuthGatekeeper::new(GatekeeperConfig {
            max_failures: 3,
            base_lockout: Duration::from_secs(60),
            max_backoff_shift: 2,
        });
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 102));

        gk.record_failure(ip);
        gk.record_failure(ip);
        assert!(gk.check(&ip).is_ok());

        gk.record_success(&ip);

        // 再次失败 2 次依然不锁定（计数已清零）
        gk.record_failure(ip);
        gk.record_failure(ip);
        assert!(gk.check(&ip).is_ok());
    }
}
