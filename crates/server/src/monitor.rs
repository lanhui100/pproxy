//! 监控轮询编排（M3 spec §2/§6）：tick → 采集 → 落库 → 评估 → 通知。
//!
//! 生命周期仿 main.rs 第 6 步 usage flush task：`tokio::spawn` + interval，
//! panic 不拖垮主服务。首 tick 立即采集且**不丢弃**（启动即采一轮，spec 隐含）。
//! 重启丢失内存态可能重复告警一次 = 已知行为（spec §6 登记）。
//!
//! WHY last_pct 内存态只在轮询任务内部使用：越线沿判定是单线程顺序语义，
//! 无跨任务共享需求；来源健康状态才需要经 MonitorHandle 暴露给 /api/quota。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pproxy_core::alert::{
    alert_message, evaluate_crossing, AlertRecord, WebhookChannel,
};
use pproxy_core::quota::{
    CfCollector, CollectError, QuotaSample, QuotaSourceState, VercelCollector,
    VercelUsageOutcome, UPSTREAM_CF, UPSTREAM_VERCEL,
};
use pproxy_core::store::{hour_floor, now_unix, QuotaSnapshotRow, Store};
use tracing::{info, warn};

/// 来源名（/api/quota sources 字段）。
pub const SOURCE_CF: &str = UPSTREAM_CF;
pub const SOURCE_VERCEL: &str = UPSTREAM_VERCEL;

/// 保留策略（M1 债务）：usage_hourly 30 天 / quota_snapshots 90 天。
const USAGE_RETENTION_SEC: u64 = 30 * 86_400;
const QUOTA_RETENTION_SEC: u64 = 90 * 86_400;

/// /api/quota sources 行。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStatus {
    pub name: String,
    pub state: QuotaSourceState,
    pub last_ok: Option<u64>,
}

/// 来源健康状态表：monitor 任务写、/api/quota 读。
pub struct MonitorHandle {
    inner: Mutex<HashMap<String, (QuotaSourceState, Option<u64>)>>,
    /// M5：/api/monitor/config 只读展示用白名单快照（仅两字段，
    /// WHY 不持有整个 MonitorConfig——其含 token/webhook 凭据，禁止出网）。
    threshold_pct: f64,
    poll_interval_sec: u64,
}

/// /api/monitor/config 响应体（手写白名单，spec §6.1：禁序列化 MonitorConfig）。
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct MonitorConfigDto {
    pub threshold_pct: f64,
    pub poll_interval_sec: u64,
}

impl MonitorHandle {
    pub fn config(&self) -> MonitorConfigDto {
        MonitorConfigDto {
            threshold_pct: self.threshold_pct,
            poll_interval_sec: self.poll_interval_sec,
        }
    }

    /// 初始注册全部已知来源：缺凭据 → Disabled；有凭据但尚无成功采样 →
    /// Error（语义为"尚未 ok"，首个立即 tick 内即被真实结果覆盖）。
    fn new(cfg: &MonitorConfig) -> Self {
        let (cf_enabled, vercel_enabled) = (cfg.cf_ready(), cfg.vercel_token.is_some());
        let mut m = HashMap::new();
        m.insert(
            SOURCE_CF.to_string(),
            (
                if cf_enabled { QuotaSourceState::Error } else { QuotaSourceState::Disabled },
                None,
            ),
        );
        m.insert(
            SOURCE_VERCEL.to_string(),
            (
                if vercel_enabled { QuotaSourceState::Error } else { QuotaSourceState::Disabled },
                None,
            ),
        );
        MonitorHandle {
            inner: Mutex::new(m),
            threshold_pct: cfg.threshold_pct,
            poll_interval_sec: cfg.poll_interval_sec,
        }
    }

