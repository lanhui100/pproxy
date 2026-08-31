//! 嵌入式网关服务器运行时（Hyper 1.x + Axum 0.7 + CONNECT 裸 TCP 分流）。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::Router;
use rand::RngCore;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::connect::handle_connect_raw;
use crate::gateway::{build_data_router, GatewayState};

pub const MAX_CONCURRENT_CONNECTIONS: usize = 512;

/// 生成 32 字符的唯一实例 UUID（防端口假活握手探测）。
pub fn generate_instance_uuid() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 引擎运行配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub listen_addr: String,
    pub instance_uuid: String,
    pub max_connections: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:8899".to_string(),
            instance_uuid: generate_instance_uuid(),
            max_connections: MAX_CONCURRENT_CONNECTIONS,
        }
    }
}

/// 启动嵌入式网关主循环。
pub async fn run_engine(
    config: EngineConfig,
    state: GatewayState,
    mut shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        addr = %local_addr,
        instance_uuid = %config.instance_uuid,
        "Pony Proxy embedded engine started successfully"
    );

    let router = build_data_router(state.clone());
    let sem = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let accept_res = if let Some(rx) = shutdown_rx.as_mut() {
            tokio::select! {
                res = listener.accept() => res,
                _ = rx.changed() => {
                    tracing::info!("Engine: received shutdown signal, draining...");
                    break;
                }
            }
        } else {
            listener.accept().await
        };

        let (stream, client_addr) = match accept_res {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "engine accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        // 入站 socket 禁用 Nagle（hyper 手动 serve_connection 不设 nodelay），
        // 消除小包响应 40ms+ 的 delayed-ACK 等待。
        let _ = stream.set_nodelay(true);

        let permit = match sem.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(%client_addr, "engine connection limit reached, dropping");
                drop(stream);
                continue;
            }
        };

        let router_clone = router.clone();
        let state_clone = state.clone();

        tokio::spawn(async move {
            let _permit = permit;
            handle_conn(stream, client_addr, router_clone, state_clone).await;
        });
    }

    Ok(())
}

async fn handle_conn(
    mut stream: tokio::net::TcpStream,
    client_addr: SocketAddr,
    router: Router,
    state: GatewayState,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut peek_buf = [0u8; 16];
    let n = match stream.peek(&mut peek_buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let first = &peek_buf[..n];
    if first.len() >= 7 && first[..7].eq_ignore_ascii_case(b"CONNECT") {
        // CONNECT 隧道：读取全头并交给 handle_connect_raw 校验鉴权
        let mut buf = Vec::with_capacity(1024);
        let mut tmp = [0u8; 2048];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let _ = stream.shutdown().await;
                return;
            }
            match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
                Ok(Ok(0)) => return,
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                        break;
                    }
                }
                _ => return,
            }
        }

        let split_pos = buf.windows(4).position(|w| w == b"\r\n\r\n");
        let (head_bytes, leftover) = match split_pos {
            Some(pos) => (&buf[..pos + 4], buf[pos + 4..].to_vec()),
            None => (&buf[..], Vec::new()),
        };
        let head = String::from_utf8_lossy(head_bytes);

        handle_connect_raw(
            client_addr.ip(),
            &head,
            leftover,
            stream,
            state.tunnel,
            state.users,
            state.tokens,
            state.gatekeeper,
        )
        .await;
        return;
    }

    // 普通 HTTP 请求
    let adapter = RouterHyperAdapter {
        router,
        client_addr,
    };
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Some(Duration::from_secs(30)))
        .keep_alive(true)
        .serve_connection(hyper_util::rt::TokioIo::new(stream), adapter)
        .await;
}

struct RouterHyperAdapter {
    router: Router,
    client_addr: SocketAddr,
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for RouterHyperAdapter {
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let mut axum_req = req.map(Body::new);
        axum_req.extensions_mut().insert(self.client_addr);
        let mut router = self.router.clone();
        Box::pin(async move {
            let res = tower::Service::call(&mut router, axum_req).await;
            match res {
                Ok(r) => Ok(r),
                Err(_) => Ok(axum::response::IntoResponse::into_response(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                )),
            }
        })
    }
}
