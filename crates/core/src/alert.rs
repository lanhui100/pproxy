//! 告警判定与通知渠道（M3）：越线沿判定纯函数 + WebhookChannel（R5 具体类型）。
//!
//! 判定规则（spec §6）：
//! - 越线沿触发：last < T 且 now ≥ T 才告警一次；持续超阈值不重发；
//! - 回落再越线 → 再次告警；
//! - R3：pct<0 哨兵快照跳过（仅记录不评估）；
//! - critical 分档：pct ≥ 95 → critical，否则 warning。

use serde::Serialize;
use std::time::Duration;

/// critical 分档线（spec §5：alerts.level ∈ {warning, critical}）。
pub const CRITICAL_PCT: f64 = 95.0;

/// 一条告警记录（落库后含 id/read_at；webhook 投递用同一形状）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertRecord {
    pub id: i64,
    pub ts: u64,
    pub level: String,
    pub message: String,
    pub read_at: Option<u64>,
}

/// 越线沿判定（纯函数）：返回 Some(level) 表示应产生一条告警。
///
/// - `now_pct < 0`（R3 哨兵）→ None；
/// - `last` 为 None 或负值哨兵（无可靠上次观测，含首 tick 即采一轮语义，
///   spec 隐含启动即采）视为"此前低于阈值"，当前已超阈值则直接告警；
/// - `last >= T`（持续超阈值或已处于高位）→ None；
/// - `last < T 且 now >= T` → Some(critical/warning)。
pub fn evaluate_crossing(
    last_pct: Option<f64>,
    now_pct: f64,
    threshold_pct: f64,
) -> Option<&'static str> {
    if now_pct.is_nan() || now_pct < 0.0 {
        return None; // NaN 或 -1 哨兵一律跳过
    }
    let was_below = match last_pct {
        None => true,
        // 负值哨兵与缺失等价：不阻塞本次越线判定（NaN < T 恒 false，需显式排除）
        Some(p) => p.is_nan() || p < threshold_pct,
    };
    if !was_below || now_pct < threshold_pct {
        return None;
    }
    Some(if now_pct >= CRITICAL_PCT { "critical" } else { "warning" })
}

/// 人话格式 message（spec §6）：`vercel bandwidth at 83.2% (used/limit)`。
/// 不含凭据；quota<0 时 limit 段为 unknown（该样本不会被评估，此分支仅防御）。
pub fn alert_message(upstream: &str, metric: &str, pct: f64, used: i64, quota: i64) -> String {
    if quota < 0 {
        format!("{upstream} {metric} at {pct:.1}% (unknown)")
    } else {
        format!("{upstream} {metric} at {pct:.1}% ({used}/{quota})")
    }
}

/// webhook body 序列化形状（spec §7 钉死）：`{event:"quota_alert", level, message, ts}`。
pub fn webhook_body(alert: &AlertRecord) -> serde_json::Value {
    serde_json::json!({
        "event": "quota_alert",
        "level": alert.level,
        "message": alert.message,
        "ts": alert.ts,
    })
}

/// Webhook 通知渠道（R5：不用 trait 分发，单实现期持有具体类型；
/// P1 出现第二渠道时再评估 enum 分发或 async-trait）。
#[derive(Clone)]
pub struct WebhookChannel {
    url: Option<String>,
    client: reqwest::Client,
}

