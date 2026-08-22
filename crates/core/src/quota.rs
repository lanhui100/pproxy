//! 配额采集（M3）：Cloudflare Workers GraphQL 用量 + Vercel usage API。
//!
//! WHY 纯函数与 IO 分离：GraphQL 请求体构造、响应解析、UTC 日期换算均为
//! 纯函数可单测（spec §9.2）；HTTP 客户端经构造注入，10s 超时独立 client，
//! 失败不重试（下个 tick 自然重试，spec §4.1）。
//!
//! R3：quota=-1 表示"未知上限"（Hobby 无官方常量），pct 同落 -1 哨兵，
//! 告警判定跳过 pct<0 的快照（alert::evaluate_crossing）。

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// CF Workers 免费额度常量（spec §4.1）：100,000 请求/日。
pub const CF_DAILY_REQUEST_QUOTA: i64 = 100_000;

/// CF 默认 GraphQL 端点；PPROXY_CF_GRAPHQL_URL 仅测试覆盖。
pub const CF_GRAPHQL_URL_DEFAULT: &str = "https://api.cloudflare.com/client/v4/graphql";

/// Vercel 默认 API base；PPROXY_VERCEL_API_BASE 仅测试覆盖。
pub const VERCEL_API_BASE_DEFAULT: &str = "https://api.vercel.com";

/// metric 命名（spec §4）：cf=requests_daily；vercel=bandwidth / function_invocations。
pub const METRIC_CF_REQUESTS_DAILY: &str = "requests_daily";
pub const METRIC_VERCEL_BANDWIDTH: &str = "bandwidth";
pub const METRIC_VERCEL_FUNCTION_INVOCATIONS: &str = "function_invocations";

pub const UPSTREAM_CF: &str = "cf";
pub const UPSTREAM_VERCEL: &str = "vercel";

/// 单次采样样本。quota=-1 / pct=-1 为哨兵值（R3）。
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSample {
    pub used: i64,
    pub quota: i64,
    pub pct: f64,
}

impl QuotaSample {
    pub fn new(used: i64, quota: i64) -> Self {
        QuotaSample { used, quota, pct: quota_pct(used, quota) }
    }
}

/// used/quota 百分比；quota≤0 → -1.0 哨兵（R3：DDL REAL NOT NULL 必须有值）。
pub fn quota_pct(used: i64, quota: i64) -> f64 {
    if quota <= 0 {
        return -1.0;
    }
    (used as f64 / quota as f64) * 100.0
}

/// 采集来源健康状态（/api/quota sources 字段，spec §8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSourceState {
    Ok,
    Disabled,
    Error,
    UnsupportedPlan,
}

impl QuotaSourceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Disabled => "disabled",
            Self::Error => "error",
            Self::UnsupportedPlan => "unsupported_plan",
        }
    }
}

/// 采集失败统一错误（Display 不含凭据；reqwest 错误仅透出 URL/连接信息）。
#[derive(Debug)]
pub enum CollectError {
    /// 来源未配置凭据（调用方应先 enabled() 判断，此变体为防御性兜底）。
    Disabled,
    Http(reqwest::Error),
    Status(u16),
    Parse(String),
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "source disabled (missing credentials)"),
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Status(code) => write!(f, "http status {code}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
        }
    }
}

impl std::error::Error for CollectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for CollectError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

// ---- 时间纯函数（无 chrono 依赖，workspace 冻结）----

