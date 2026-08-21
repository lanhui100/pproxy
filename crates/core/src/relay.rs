use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::time::Duration;
use tracing::debug;

pub async fn http_connect(
    upstream_addr: &str,
    target_host: &str,
    target_port: u16,
) -> anyhow::Result<TcpStream> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(upstream_addr),
    ).await??;

    let req = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
        target_host, target_port, target_host, target_port
    );
    stream.write_all(req.as_bytes()).await?;

    let mut buf = [0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(8), stream.read(&mut buf))
        .await?
        .unwrap_or(0);

    let resp = String::from_utf8_lossy(&buf[..n]);
    if resp.contains("200") {
        debug!("http_connect ok: {}:{} via {}", target_host, target_port, upstream_addr);
        Ok(stream)
    } else {
        anyhow::bail!("connect failed: {}", resp.lines().next().unwrap_or(""))
    }
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
