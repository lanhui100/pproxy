//! pproxy-server 库导出，供 CLI serve 与网关组件共享使用。

pub mod api;
pub mod connect;
pub mod dsk;
pub mod gateway;
pub mod monitor;
pub mod tunnel;

/// 回环地址判定：127.* / localhost / ::1 / [::1] 视为回环；空代表 0.0.0.0。
pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host == "[::1]" || host == "::1" {
        return true;
    }
    if host.is_empty() {
        return false;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}