impl WebhookChannel {
    /// url 缺失 → 不发外部通知（send 直接收空 Ok，仅落库）。
    pub fn new(url: Option<String>) -> Self {
        WebhookChannel {
            url,
            // 15s 独立超时 client（spec §7）
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("webhook http client build"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.url.is_some()
    }

    /// POST JSON 到配置地址；2xx 即成功。调用方（monitor）负责非阻塞 spawn、
    /// 失败仅 warn（告警已落库，不丢不重试，spec §6）。
    pub async fn send(&self, alert: &AlertRecord) -> Result<(), String> {
        let Some(url) = self.url.as_deref() else {
            return Ok(());
        };
        let resp = self
            .client
            .post(url)
            .json(&webhook_body(alert))
            .send()
            .await
            .map_err(|e| format!("webhook post failed: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("webhook returned status {}", status.as_u16()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(threshold: f64, last: Option<f64>, now: f64) -> Option<&'static str> {
        evaluate_crossing(last, now, threshold)
    }

    // ---- §9.1 越线沿判定 ----

    #[test]
    fn first_crossing_fires() {
        assert_eq!(t(80.0, None, 85.0), Some("warning"), "首观测点即超阈值 → 触发");
        assert_eq!(t(80.0, None, 50.0), None, "首观测点未超阈值");
    }

    #[test]
    fn sustained_over_threshold_does_not_refire() {
        assert_eq!(t(80.0, Some(83.0), 86.0), None, "持续超阈值不重发");
        assert_eq!(t(80.0, Some(80.0), 90.0), None, "已在阈值上不重发");
    }

    #[test]
    fn fall_back_then_recross_fires_again() {
        assert_eq!(t(80.0, Some(85.0), 70.0), None, "回落本身不告警");
        assert_eq!(t(80.0, Some(70.0), 88.0), Some("warning"), "回落后再越线再触发");
    }

    #[test]
    fn negative_sentinel_skipped() {
        assert_eq!(t(80.0, None, -1.0), None, "R3：pct=-1 哨兵仅记录不评估");
        assert_eq!(t(80.0, Some(-1.0), 85.0), Some("warning"), "上次为哨兵不阻塞本次判定");
    }

    #[test]
    fn critical_tiering() {
        assert_eq!(t(80.0, None, 94.9), Some("warning"));
        assert_eq!(t(80.0, None, 95.0), Some("critical"), "≥95 critical 分档");
        assert_eq!(t(80.0, None, 99.9), Some("critical"));
        assert_eq!(
            evaluate_crossing(None, 100.0, 100.0),
            Some("critical"),
            "恰好等于阈值即算越线（now >= T）"
        );
    }

    // ---- message 格式 ----

    #[test]
    fn message_human_readable_no_credentials() {
        let m = alert_message("vercel", "bandwidth", 83.24, 123, 456);
        assert_eq!(m, "vercel bandwidth at 83.2% (123/456)");
        assert!(!m.contains("token") && !m.contains("Bearer"), "不含凭据字样");
        let m2 = alert_message("cf", "requests_daily", 85.0, 85_000, 100_000);
        assert_eq!(m2, "cf requests_daily at 85.0% (85000/100000)");
    }

    // ---- webhook body 形状 ----

    #[test]
    fn webhook_body_shape_pinned() {
        let record = AlertRecord {
            id: 7,
            ts: 1_787_356_800,
            level: "warning".into(),
            message: "cf requests_daily at 85.0% (85000/100000)".into(),
            read_at: None,
        };
        let body = webhook_body(&record);
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 4, "恰好四个字段");
        assert_eq!(obj["event"], "quota_alert");
        assert_eq!(obj["level"], "warning");
        assert_eq!(obj["ts"], 1_787_356_800u64);
        assert!(obj["message"].as_str().unwrap().contains("85000"));
    }

    // ---- WebhookChannel：无 url 时静默成功 ----

    #[tokio::test]
    async fn disabled_channel_send_ok() {
        let ch = WebhookChannel::new(None);
        assert!(!ch.enabled());
        let record = AlertRecord {
            id: 1,
            ts: 0,
            level: "warning".into(),
            message: "x".into(),
            read_at: None,
        };
        assert!(ch.send(&record).await.is_ok(), "未配置 webhook 不算失败");
    }

    #[tokio::test]
    async fn unreachable_webhook_returns_err() {
        // 127.0.0.1:1 不可达 → send 必须返回 Err（由调用方 warn 兜底）
        let ch = WebhookChannel::new(Some("http://127.0.0.1:1/hook".to_string()));
        let record = AlertRecord {
            id: 1,
            ts: 0,
            level: "warning".into(),
            message: "x".into(),
            read_at: None,
        };
        assert!(ch.send(&record).await.is_err());
    }
}
