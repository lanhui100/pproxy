use std::collections::HashMap;

use pproxy_core::{EdgeClient, ForwardRequest, Pool, PoolConfig};
use reqwest::header::HeaderMap;
use reqwest::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, warn};

const MAX_HEAD_SIZE: usize = 64 * 1024;
const MAX_BODY_SIZE: usize = 32 * 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = load_config().await?;
    let addr: std::net::SocketAddr = format!("{}:{}", config.listen_host, config.listen_port).parse()?;

    let mut edges: HashMap<String, EdgeClient> = HashMap::new();
    if let (Some(url), Some(secret)) = (&config.worker_url, &config.worker_secret) {
        edges.insert("worker".into(), EdgeClient::new(url, secret)?);
    }
    for (name, up) in &config.upstreams {
        edges.insert(name.clone(), EdgeClient::new(&up.url, &up.secret)?);
    }
    if edges.is_empty() {
        warn!("no upstreams configured, edge forwarding disabled");
    }
    for (route, up) in &config.route_upstreams {
        info!("route {} -> upstream {}", route, up);
    }

    let pool = Pool::new(config.clone()).await;
    let routes = config.routes.clone();
    let route_upstreams = config.route_upstreams.clone();

    // stats API
    let pool3 = pool.clone();
    tokio::spawn(async move {
        let app = axum::Router::new()
            .route("/stats", axum::routing::get({
                let p = pool3.clone();
                move || {
                    let p = p.clone();
                    async move { axum::Json(p.stats().await) }
                }
            }))
            .route("/refresh", axum::routing::post({
                let p = pool3.clone();
                move || {
                    let p = p.clone();
                    async move { p.refresh().await; "ok" }
                }
            }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:8900").await.unwrap();
        axum::serve(l, app).await.unwrap();
    });

    let listener = TcpListener::bind(&addr).await?;
    info!("pproxy-server listening on {}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let edges = edges.clone();
        let routes = routes.clone();
        let route_upstreams = route_upstreams.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, pool, edges, routes, route_upstreams).await {
                warn!("client error: {}", e);
            }
        });
    }
}

async fn handle_client(
    mut stream: tokio::net::TcpStream,
    pool: Pool,
    edges: HashMap<String, EdgeClient>,
    routes: HashMap<String, String>,
    route_upstreams: HashMap<String, String>,
) -> anyhow::Result<()> {
    let (head, mut leftover) = read_request_head(&mut stream).await?;
    let head_str = String::from_utf8_lossy(&head);
    let mut lines = head_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.get(0).copied().unwrap_or("").to_ascii_uppercase();
    let target = parts.get(1).copied().unwrap_or("").to_string();

    let headers = parse_headers(&head_str);

    if method == "CONNECT" {
        let (host, port) = parse_host_port(&target)?;
        let upstream = connect_with_failover(&host, port, &pool).await
            .ok_or_else(|| anyhow::anyhow!("all upstreams failed"))?;
        stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        pproxy_core::relay::relay(stream, upstream).await;
        return Ok(());
    }

    // HTTP request -> edge gateway (worker) or local info
    let path = extract_path(&target)?;
    if path == "/" || path == "" {
        let body = serde_json::json!({
            "service": "pproxy-server",
            "routes": routes.keys().collect::<Vec<_>>(),
            "usage": "http://127.0.0.1:<port>/<route>/<path>",
        });
        let body = serde_json::to_vec(&body)?;
        write_response(&mut stream, 200, "OK", &[(b"content-type", b"application/json")], &body).await?;
        return Ok(());
    }

    let Some(edge) = pick_edge(&path, &edges, &route_upstreams) else {
        write_response(&mut stream, 503, "no upstream", &[], b"no upstream for route").await?;
        return Ok(());
    };

    let Some(target_url) = resolve_route(&path, &routes) else {
        write_response(&mut stream, 404, "unknown route", &[], format!("unknown route: {path}").as_bytes()).await?;
        return Ok(());
    };

    let req_method = Method::from_bytes(method.as_bytes())?;
    let body = read_body(&mut stream, &headers, &mut leftover).await?;
    let fwd = ForwardRequest {
        method: req_method,
        target_url,
        headers: EdgeClient::sanitize_headers(&headers),
        body: Some(body),
    };

    let resp = edge.execute(fwd).await?;

    // write status + headers
    let status = resp.status();
    let reason = status.canonical_reason().unwrap_or("");
    let mut head_out = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    for (k, v) in resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        if matches!(name.as_str(), "transfer-encoding" | "connection" | "content-length") {
            continue;
        }
        head_out.push_str(&format!("{}: {}\r\n", k, v.to_str().unwrap_or("")));
    }
    head_out.push_str("connection: close\r\n\r\n");
    stream.write_all(head_out.as_bytes()).await?;

    // stream body (SSE compatible)
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await? {
        stream.write_all(&chunk).await?;
    }
    Ok(())
}

