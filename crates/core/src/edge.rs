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

    pub async fn execute(&self, req: ForwardRequest) -> anyhow::Result<Response> {
        let is_idempotent = matches!(
            req.method,
            Method::GET | Method::HEAD | Method::OPTIONS
        );

        let max_attempts = if is_idempotent { 3 } else { 2 };
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
                    // 仅对幂等请求且处于 502/503/504 错误时在未超限时重试
                    if is_idempotent
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
                    // 仅在建联失败（未向网络发出数据）或幂等请求时允许重试，防非幂等 POST 幽灵扣费
                    let is_connect_err = e.is_connect();
                    if (is_connect_err || is_idempotent) && attempt < max_attempts {
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
