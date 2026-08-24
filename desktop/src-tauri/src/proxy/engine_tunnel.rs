//! WS 隧道客户端（M6 后续任务实现）：经 wss 连接 gate worker 中继 TLS 字节。
//!
//! 当前为占位：返回未实现错误——引擎在 tunnel_url 配置时对白名单流量调用
//! 此模块，收到 NotImplemented 即按 R4 语义关闭连接（无静默回落）。
//! 实现要点（spec §3）：wss + Bearer 首帧 JSON {"host","port"} → 二进制双向。

use tokio::net::TcpStream;

use super::engine::{EngineConfig, ReqHead};

pub async fn connect_and_relay(
    _client: TcpStream,
    _parsed: ReqHead,
    _head: &str,
    _cfg: &EngineConfig,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tunnel client not implemented yet (M6 task)",
    ))
}
