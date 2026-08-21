use crate::ProxyRecord;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::time::Duration;
use tracing::debug;

pub async fn verify_batch(
    candidates: &[ProxyRecord],
    concurrency: usize,
) -> Vec<ProxyRecord> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut handles = Vec::new();

    for c in candidates {
        let sem = sem.clone();
        let c = c.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            verify_one(&c).await
        }));
    }

    let mut verified = Vec::new();
    for h in handles {
        if let Ok(Some(r)) = h.await {
            verified.push(r);
        }
    }
    verified
}

async fn verify_one(p: &ProxyRecord) -> Option<ProxyRecord> {
    let addr = p.addr();

    let mut stream = match tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(&addr),
    ).await {
        Ok(Ok(s)) => s,
        _ => return None,
    };

    let connect_req = format!(
        "CONNECT ipinfo.io:80 HTTP/1.1\r\nHost: ipinfo.io:80\r\n\r\n"
    );
    if stream.write_all(connect_req.as_bytes()).await.is_err() {
        return None;
    }

    let mut buf = [0u8; 1024];
    let n = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return None,
    };

    let resp = String::from_utf8_lossy(&buf[..n]);
    if !resp.contains("200") {
        return None;
    }

    // Send HTTP request through tunnel
    let http_req = b"GET /json HTTP/1.1\r\nHost: ipinfo.io\r\nConnection: close\r\n\r\n";
    if stream.write_all(http_req).await.is_err() {
        return None;
    }

    let mut body = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut body)).await;
    let body_str = String::from_utf8_lossy(&body);

    if let Ok(j) = serde_json::from_str::<serde_json::Value>(&body_str) {
        let country = j["country"].as_str().unwrap_or("").to_string();
        // allow empty country (mark as unknown)
        let mut out = p.clone();
        out.country = if country.is_empty() { None } else { Some(country) };
        out.last_verified = Some(now_secs());
        debug!("verified: {} ({})", addr, out.country.as_deref().unwrap_or("unknown"));
        Some(out)
    } else {
        None
    }
}

#[inline]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