async fn read_request_head(stream: &mut tokio::net::TcpStream) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    loop {
        if let Some(pos) = find_head_end(&buf) {
            return Ok((buf[..pos].to_vec(), buf[pos + 4..].to_vec()));
        }
        if buf.len() > MAX_HEAD_SIZE {
            anyhow::bail!("request head too large");
        }
        let n = tokio::time::timeout(std::time::Duration::from_secs(30), stream.read(&mut tmp)).await??;
        if n == 0 {
            anyhow::bail!("connection closed before request head complete");
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_headers(head: &str) -> HeaderMap {
    let mut map = HeaderMap::new();
    for line in head.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.trim().as_bytes()),
                reqwest::header::HeaderValue::from_bytes(v.trim().as_bytes()),
            ) {
                map.insert(name, val);
            }
        }
    }
    map
}

async fn read_body(
    stream: &mut tokio::net::TcpStream,
    headers: &HeaderMap,
    leftover: &mut Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let content_length = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);

    if content_length == 0 {
        return Ok(std::mem::take(leftover));
    }
    if content_length > MAX_BODY_SIZE {
        anyhow::bail!("request body too large");
    }

    let mut body = std::mem::take(leftover);
    while body.len() < content_length {
        let mut tmp = [0u8; 16384];
        let n = tokio::time::timeout(std::time::Duration::from_secs(60), stream.read(&mut tmp)).await??;
        if n == 0 {
            anyhow::bail!("connection closed before body complete");
        }
        body.extend_from_slice(&tmp[..n]);
    }
    if body.len() > content_length {
        // extra bytes belong to a pipelined request; drop for simplicity
        body.truncate(content_length);
    }
    Ok(body)
}

fn pick_edge(
    path: &str,
    edges: &HashMap<String, EdgeClient>,
    route_upstreams: &HashMap<String, String>,
) -> Option<EdgeClient> {
    let name = route_upstreams.get(path.trim_start_matches('/').split('/').next()?);
    let name = name.map(|s| s.as_str()).unwrap_or("worker");
    edges.get(name).cloned()
}

fn extract_path(target: &str) -> anyhow::Result<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        let url = reqwest::Url::parse(target)?;
        let path_query = match url.query() {
            Some(q) => format!("{}?{}", url.path(), q),
            None => url.path().to_string(),
        };
        return Ok(path_query);
    }
    Ok(target.to_string())
}

fn resolve_route(path: &str, routes: &HashMap<String, String>) -> Option<String> {
    let rest = path.trim_start_matches('/');
    let (name, tail) = match rest.split_once('/') {
        Some((n, t)) => (n, t),
        None => (rest, ""),
    };
    let host = routes.get(name)?;
    let tail = tail.split_once('?').map_or(tail, |(p, _)| p);
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    if query.is_empty() {
        Some(format!("https://{host}/{tail}"))
    } else {
        Some(format!("https://{host}/{tail}?{query}"))
    }
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    headers: &[(&[u8], &[u8])],
    body: &[u8],
) -> anyhow::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{}: {}\r\n", String::from_utf8_lossy(k), String::from_utf8_lossy(v)));
    }
    head.push_str(&format!("content-length: {}\r\n", body.len()));
    head.push_str("connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

async fn connect_with_failover(host: &str, port: u16, pool: &Pool) -> Option<tokio::net::TcpStream> {
    if let Some(addr) = pool.next().await {
        if let Ok(s) = pproxy_core::relay::http_connect(&addr, host, port).await {
            return Some(s);
        }
        pool.mark_failed(&addr).await;
    }
    // pool exhausted/empty -> direct connect
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .ok()?
    .ok()
}

fn parse_host_port(target: &str) -> anyhow::Result<(String, u16)> {
    let mut parts = target.split(':');
    let host = parts.next().unwrap_or("").to_string();
    let port: u16 = parts.next().unwrap_or("443").parse()?;
    Ok((host, port))
}

async fn load_config() -> anyhow::Result<PoolConfig> {
    let path = std::env::var("PPROXY_CONFIG")
        .unwrap_or_else(|_| "/home/USER/pproxy/config.json".into());
    if std::path::Path::new(&path).exists() {
        let data = tokio::fs::read_to_string(&path).await?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(PoolConfig::default())
    }
}