    fn set_state(&self, source: &str, state: QuotaSourceState) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        m.entry(source.to_string())
            .and_modify(|(s, _)| *s = state)
            .or_insert((state, None));
    }

    fn set_ok(&self, source: &str, ts: u64) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        m.insert(source.to_string(), (QuotaSourceState::Ok, Some(ts)));
    }

    /// 快照（按来源名稳定排序，供 /api/quota 响应）。
    pub fn statuses(&self) -> Vec<SourceStatus> {
        let m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<SourceStatus> = m
            .iter()
            .map(|(name, (state, last_ok))| SourceStatus {
                name: name.clone(),
                state: *state,
                last_ok: *last_ok,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// 监控配置（全部环境变量，config.json 零改动，spec §3）。
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub poll_interval_sec: u64,
    pub threshold_pct: f64,
    pub cf_token: Option<String>,
    pub cf_account_tag: Option<String>,
    pub cf_graphql_url: Option<String>,
    pub vercel_token: Option<String>,
    pub vercel_team_id: Option<String>,
    pub vercel_api_base: Option<String>,
    pub webhook_url: Option<String>,
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

impl MonitorConfig {
    pub fn from_env() -> Self {
        let threshold_pct = env_opt("PPROXY_ALERT_THRESHOLD_PCT")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|t| (0.0..=100.0).contains(t))
            .unwrap_or(80.0);
        MonitorConfig {
            poll_interval_sec: env_opt("PPROXY_POLL_INTERVAL_SEC")
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v >= 1)
                .unwrap_or(3600),
            threshold_pct,
            cf_token: env_opt("PPROXY_CF_API_TOKEN"),
            cf_account_tag: env_opt("PPROXY_CF_ACCOUNT_TAG"),
            cf_graphql_url: env_opt("PPROXY_CF_GRAPHQL_URL"),
            vercel_token: env_opt("PPROXY_VERCEL_TOKEN"),
            vercel_team_id: env_opt("PPROXY_VERCEL_TEAM_ID"),
            vercel_api_base: env_opt("PPROXY_VERCEL_API_BASE"),
            webhook_url: env_opt("PPROXY_ALERT_WEBHOOK_URL"),
        }
    }

    /// 缺失来源 disabled：启动 info 一行非错误（spec §3）。
    pub fn log_disabled(&self) {
        if !self.cf_ready() {
            info!("monitor: cf source disabled (PPROXY_CF_API_TOKEN/PPROXY_CF_ACCOUNT_TAG missing)");
        }
        if self.vercel_token.is_none() {
            info!("monitor: vercel source disabled (PPROXY_VERCEL_TOKEN missing)");
        }
        if self.webhook_url.is_none() {
            info!("monitor: webhook notification disabled (PPROXY_ALERT_WEBHOOK_URL missing)");
        }
        info!(
            threshold_pct = self.threshold_pct,
            poll_interval_sec = self.poll_interval_sec,
            "monitor: polling configured"
        );
    }

    fn cf_ready(&self) -> bool {
        self.cf_token.is_some() && self.cf_account_tag.is_some()
    }
}

/// 装配入口：构建采集器与渠道，spawn 轮询任务，返回健康句柄给 AdminState。
pub fn spawn_monitor(store: Arc<Store>, cfg: MonitorConfig) -> Arc<MonitorHandle> {
    let handle = Arc::new(MonitorHandle::new(&cfg));
    let h = Arc::clone(&handle);
    tokio::spawn(async move {
        run_loop(store, cfg, h).await;
    });
    handle
}

async fn run_loop(store: Arc<Store>, cfg: MonitorConfig, handle: Arc<MonitorHandle>) {
    let cf = CfCollector::new(
        cfg.cf_token.clone(),
        cfg.cf_account_tag.clone(),
        cfg.cf_graphql_url.clone(),
    );
    let vercel = VercelCollector::new(
        cfg.vercel_token.clone(),
        cfg.vercel_team_id.clone(),
        cfg.vercel_api_base.clone(),
    );
    let webhook = WebhookChannel::new(cfg.webhook_url.clone());
    let mut last_pct: HashMap<(&'static str, &'static str), f64> = HashMap::new();
    let mut vercel_unsupported_warned = false;

    // 首 tick 立即触发（tokio interval 语义），结果照常落库不丢弃
    let mut interval = tokio::time::interval(Duration::from_secs(cfg.poll_interval_sec.max(1)));
    loop {
        interval.tick().await;
        tick(
            &store,
            &cfg,
            &handle,
            &cf,
            &vercel,
            &webhook,
            &mut last_pct,
            &mut vercel_unsupported_warned,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn tick(
    store: &Arc<Store>,
    cfg: &MonitorConfig,
    handle: &Arc<MonitorHandle>,
    cf: &CfCollector,
    vercel: &VercelCollector,
    webhook: &WebhookChannel,
    last_pct: &mut HashMap<(&'static str, &'static str), f64>,
    vercel_unsupported_warned: &mut bool,
) {
    let now = now_unix();
    let ts = hour_floor(now);

    // ---- CF 来源 ----
    if cf.enabled() {
        match cf.collect_daily_requests(now).await {
            Ok(sample) => match persist_sample(store, SOURCE_CF, "requests_daily", ts, &sample).await {
                Ok(()) => {
                    handle.set_ok(SOURCE_CF, now);
                    evaluate_and_notify(store, webhook, last_pct, cfg.threshold_pct, SOURCE_CF, "requests_daily", &sample, now).await;
                }
                Err(e) => {
                    warn!(error = %e, "monitor: cf snapshot persist failed");
                    handle.set_state(SOURCE_CF, QuotaSourceState::Error);
                }
            },
            Err(CollectError::Disabled) => {}
            Err(e) => {
                warn!(error = %e, "monitor: cf collect failed (retry next tick)");
                handle.set_state(SOURCE_CF, QuotaSourceState::Error);
            }
        }
    }

    // ---- Vercel 来源 ----
    if vercel.enabled() {
        match vercel.collect_usage(now).await {
            Ok(VercelUsageOutcome::UnsupportedPlan) => {
                // 能力探测降级：首 tick warn 一次后静默 skip（spec §4.2）
                handle.set_state(SOURCE_VERCEL, QuotaSourceState::UnsupportedPlan);
                if !*vercel_unsupported_warned {
                    warn!("monitor: vercel usage gated by plan (plan_upgrade_required); degraded to unsupported_plan, skipping silently");
                    *vercel_unsupported_warned = true;
                }
            }
            Ok(VercelUsageOutcome::Samples(samples)) => {
                let mut persist_err = None;
                for (metric, sample) in &samples {
                    if let Err(e) = persist_sample(store, SOURCE_VERCEL, metric, ts, sample).await {
                        persist_err = Some(e);
                        continue;
                    }
                    evaluate_and_notify(store, webhook, last_pct, cfg.threshold_pct, SOURCE_VERCEL, metric, sample, now).await;
                }
                match persist_err {
                    None => handle.set_ok(SOURCE_VERCEL, now),
                    Some(e) => {
                        warn!(error = %e, "monitor: vercel snapshot persist failed");
                        handle.set_state(SOURCE_VERCEL, QuotaSourceState::Error);
                    }
                }
            }
            Err(CollectError::Disabled) => {}
            Err(e) => {
                warn!(error = %e, "monitor: vercel collect failed (retry next tick)");
                handle.set_state(SOURCE_VERCEL, QuotaSourceState::Error);
            }
        }
    }

    // ---- 保留策略清理（M1 债务：每 tick 顺带执行，DELETE 低频廉价）----
    prune_retention(store, now).await;
}

async fn persist_sample(
    store: &Arc<Store>,
    upstream: &'static str,
    metric: &str,
    ts: u64,
    sample: &QuotaSample,
) -> Result<(), String> {
    let row = QuotaSnapshotRow {
        ts,
        upstream: upstream.to_string(),
        metric: metric.to_string(),
        used: sample.used,
        quota: sample.quota,
        pct: sample.pct,
    };
    let st = Arc::clone(store);
    tokio::task::spawn_blocking(move || st.upsert_quota_snapshot(&row))
        .await
        .map_err(|e| format!("persist join error: {e}"))?
        .map_err(|e| format!("persist error: {e}"))
}

/// 落库后的告警评估：越线沿判定 → insert_alert → webhook 非阻塞投递。
/// R3：pct<0 哨兵样本仅落库不评估（evaluate_crossing 返回 None）。
#[allow(clippy::too_many_arguments)]
async fn evaluate_and_notify(
    store: &Arc<Store>,
    webhook: &WebhookChannel,
    last_pct: &mut HashMap<(&'static str, &'static str), f64>,
    threshold: f64,
    upstream: &'static str,
    metric: &'static str,
    sample: &QuotaSample,
    now: u64,
) {
    let key = (upstream, metric);
    let last = last_pct.get(&key).copied();
    // 哨兵样本不更新 last（保留最近一次可靠观测，回落判定不被 -1 污染）
    if sample.pct >= 0.0 {
        last_pct.insert(key, sample.pct);
    }
    let Some(level) = evaluate_crossing(last, sample.pct, threshold) else {
        return;
    };
    let message = alert_message(upstream, metric, sample.pct, sample.used, sample.quota);
    info!(upstream, metric, level, %message, "monitor: quota alert triggered");
    let st = Arc::clone(store);
    let level_owned = level.to_string();
    let msg = message.clone();
    let inserted = tokio::task::spawn_blocking(move || st.insert_alert(now, &level_owned, &msg))
        .await;
    let alert_id = match inserted {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => {
            warn!(error = %e, "monitor: alert persist failed (webhook skipped)");
            return;
        }
        Err(e) => {
            warn!(error = %e, "monitor: alert persist join error (webhook skipped)");
            return;
        }
    };
    let record = AlertRecord {
        id: alert_id,
        ts: now,
        level: level.to_string(),
        message,
        read_at: None,
    };
    // 非阻塞投递（spec §7）：失败仅 warn，告警已落库，不丢不重试
    let wh = webhook.clone();
    tokio::spawn(async move {
        if let Err(e) = wh.send(&record).await {
            warn!(alert_id = record.id, error = %e, "monitor: webhook delivery failed (alert kept in db)");
        }
    });
}

async fn prune_retention(store: &Arc<Store>, now: u64) {
    let usage_cutoff = hour_floor(now.saturating_sub(USAGE_RETENTION_SEC));
    let quota_cutoff = now.saturating_sub(QUOTA_RETENTION_SEC);
    let st = Arc::clone(store);
    let result = tokio::task::spawn_blocking(move || {
        let usage = st.prune_usage_before(usage_cutoff);
        let quota = st.prune_quota_before(quota_cutoff);
        (usage, quota)
    })
    .await;
    match result {
        Ok((Ok(u), Ok(q))) => {
            if u > 0 || q > 0 {
                info!(usage_pruned = u, quota_pruned = q, "monitor: retention prune done");
            }
        }
        _ => warn!("monitor: retention prune failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M5 §6.1：config 白名单 DTO 恰好两字段且与 MonitorConfig 凭据字段隔离。
    #[test]
    fn monitor_config_dto_whitelisted_shape() {
        let cfg = MonitorConfig {
            poll_interval_sec: 60,
            threshold_pct: 50.0,
            cf_token: Some("cf_secret".into()),
            cf_account_tag: Some("tag".into()),
            cf_graphql_url: None,
            vercel_token: Some("vercel_secret".into()),
            vercel_team_id: None,
            vercel_api_base: None,
            webhook_url: Some("http://hook".into()),
        };
        let handle = MonitorHandle::new(&cfg);
        let dto = handle.config();
        assert_eq!(dto.threshold_pct, 50.0);
        assert_eq!(dto.poll_interval_sec, 60);

        // 序列化键集精确等于两字段——凭据字段（cf_token/vercel_token/webhook_url）
        // 永不出现在响应中
        let json = serde_json::to_value(&dto).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["poll_interval_sec", "threshold_pct"]);
        assert!(!json.to_string().contains("cf_secret"));
        assert!(!json.to_string().contains("vercel_secret"));
        assert!(!json.to_string().contains("http://hook"));
    }
}