/// unix 秒 → UTC "YYYY-MM-DD"（civil_from_days，Howard Hinnant 算法）。
pub fn utc_date_string(ts: u64) -> String {
    let (y, m, d) = civil_from_days((ts / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// unix 秒 → UTC RFC3339 形式 "YYYY-MM-DDTHH:MM:SSZ"（Vercel from/to 用）。
pub fn iso_utc_string(ts: u64) -> String {
    let day = (ts / 86_400) as i64;
    let rem = ts % 86_400;
    let (y, mo, d) = civil_from_days(day);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 天数（自 1970-01-01）→ (年, 月, 日)。Proleptic Gregorian 历法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- Cloudflare ----

const CF_GRAPHQL_QUERY: &str = r#"query($accountTag: String!, $since: Date!, $until: Date!) {
  viewer { accounts(filter: {accountTag: $accountTag}) {
    workersInvocationsAdaptive(
      filter: { date_geq: $since, date_leq: $until }, limit: 10000,
      orderBy: [date_ASC]) {
      sum { requests }
      dimensions { date }
    } } }
}"#;

/// 构造 GraphQL 请求体。S-P2-10：accountTag/since/until 一律走 JSON variables
/// 字段由 serde 转义，禁止 format! 拼接查询文本（无注入面）。
pub fn cf_graphql_request_body(account_tag: &str, since: &str, until: &str) -> serde_json::Value {
    serde_json::json!({
        "query": CF_GRAPHQL_QUERY,
        "variables": {
            "accountTag": account_tag,
            "since": since,
            "until": until,
        },
    })
}

/// CF GraphQL 解析三态输入（spec §9.2）：
/// - [`CfUsageOutcome::Ok`]：正常（空行集 → used=0，仍属正常口径）
/// - [`CfUsageOutcome::GraphqlErrors`]：errors 数组非空 → 该次采集失败
#[derive(Debug, Clone, PartialEq)]
pub enum CfUsageOutcome {
    Ok(QuotaSample),
    GraphqlErrors(String),
}

#[derive(Deserialize)]
struct CfErrorItem {
    #[serde(default)]
    message: Option<String>,
}

/// 解析 CF GraphQL 响应：errors 非空优先返回；used = 各行 sum(requests) 求和；
/// 行集为空 → used=0。data 缺失视为解析失败。
pub fn parse_cf_usage(body: &[u8], quota: i64) -> Result<CfUsageOutcome, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    if let Some(errs) = v.get("errors").and_then(|e| e.as_array()) {
        if !errs.is_empty() {
            let msgs: Vec<String> = errs
                .iter()
                .filter_map(|e| {
                    serde_json::from_value::<CfErrorItem>(e.clone())
                        .ok()
                        .and_then(|i| i.message)
                })
                .collect();
            return Ok(CfUsageOutcome::GraphqlErrors(if msgs.is_empty() {
                "(graphql errors without message)".to_string()
            } else {
                msgs.join("; ")
            }));
        }
    }
    let rows = v
        .pointer("/data/viewer/accounts/0/workersInvocationsAdaptive")
        .and_then(|r| r.as_array())
        .ok_or_else(|| "missing data.viewer.accounts[0].workersInvocationsAdaptive".to_string())?;
    let mut used: i64 = 0;
    for r in rows {
        used += r.pointer("/sum/requests").and_then(|x| x.as_i64()).unwrap_or(0);
    }
    Ok(CfUsageOutcome::Ok(QuotaSample::new(used, quota)))
}

/// Cloudflare Workers 用量采集器（10s 独立超时 client）。
pub struct CfCollector {
    token: Option<String>,
    account_tag: Option<String>,
    graphql_url: String,
    client: reqwest::Client,
}

impl CfCollector {
    /// token 与 account_tag 任一缺失 → 来源 disabled（启动 info 一行非错误）。
    pub fn new(token: Option<String>, account_tag: Option<String>, graphql_url: Option<String>) -> Self {
        CfCollector {
            token,
            account_tag,
            graphql_url: graphql_url
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| CF_GRAPHQL_URL_DEFAULT.to_string()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("cf http client build"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.token.is_some() && self.account_tag.is_some()
    }

    /// 当日 UTC 窗口 requests_daily 采样。
    pub async fn collect_daily_requests(&self, now_unix_ts: u64) -> Result<QuotaSample, CollectError> {
        let Some(token) = self.token.as_deref() else {
            return Err(CollectError::Disabled);
        };
        let Some(tag) = self.account_tag.as_deref() else {
            return Err(CollectError::Disabled);
        };
        let date = utc_date_string(now_unix_ts);
        let body = cf_graphql_request_body(tag, &date, &date);
        let resp = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(CollectError::Status(status.as_u16()));
        }
        match parse_cf_usage(&bytes, CF_DAILY_REQUEST_QUOTA).map_err(CollectError::Parse)? {
            CfUsageOutcome::Ok(s) => Ok(s),
            CfUsageOutcome::GraphqlErrors(msg) => Err(CollectError::Parse(format!(
                "cloudflare graphql errors: {msg}"
            ))),
        }
    }
}

// ---- Vercel ----

/// Vercel usage 解析结果（spec §4.2 Hobby 降级口径）。
#[derive(Debug, Clone, PartialEq)]
pub enum VercelUsageOutcome {
    /// `plan_upgrade_required`：能力探测降级，非错误路径；首 tick warn 一次后静默 skip。
    UnsupportedPlan,
    /// 可用：bandwidth / function_invocations 两指标（quota=-1/pct=-1 哨兵，仅记录不评估）。
    Samples(Vec<(&'static str, QuotaSample)>),
}

/// 解析 Vercel /v1/usage 响应。字段名以实际响应为准并由单测钉住：
/// `usage.bandwidth`、`usage.functionInvocations`（camelCase，Vercel 官方口径）。
pub fn parse_vercel_usage(body: &[u8]) -> Result<VercelUsageOutcome, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    if let Some(code) = v.pointer("/error/code").and_then(|c| c.as_str()) {
        if code == "plan_upgrade_required" {
            return Ok(VercelUsageOutcome::UnsupportedPlan);
        }
        return Err(format!("vercel api error: {code}"));
    }
    let usage = v.get("usage").cloned().unwrap_or(serde_json::Value::Null);
    let bandwidth = usage.get("bandwidth").and_then(|x| x.as_i64()).unwrap_or(0);
    let func = usage
        .get("functionInvocations")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    Ok(VercelUsageOutcome::Samples(vec![
        (METRIC_VERCEL_BANDWIDTH, QuotaSample::new(bandwidth, -1)),
        (
            METRIC_VERCEL_FUNCTION_INVOCATIONS,
            QuotaSample::new(func, -1),
        ),
    ]))
}

/// Vercel usage 采集器（10s 独立超时 client）。
pub struct VercelCollector {
    token: Option<String>,
    team_id: Option<String>,
    api_base: String,
    client: reqwest::Client,
}

impl VercelCollector {
    /// token 缺失 → disabled；team_id 缺失按单用户口径尝试（spec §3）。
    pub fn new(token: Option<String>, team_id: Option<String>, api_base: Option<String>) -> Self {
        VercelCollector {
            token,
            team_id,
            api_base: api_base
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| VERCEL_API_BASE_DEFAULT.to_string())
                .trim_end_matches('/')
                .to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("vercel http client build"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.token.is_some()
    }

    /// GET /v1/usage?from=<ISO>&to=<ISO>[&teamId=]（当日 UTC 窗口）。
    pub async fn collect_usage(&self, now_unix_ts: u64) -> Result<VercelUsageOutcome, CollectError> {
        let Some(token) = self.token.as_deref() else {
            return Err(CollectError::Disabled);
        };
        let day_start = now_unix_ts - now_unix_ts % 86_400;
        let mut url = format!(
            "{}/v1/usage?from={}&to={}",
            self.api_base,
            urlencode(iso_utc_string(day_start)),
            urlencode(iso_utc_string(now_unix_ts)),
        );
        if let Some(team) = self.team_id.as_deref().filter(|t| !t.is_empty()) {
            url.push_str("&teamId=");
            url.push_str(team);
        }
        let resp = self
            .client
            .get(url)
            .bearer_auth(token)
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(CollectError::Status(status.as_u16()));
        }
        parse_vercel_usage(&bytes).map_err(CollectError::Parse)
    }
}

/// 最小 URL 编码：ISO 串中的 ':' 与 '+'（生成串仅含这两类保留字符）。
fn urlencode(s: String) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- utc_date_string / iso_utc_string ----

    #[test]
    fn utc_date_known_values() {
        assert_eq!(utc_date_string(0), "1970-01-01");
        assert_eq!(utc_date_string(86_399), "1970-01-01");
        assert_eq!(utc_date_string(86_400), "1970-01-02");
        // 实测校准基准（date -u -d @1787356800 +%F → 2026-08-22）
        assert_eq!(utc_date_string(1_787_356_800), "2026-08-22");
        assert_eq!(utc_date_string(1_787_356_799), "2026-08-21", "秒级边界归前一天");
    }

    #[test]
    fn iso_utc_known_values() {
        assert_eq!(iso_utc_string(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso_utc_string(1_787_356_800 + 50_000),
            "2026-08-22T13:53:20Z"
        );
    }

    // ---- quota_pct / QuotaSample ----

    #[test]
    fn quota_pct_normal_and_sentinel() {
        assert!((quota_pct(85_000, 100_000) - 85.0).abs() < f64::EPSILON);
        assert_eq!(quota_pct(0, 100_000), 0.0);
        assert_eq!(quota_pct(123, -1), -1.0, "R3：未知上限 → -1 哨兵");
        assert_eq!(quota_pct(123, 0), -1.0);
        assert_eq!(QuotaSample::new(85_000, 100_000).pct, 85.0);
    }

    // ---- cf_graphql_request_body：变量化注入面 ----

    #[test]
    fn cf_body_variables_not_interpolated() {
        // S-P2-10：恶意 accountTag 必须经 JSON variables 序列化转义，
        // 不得出现在 query 文本中（禁止 format! 拼 GraphQL）。
        let evil = "\"} evil {\"";
        let body = cf_graphql_request_body(evil, "2026-08-22", "2026-08-22");
        let query = body["query"].as_str().unwrap();
        assert!(query.contains("$accountTag"), "query 使用变量占位");
        assert!(!query.contains(evil), "查询文本不含插值");
        let vars = &body["variables"];
        assert_eq!(vars["accountTag"].as_str(), Some(evil));
        assert_eq!(vars["since"].as_str(), Some("2026-08-22"));
        assert_eq!(vars["until"].as_str(), Some("2026-08-22"));
    }

    // ---- parse_cf_usage 三态 ----

    #[test]
    fn parse_cf_usage_normal_sums_rows() {
        let body = br#"{"errors":[],"data":{"viewer":{"accounts":[{"workersInvocationsAdaptive":[{"sum":{"requests":60000},"dimensions":{"date":"2026-08-22"}},{"sum":{"requests":25000},"dimensions":{"date":"2026-08-22"}}]}]}}}"#;
        let out = parse_cf_usage(body, CF_DAILY_REQUEST_QUOTA).unwrap();
        assert_eq!(
            out,
            CfUsageOutcome::Ok(QuotaSample::new(85_000, 100_000)),
            "used = 各行 sum(requests) 求和"
        );
    }

    #[test]
    fn parse_cf_usage_errors_non_empty() {
        let body = br#"{"errors":[{"message":"Authentication error"}],"data":null}"#;
        let out = parse_cf_usage(body, CF_DAILY_REQUEST_QUOTA).unwrap();
        assert_eq!(
            out,
            CfUsageOutcome::GraphqlErrors("Authentication error".to_string())
        );
    }

    #[test]
    fn parse_cf_usage_empty_rows_is_zero() {
        let body = br#"{"errors":[],"data":{"viewer":{"accounts":[{"workersInvocationsAdaptive":[]}]}}}"#;
        let out = parse_cf_usage(body, CF_DAILY_REQUEST_QUOTA).unwrap();
        assert_eq!(out, CfUsageOutcome::Ok(QuotaSample::new(0, 100_000)));
    }

    #[test]
    fn parse_cf_usage_missing_data_is_parse_error() {
        assert!(parse_cf_usage(br#"{"errors":[],"data":null}"#, 100_000).is_err());
        assert!(parse_cf_usage(b"not json", 100_000).is_err());
    }

    // ---- parse_vercel_usage：降级识别 + 字段钉住 ----

    #[test]
    fn vercel_plan_upgrade_required_recognized() {
        // 实测 Hobby 返回形状（spec §0 探测结论），字段路径 /error/code
        let body = br#"{"error":{"code":"plan_upgrade_required","message":"..."}}"#;
        assert_eq!(
            parse_vercel_usage(body).unwrap(),
            VercelUsageOutcome::UnsupportedPlan
        );
    }

    #[test]
    fn vercel_usage_samples_pinned_fields() {
        // 字段名以实际响应为准并在此钉住：usage.bandwidth / usage.functionInvocations
        let raw = br#"{"usage":{"bandwidth":123456789,"functionInvocations":4200}}"#;
        let out = parse_vercel_usage(raw).unwrap();
        match out {
            VercelUsageOutcome::Samples(samples) => {
                assert_eq!(samples.len(), 2);
                assert_eq!(samples[0].0, METRIC_VERCEL_BANDWIDTH);
                assert_eq!(samples[0].1.used, 123_456_789);
                assert_eq!(samples[0].1.quota, -1, "Hobby 无官方上限 → -1 哨兵");
                assert_eq!(samples[0].1.pct, -1.0, "R3：pct 哨兵");
                assert_eq!(samples[1].0, METRIC_VERCEL_FUNCTION_INVOCATIONS);
                assert_eq!(samples[1].1.used, 4_200);
            }
            other => panic!("expected samples, got {other:?}"),
        }
    }

    #[test]
    fn vercel_missing_usage_defaults_zero_and_other_error_is_err() {
        let out = parse_vercel_usage(br#"{"usage":{}}"#).unwrap();
        match out {
            VercelUsageOutcome::Samples(s) => {
                assert!(s.iter().all(|(_, q)| q.used == 0));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_vercel_usage(br#"{"error":{"code":"invalid_token"}}"#).is_err());
    }
}
