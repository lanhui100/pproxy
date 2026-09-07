use anyhow::Context;
use reqwest::header::HeaderMap;
use reqwest::{Client, Method, Response};

const HOP_BY_HOP: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "x-proxy-secret",
];

#[derive(Clone)]
pub struct EdgeClient {
    http: Client,
    worker_url: String,
    secret: String,
}

pub struct ForwardRequest {
    pub method: Method,
    pub target_url: String,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
}

impl EdgeClient {
    pub fn new(worker_url: &str, secret: &str) -> anyhow::Result<Self> {
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
            .pool_idle_timeout(Some(std::time::Duration::from_secs(90)))
            .pool_max_idle_per_host(32)
            .build()
            .context("build edge http client")?;
        Ok(Self {
            http,
            worker_url: worker_url.trim_end_matches('/').to_string(),
            secret: secret.to_string(),
        })
    }

    pub fn sanitize_headers(headers: &HeaderMap) -> HeaderMap {
        let mut out = HeaderMap::new();
        for (k, v) in headers {
            let name = k.as_str().to_ascii_lowercase();
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
        out
    }

    /// 连接池保活 ping（性能专项，PPROXY_EDGE_KEEPALIVE 开启时由装配点周期调用）。
    ///
    /// 两个正确性要点（对抗审核 P0）：
    /// 1. 必须读尽响应 body——否则 reqwest 不把连接归还池，预热完全无效；
    /// 2. 不走 `execute`——避免误触发幂等重试/120s 超时逻辑，也不带 url 参数
    ///    （CF worker 返回 404 伪装页、Vercel 返回 400，均可在握手层完成预热）。
    pub async fn keepalive_ping(&self) -> anyhow::Result<()> {
        let resp = self
            .http
            .get(&self.worker_url)
            .header("X-Proxy-Secret", &self.secret)
            .send()
            .await
            .context("edge keepalive ping failed")?;
        // 读尽 body，连接才归还池
        let _ = resp.bytes().await.context("edge keepalive drain failed")?;
        Ok(())
    }

    pub async fn execute(&self, req: ForwardRequest) -> anyhow::Result<Response> {
        let is_idempotent = matches!(
            req.method,
            Method::GET | Method::HEAD | Method::OPTIONS
        );
        let has_idempotency_key = req.headers.contains_key("idempotency-key")
            || req.headers.contains_key("x-idempotency-key")
            || req.headers.contains_key("idempotency_key")
            || req.headers.contains_key("x-idempotency_key");
        let retryable_method = is_idempotent || has_idempotency_key;

        let max_attempts = 5;
        let mut attempt = 0;

        loop {
            attempt += 1;
            let mut url = reqwest::Url::parse(&self.worker_url).context("invalid worker_url")?;
            url.query_pairs_mut().append_pair("url", &req.target_url);

            let mut builder = self
                .http
                .request(req.method.clone(), url)
                .header("X-Proxy-Secret", &self.secret)
                .headers(req.headers.clone());

            if let Some(body) = &req.body {
                builder = builder.body(body.clone());
            }

            let send_res = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                builder.send(),
            )
            .await;

            match send_res {
                Ok(Ok(resp)) => {
                    let status = resp.status();
                    // 幂等请求或携带幂等键的请求遇到 502/503/504 时重试，上游可凭键去重
                    if retryable_method
                        && (status == reqwest::StatusCode::BAD_GATEWAY
                            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                            || status == reqwest::StatusCode::GATEWAY_TIMEOUT)
                        && attempt < max_attempts
                    {
                        tracing::warn!(
                            upstream = %self.worker_url,
                            status = status.as_u16(),
                            attempt,
                            "idempotent request hit transient server error, retrying"
                        );
                        let backoff = std::time::Duration::from_millis(50 * (1 << attempt) + (rand::random::<u64>() % 50));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Ok(resp);
                }
                Ok(Err(e)) => {
                    // 建联失败（未发出数据）或幂等/带幂等键请求允许重试，无键 POST 不重试防幽灵扣费
                    let is_connect_err = e.is_connect();
                    if (is_connect_err || retryable_method) && attempt < max_attempts {
                        tracing::warn!(
                            upstream = %self.worker_url,
                            error = %e,
                            is_connect_err,
                            attempt,
                            "edge request transient error, retrying"
                        );
                        let backoff = std::time::Duration::from_millis(50 * (1 << attempt) + (rand::random::<u64>() % 50));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(e).context("edge request failed");
                }
                Err(_) => {
                    anyhow::bail!("edge request timeout after 120s");
                }
            }
        }
    }
}
