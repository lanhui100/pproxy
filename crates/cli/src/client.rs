//! AdminClient：管理面 REST API 的 reqwest 封装（M2 §2）。
//!
//! 唯一 HTTP 出口：全部 /api/* 调用集中于此，cmd/* 只消费结构化结果。
//! 错误统一 [`ApiError`]，main 按类别映射退出码（1/3）。

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize as _;
use serde_json::Value;

/// API 调用错误（退出码映射在 main：Connection→3，其余→1）。
#[derive(Debug)]
pub enum ApiError {
    /// 管理面不可达/超时/DNS 失败 → 退出码 3
    Connection(String),
    /// 服务端返回 4xx/5xx → 退出码 1（携带状态码与 error 文案）
    Status(u16, String),
    /// 响应体不符合预期 JSON 结构 → 退出码 1
    BadResponse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(e) => write!(
                f,
                "cannot reach admin api: {e} — pproxy-server 未运行? (systemctl status pproxy)"
            ),
            Self::Status(code, msg) => write!(f, "api error (HTTP {code}): {msg}"),
            Self::BadResponse(e) => write!(f, "unexpected response: {e}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// main 的退出码映射纯函数（可测）：Connection→3，其余→1。
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Connection(_) => 3,
            _ => 1,
        }
    }

    fn from_reqwest(e: reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            // 有响应状态码的错误（服务端返回的非 2xx）
            return Self::Status(status.as_u16(), e.to_string());
        }
        if e.is_timeout() || e.is_connect() || e.is_request() {
            return Self::Connection(e.to_string());
        }
        Self::BadResponse(e.to_string())
    }
}

/// 管理 API 客户端。clone 廉价（内部 Arc）。
#[derive(Clone)]
pub struct AdminClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

/// GET /api/tokens 列表行（脱敏字段，C-P1-8）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TokenInfo {
    #[serde(deserialize_with = "de_i64_from_u64")]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub last_used_at: Option<u64>,
    #[serde(default)]
    pub status: Option<String>,
}

/// GET /api/routes 列表行。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RouteInfo {
    pub name: String,
    pub target_host: String,
    #[serde(default)]
    pub override_upstream: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub effective_upstream: Option<String>,
}

/// POST /api/routes/{name}/test 结果。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RouteTestResult {
    pub ok: bool,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// GET /api/usage 行。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UsageRowInfo {
    pub route: String,
    #[serde(deserialize_with = "de_i64_from_u64")]
    pub token_id: i64,
    pub requests: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// GET /api/usage 响应。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UsageReport {
    #[serde(default)]
    pub rows: Vec<UsageRowInfo>,
    #[serde(default)]
    pub total: UsageTotal,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct UsageTotal {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub bytes_in: u64,
    #[serde(default)]
    pub bytes_out: u64,
}

/// GET /api/tokens 创建响应（明文仅此一次）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreatedToken {
    #[serde(deserialize_with = "de_i64_from_u64")]
    pub id: i64,
    pub name: String,
    pub token: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

fn de_i64_from_u64<'de, D>(de: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Value::deserialize(de)?;
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| u as i64))
            .ok_or_else(|| serde::de::Error::custom("expected integer")),
        _ => Err(serde::de::Error::custom("expected number")),
    }
}

/// 同 host 才跟随重定向（M2 §6 安全约束）。纯函数便于复用。
fn follow_redirect(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    match attempt.url().host_str() {
        Some(h) if attempt.previous().last().and_then(|u| u.host_str()) == Some(h) => attempt.follow(),
        _ => attempt.stop(),
    }
}

