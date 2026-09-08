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

/// 判定是否应对指定名称的上游边缘执行主动连接池保活（PPROXY_EDGE_KEEPALIVE）。
///
/// 核心风控（2026-09 Vercel 额度防超限）：Vercel Serverless Function 每次 HTTP ping 计一次完整函数调用
/// （45s 间隔约 1920 次/日，按月计 ~5.7 万次调用），持续空耗 Fluid Active CPU 与 Provisioned Memory；
/// 且 Vercel 无跨洲多 RTT 冷握手顾虑（AWS 节点），严禁对其主动保活。
/// Cloudflare Worker（"worker" / "cf"）处于边缘且调用计费宽松，保留保活。
pub fn should_keepalive_edge(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(lower.contains("vercel") || lower.contains("vgate") || lower.contains("vedge"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_keepalive_edge() {
        // CF / 普通 Worker 允许保活
        assert!(should_keepalive_edge("worker"));
        assert!(should_keepalive_edge("cf"));
        assert!(should_keepalive_edge("gate"));
        assert!(should_keepalive_edge("custom-proxy"));

        // Vercel / vgate / vedge 严格禁止保活，防止烧函数调用指标
        assert!(!should_keepalive_edge("vercel"));
        assert!(!should_keepalive_edge("Vercel"));
        assert!(!should_keepalive_edge("vgate"));
        assert!(!should_keepalive_edge("vedge"));
        assert!(!should_keepalive_edge("my-vercel-upstream"));
    }
}
