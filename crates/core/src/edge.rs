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
        let mut url = reqwest::Url::parse(&self.worker_url).context("invalid worker_url")?;
        url.query_pairs_mut().append_pair("url", &req.target_url);

        let mut builder = self
            .http
            .request(req.method, url)
            .header("X-Proxy-Secret", &self.secret)
            .headers(req.headers);

        if let Some(body) = req.body {
            builder = builder.body(body);
        }

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            builder.send(),
        )
        .await
        .context("edge request timeout")?
        .context("edge request failed")?;

        Ok(resp)
    }
}