impl AdminClient {
    /// 构建 client。`base` 形如 `http://127.0.0.1:8900`（尾斜杠容忍）。
    /// 默认 15s 超时；test 类调用用 [`Self::with_timeout`] 放宽到 30s（M2 §6）。
    pub fn new(base: &str, token: &str) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::custom(follow_redirect))
            .build()
            .map_err(ApiError::from_reqwest)?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    /// 放宽超时的副本（route test / doctor 用）。
    pub fn with_timeout(&self, secs: u64) -> Self {
        let mut c = self.clone();
        c.http = reqwest::Client::builder()
            .timeout(Duration::from_secs(secs))
            .redirect(reqwest::redirect::Policy::custom(follow_redirect))
            .build()
            .unwrap_or_else(|_| c.http.clone());
        c
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, ApiError> {
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.request(method, &url).bearer_auth(&self.token);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(ApiError::from_reqwest)?;
        let status = resp.status();
        let text = resp.text().await.map_err(ApiError::from_reqwest)?;
        if !status.is_success() {
            // error 字段优先，body 非 JSON 时截断原文 200 字节（M2 §4）
            let msg = parse_error_message(&text);
            return Err(ApiError::Status(status.as_u16(), msg));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| ApiError::BadResponse(format!("invalid json from {url}: {e}")))
    }

    // ---- health ----

    pub async fn health(&self) -> Result<Value, ApiError> {
        self.send(reqwest::Method::GET, "/api/health", None, None).await
    }

    // ---- tokens ----

    pub async fn create_token(
        &self,
        name: &str,
        expires_days: Option<u64>,
    ) -> Result<CreatedToken, ApiError> {
        let mut body = serde_json::json!({ "name": name });
        if let Some(d) = expires_days {
            body["expires_days"] = Value::from(d);
        }
        let v = self
            .send(reqwest::Method::POST, "/api/tokens", Some(body), None)
            .await?;
        serde_json::from_value(v).map_err(|e| ApiError::BadResponse(e.to_string()))
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenInfo>, ApiError> {
        let v = self.send(reqwest::Method::GET, "/api/tokens", None, None).await?;
        let items = v.get("tokens").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| ApiError::BadResponse(e.to_string()))
    }

    pub async fn revoke_token(&self, id: i64) -> Result<bool, ApiError> {
        let v = self
            .send(reqwest::Method::DELETE, &format!("/api/tokens/{id}"), None, None)
            .await?;
        Ok(v.get("revoked").and_then(Value::as_bool).unwrap_or(false))
    }

    // ---- routes ----

    pub async fn list_routes(&self) -> Result<Vec<RouteInfo>, ApiError> {
        let v = self.send(reqwest::Method::GET, "/api/routes", None, None).await?;
        let items = v.get("routes").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| ApiError::BadResponse(e.to_string()))
    }

    pub async fn create_route(
        &self,
        name: &str,
        target_host: &str,
        override_upstream: Option<&str>,
    ) -> Result<(String, String), ApiError> {
        let mut body = serde_json::json!({ "name": name, "target_host": target_host });
        if let Some(u) = override_upstream {
            body["override_upstream"] = Value::from(u);
        }
        let v = self
            .send(reqwest::Method::POST, "/api/routes", Some(body), None)
            .await?;
        let upstream = v
            .get("upstream")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let rname = v.get("name").and_then(Value::as_str).unwrap_or(name).to_string();
        Ok((rname, upstream))
    }

    /// DELETE 返回是否命中（false=404 not_found）。
    pub async fn delete_route(&self, name: &str) -> Result<bool, ApiError> {
        match self
            .send(reqwest::Method::DELETE, &format!("/api/routes/{name}"), None, None)
            .await
        {
            Ok(_) => Ok(true),
            Err(ApiError::Status(404, _)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// PATCH 三态透传（enabled / override_upstream 各自独立调用入口封装在此）。
    pub async fn set_route_enabled(&self, name: &str, enabled: bool) -> Result<(), ApiError> {
        let body = serde_json::json!({ "enabled": enabled });
        self.send(reqwest::Method::PATCH, &format!("/api/routes/{name}"), Some(body), None)
            .await?;
        Ok(())
    }

    pub async fn test_route(&self, name: &str) -> Result<RouteTestResult, ApiError> {
        let v = self
            .send(
                reqwest::Method::POST,
                &format!("/api/routes/{name}/test"),
                None,
                Some(Duration::from_secs(30)),
            )
            .await?;
        serde_json::from_value(v).map_err(|e| ApiError::BadResponse(e.to_string()))
    }

    // ---- usage ----

    pub async fn usage(
        &self,
        hours: u64,
        route: Option<&str>,
        token_id: Option<i64>,
    ) -> Result<UsageReport, ApiError> {
        let mut params = HashMap::new();
        params.insert("hours", hours.to_string());
        if let Some(r) = route {
            params.insert("route", r.to_string());
        }
        if let Some(t) = token_id {
            params.insert("token_id", t.to_string());
        }
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let v = self
            .send(reqwest::Method::GET, &format!("/api/usage?{query}"), None, None)
            .await?;
        serde_json::from_value(v).map_err(|e| ApiError::BadResponse(e.to_string()))
    }

    /// 数据面抽样探测：GET 完整 URL，返回状态码（doctor 用）。
    /// 不走 /api/*，无鉴权头；连接类错误统一 Connection 语义。
    pub async fn probe_get(&self, url: &str) -> Result<u16, ApiError> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(ApiError::from_reqwest)?;
        Ok(resp.status().as_u16())
    }
}

/// 从错误 body 提取 error 字段；非 JSON 时截断原文 200 字节。
pub(crate) fn parse_error_message(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(body)
            .to_string(),
        Err(_) => truncate_bytes(body, 200),
    }
}

/// 按 UTF-8 安全边界截断（字节上限）。
pub(crate) fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_mapping() {
        assert_eq!(ApiError::Connection("x".into()).exit_code(), 3);
        assert_eq!(ApiError::Status(404, "not_found".into()).exit_code(), 1);
        assert_eq!(ApiError::BadResponse("x".into()).exit_code(), 1);
    }

    #[test]
    fn error_body_parsing() {
        assert_eq!(
            parse_error_message(r#"{"error":"not_found"}"#),
            "not_found"
        );
        assert_eq!(
            parse_error_message("<html>oops</html>"),
            "<html>oops</html>"
        );
    }

    #[test]
    fn truncation_is_utf8_safe_and_capped() {
        let long = "a".repeat(300);
        let t = truncate_bytes(&long, 200);
        assert_eq!(t.len(), 203); // 200 + '…'(3 bytes)
        assert!(t.ends_with('…'));

        // 多字节边界：中文字符 3 字节
        let cn = "中".repeat(100); // 300 bytes
        let t = truncate_bytes(&cn, 200);
        assert!(!t.trim_end_matches('…').ends_with('\u{FFFD}'));
        assert!(t.chars().count() <= 68);
    }

    #[tokio::test]
    async fn connection_refused_maps_to_connection_variant() {
        // 关闭端口上必连失败
        let c = AdminClient::new("http://127.0.0.1:1", "tok").unwrap();
        let err = c.health().await.unwrap_err();
        assert!(matches!(err, ApiError::Connection(_)));
        assert_eq!(err.exit_code(), 3);
    }
}
