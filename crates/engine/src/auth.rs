//! 网关统一认证中间件（Basic Auth + Token Auth + Gatekeeper 防爆破）。

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;

pub const X_PONY_TOKEN: &str = "x-pony-token";
pub const PROXY_AUTHORIZATION: &str = "proxy-authorization";
pub const AUTHORIZATION: &str = "authorization";

/// 鉴权方式与主体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSubject {
    User { id: i64, username: String },
    Token { id: i64, name: String },
    /// 集群节点转发来源（X-Pony-Cluster-Ticket 验签通过；id 存 node_id 字符串）
    ClusterNode { id: i64, name: String },
}

/// 鉴权成功后注入 Request Extension 的上下文。
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub subject: AuthSubject,
    pub route: String,
    pub path_query: String,
}

/// 从 Basic Auth Header (如 `Basic dXNlcjpwYXNz`) 解析 (username, password)。
pub fn parse_basic_auth(header_val: &str) -> Option<(String, String)> {
    let header_val = header_val.trim();
    let payload = if header_val.to_ascii_lowercase().starts_with("basic ") {
        &header_val[6..].trim()
    } else {
        return None;
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    let creds = String::from_utf8(decoded).ok()?;
    let (username, password) = creds.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

/// 从 Headers 提取所有认证凭据。
pub fn extract_credentials(headers: &HeaderMap) -> (Option<(String, String)>, Option<String>) {
    // 1. 尝试从 Proxy-Authorization 或 Authorization 提取 Basic Auth
    let basic_auth = headers
        .get(PROXY_AUTHORIZATION)
        .or_else(|| headers.get(AUTHORIZATION))
        .and_then(|v| v.to_str().ok())
        .and_then(parse_basic_auth);

    // 2. 尝试从 X-Pony-Token 提取 Token
    let token_header = headers
        .get(X_PONY_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    (basic_auth, token_header)
}

/// 拆分路径的首段与剩余部分。
pub fn split_first_segment(path: &str) -> (&str, &str) {
    let clean = path.trim_start_matches('/');
    match clean.split_once('/') {
        Some((first, rest)) => (first, rest),
        None => (clean, ""),
    }
}

/// 统一 401 响应。
pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        r#"{"error":"unauthorized"}"#,
    )
        .into_response()
}

/// 统一 407 响应（代理鉴权请求）。
pub fn proxy_auth_required() -> Response {
    (
        StatusCode::PROXY_AUTHENTICATION_REQUIRED,
        [
            ("proxy-authenticate", r#"Basic realm="Pony Proxy""#),
            ("content-type", "application/json"),
        ],
        r#"{"error":"proxy_authentication_required"}"#,
    )
        .into_response()
}

/// 统一 429 锁定响应（防爆破）。
pub fn rate_limited_lockout() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "application/json")],
        r#"{"error":"ip_temporarily_locked_due_to_auth_failures"}"#,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_auth_valid() {
        let header = "Basic YWxpY2U6bXlwYXNz"; // alice:mypass
        let parsed = parse_basic_auth(header);
        assert_eq!(parsed, Some(("alice".to_string(), "mypass".to_string())));
    }

    #[test]
    fn parse_basic_auth_invalid() {
        assert_eq!(parse_basic_auth("Bearer xxx"), None);
        assert_eq!(parse_basic_auth("Basic !!!invalidbase64"), None);
        assert_eq!(parse_basic_auth("Basic bm9jb2xvbg=="), None); // "nocolon"
    }

    #[test]
    fn split_first_segment_vectors() {
        assert_eq!(split_first_segment("/openai/v1/chat"), ("openai", "v1/chat"));
        assert_eq!(split_first_segment("openai/v1/chat"), ("openai", "v1/chat"));
        assert_eq!(split_first_segment("/openai"), ("openai", ""));
        assert_eq!(split_first_segment(""), ("", ""));
    }
}
