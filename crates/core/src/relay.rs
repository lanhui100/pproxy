use anyhow::Context;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::time::Duration;
use tracing::debug;

pub async fn http_connect_ext(
    upstream_addr: &str,
    target_host: &str,
    target_port: u16,
) -> anyhow::Result<(TcpStream, Vec<u8>)> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(upstream_addr),
    ).await??;

    let req = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
        target_host, target_port, target_host, target_port
    );
    stream.write_all(req.as_bytes()).await?;

    let mut header_buf = Vec::with_capacity(1024);
    let mut temp = [0u8; 512];
    let delimiter = b"\r\n\r\n";

    // 循环读满完整的 HTTP 响应头，杜绝分包导致握手失败
    let end = loop {
        let n = tokio::time::timeout(Duration::from_secs(8), stream.read(&mut temp))
            .await?
            .context("upstream closed connection during CONNECT handshake")?;
        if n == 0 {
            anyhow::bail!("unexpected EOF during CONNECT handshake");
        }
        header_buf.extend_from_slice(&temp[..n]);

        if let Some(pos) = header_buf.windows(4).position(|w| w == delimiter) {
            break pos + 4;
        }

        if header_buf.len() > 16 * 1024 {
            anyhow::bail!("CONNECT response header too large");
        }
    };

    let header_bytes = &header_buf[..end];
    let leftover = header_buf[end..].to_vec();

    let header_str = String::from_utf8_lossy(header_bytes);
    let first_line = header_str.lines().next().unwrap_or("");

    // 精确解析状态码（200..=299），杜绝子串 200 误判
    let mut parts = first_line.split_whitespace();
    let _proto = parts.next();
    let status_code: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if (200..=299).contains(&status_code) {
        debug!("http_connect ok: {}:{} via {} (status {})", target_host, target_port, upstream_addr, status_code);
        Ok((stream, leftover))
    } else {
        anyhow::bail!("connect failed: {}", first_line);
    }
}

pub async fn http_connect(
    upstream_addr: &str,
    target_host: &str,
    target_port: u16,
) -> anyhow::Result<TcpStream> {
    let (stream, _) = http_connect_ext(upstream_addr, target_host, target_port).await?;
    Ok(stream)
}

pub async fn socks5_connect(
    upstream_addr: &str,
    target_host: &str,
    target_port: u16,
) -> anyhow::Result<TcpStream> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(upstream_addr),
    ).await??;

    // SOCKS5 greeting
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await?;
    if buf[0] != 0x05 || buf[1] != 0x00 {
        anyhow::bail!("socks5 greeting failed");
    }

    // Connect request (domain name)
    let host_bytes = target_host.as_bytes();
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&req).await?;

    let mut resp = [0u8; 4];
    stream.read_exact(&mut resp).await?;
    if resp[1] != 0x00 {
        anyhow::bail!("socks5 connect failed: REP={}", resp[1]);
    }

    debug!("socks5_connect ok: {}:{} via {}", target_host, target_port, upstream_addr);
    Ok(stream)
}

pub async fn relay_with_leftover(mut client: TcpStream, upstream: TcpStream, leftover: Vec<u8>) {
    // 将握手粘包多读的 Early Data 在全双工中继前单次 flush 给下游，保证数据零丢失零封装开销
    if !leftover.is_empty() {
        if let Err(e) = client.write_all(&leftover).await {
            debug!("failed to flush leftover bytes to client: {e}");
            return;
        }
    }
    relay(client, upstream).await;
}

pub async fn relay(client: TcpStream, upstream: TcpStream) {
    let (mut cr, mut cw) = client.into_split();
    let (mut ur, mut uw) = upstream.into_split();

    let c2u = tokio::spawn(async move {
        tokio::io::copy(&mut cr, &mut uw).await.ok();
        uw.shutdown().await.ok();
    });
    let u2c = tokio::spawn(async move {
        tokio::io::copy(&mut ur, &mut cw).await.ok();
        cw.shutdown().await.ok();
    });

    let _ = tokio::join!(c2u, u2c);
}
