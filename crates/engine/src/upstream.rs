//! 双模上游管理器（模式 A: 直连边缘 vs 模式 B: 中继远端代理）。
//!
//! 支持运行时原子模式切换、连接池 Graceful Drain，杜绝双模切换时的 502/断流。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use pproxy_core::{EdgeClient, ForwardRequest, RouteTable};
use tokio::sync::RwLock;

/// 远端代理服务器配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProxyConfig {
    pub server_url: String, // e.g. "https://access.ponyjob.top" 或 "http://192.168.1.100:8899"
    pub username: Option<String>,
    pub password: Option<String>,
}

/// 上游工作模式枚举。
#[derive(Clone)]
pub enum UpstreamMode {
    /// 模式 A：本机直连边缘出口（CF Worker / Vercel）
    Direct {
        edges: Arc<HashMap<String, EdgeClient>>,
    },
    /// 模式 B：中继转发至远端代理服务器（如 Linux Dev Server）
    Chained {
        config: RemoteProxyConfig,
        client: reqwest::Client,
    },
}

/// 上游调度器。
pub struct UpstreamManager {
    mode: RwLock<UpstreamMode>,
    routes: Arc<RouteTable>,
}

impl UpstreamManager {
    pub fn new_direct(
        edges: HashMap<String, EdgeClient>,
        routes: Arc<RouteTable>,
    ) -> Self {
        Self {
            mode: RwLock::new(UpstreamMode::Direct {
                edges: Arc::new(edges),
            }),
            routes,
        }
    }

    pub fn new_chained(
        config: RemoteProxyConfig,
        routes: Arc<RouteTable>,
    ) -> Result<Self, reqwest::Error> {
        let client = build_chained_client(&config)?;
        Ok(Self {
            mode: RwLock::new(UpstreamMode::Chained { config, client }),
            routes,
        })
    }

    /// 切换为模式 A（直连）。
    pub async fn switch_to_direct(&self, edges: HashMap<String, EdgeClient>) {
        let mut lock = self.mode.write().await;
        *lock = UpstreamMode::Direct {
            edges: Arc::new(edges),
        };
        tracing::info!("UpstreamManager: Switched to Mode A (Direct EdgeClient)");
    }

    /// 切换为模式 B（中继），原子刷新 Client 连接池。
    pub async fn switch_to_chained(&self, config: RemoteProxyConfig) -> Result<(), reqwest::Error> {
        let client = build_chained_client(&config)?;
        let mut lock = self.mode.write().await;
        *lock = UpstreamMode::Chained { config, client };
        tracing::info!("UpstreamManager: Switched to Mode B (Chained Remote Proxy)");
        Ok(())
    }

    /// 执行转发。
    pub async fn forward(
        &self,
        route_name: &str,
        path_and_query: &str,
        method: Method,
        headers: HeaderMap,
        body_bytes: Vec<u8>,
    ) -> Response {
        let current_mode = { self.mode.read().await.clone() };

        match current_mode {
            UpstreamMode::Direct { edges } => {
                let (target_url, upstream) = match self.routes.resolve(route_name, path_and_query) {
                    Ok(r) => r,
                    Err(_) => {
                        return (
                            StatusCode::NOT_FOUND,
                            [("content-type", "application/json")],
                            r#"{"error":"route_not_found_or_disabled"}"#,
                        )
                            .into_response();
                    }
                };

                let edge = match edges.get(upstream.as_str()) {
                    Some(e) => e,
                    None => {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [("content-type", "application/json")],
                            r#"{"error":"upstream_client_not_configured"}"#,
                        )
                            .into_response();
                    }
                };

                let fwd = ForwardRequest {
                    method: reqwest::Method::from_bytes(method.as_str().as_bytes())
                        .unwrap_or(reqwest::Method::GET),
                    target_url,
                    headers: pproxy_core::EdgeClient::sanitize_headers(&headers),
                    body: if body_bytes.is_empty() {
                        None
                    } else {
                        Some(body_bytes)
                    },
                };

                match edge.execute(fwd).await {
                    Ok(resp) => {
                        let status = resp.status();
                        let resp_headers = resp.headers().clone();
                        let stream = resp.bytes_stream();
                        let body = Body::from_stream(stream);
                        let mut r = Response::new(body);
                        *r.status_mut() = status;
                        for (k, v) in resp_headers.iter() {
                            let name = k.as_str().to_ascii_lowercase();
                            if !matches!(
                                name.as_str(),
                                "transfer-encoding" | "connection" | "content-length"
                            ) {
                                r.headers_mut().insert(k.clone(), v.clone());
                            }
                        }
                        r
                    }
                    Err(e) => {
                        tracing::error!(%route_name, error = %e, "direct upstream forward failed");
                        (
                            StatusCode::BAD_GATEWAY,
                            [("content-type", "application/json")],
                            format!(r#"{{"error":"upstream_failed","detail":"{e}"}}"#),
                        )
                            .into_response()
                    }
                }
            }
            UpstreamMode::Chained { config, client } => {
                let base = config.server_url.trim_end_matches('/');
                let target_url = format!("{base}/{route_name}/{path_and_query}");

                let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
                    .unwrap_or(reqwest::Method::GET);
                let mut req = client.request(req_method, &target_url);

                // 注入远端代理 Proxy-Authorization（与下游业务服务的 Authorization 互不干扰）
                if let (Some(u), Some(p)) = (&config.username, &config.password) {
                    let creds = format!("{u}:{p}");
                    let encoded = base64::engine::general_purpose::STANDARD.encode(creds.as_bytes());
                    req = req.header("proxy-authorization", format!("Basic {encoded}"));
                }

                // 完整清洗 Hop-by-Hop 头，同时保留下游业务的 Authorization (如 Bearer token)
                for (k, v) in headers.iter() {
                    let name = k.as_str().to_ascii_lowercase();
                    if !matches!(
                        name.as_str(),
                        "host"
                            | "content-length"
                            | "connection"
                            | "proxy-authorization"
                            | "keep-alive"
                            | "proxy-authenticate"
                            | "te"
                            | "trailer"
                            | "transfer-encoding"
                            | "upgrade"
                            | "x-proxy-secret"
                    ) {
                        req = req.header(k.as_str(), v.as_bytes());
                    }
                }

                if !body_bytes.is_empty() {
                    req = req.body(body_bytes);
                }

                match req.send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let resp_headers = resp.headers().clone();
                        let stream = resp.bytes_stream();
                        let body = Body::from_stream(stream);
                        let mut r = Response::new(body);
                        *r.status_mut() = status;
                        for (k, v) in resp_headers.iter() {
                            let name = k.as_str().to_ascii_lowercase();
                            if !matches!(
                                name.as_str(),
                                "transfer-encoding" | "connection" | "content-length"
                            ) {
                                r.headers_mut().insert(k.clone(), v.clone());
                            }
                        }
                        r
                    }
                    Err(e) => {
                        tracing::error!(%route_name, error = %e, "chained remote proxy forward failed");
                        (
                            StatusCode::BAD_GATEWAY,
                            [("content-type", "application/json")],
                            format!(r#"{{"error":"remote_proxy_unreachable","detail":"{e}"}}"#),
                        )
                            .into_response()
                    }
                }
            }
        }
    }
}

fn build_chained_client(_config: &RemoteProxyConfig) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
}
