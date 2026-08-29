//! 隧道中继下发配置（桌面端「自动配置」数据源）。
//!
//! 来源：环境变量注入（与 monitor 同策略，config.json 零改动）——
//! - `PPROXY_TUNNEL_GATE_URL`：gate worker 的 wss:// 端点
//! - `PPROXY_TUNNEL_TOKEN`：gate 侧 TUNNEL_TOKEN_HASH 对应的明文令牌
//!
//! 安全边界：二者经管理面 admin 鉴权后下发（与设备密钥同通道），
//! 不做任何编译期/默认值兜底——未配置即下发 null，由前端引导手动配置。

/// 一组完整的隧道下发配置（url 与 token 必须同时存在才构造成功）。
#[derive(Debug, Clone)]
pub struct TunnelProvision {
    pub url: String,
    pub token: String,
}

impl TunnelProvision {
    /// 从环境变量装配：任一缺失/为空 → 兜底从 PoolConfig 推导。
    #[allow(dead_code)]
    pub fn from_env() -> Option<Self> {
        Self::from_pool_config_and_env(&pproxy_core::PoolConfig::default())
    }

    /// 智能推导装配：优先读取环境变量，缺省由 PoolConfig 自动派生。
    pub fn from_pool_config_and_env(pool_config: &pproxy_core::PoolConfig) -> Option<Self> {
        let url = std::env::var("PPROXY_TUNNEL_GATE_URL")
            .ok()
            .or_else(|| pool_config.worker_url.as_deref().and_then(crate::connect::derive_gate_url_from_worker))?;
        let token = std::env::var("PPROXY_TUNNEL_TOKEN")
            .ok()
            .or_else(|| pool_config.worker_secret.clone())?;
        let url = url.trim().to_string();
        let token = token.trim().to_string();
        if url.is_empty() || token.is_empty() || (!url.starts_with("wss://") && !url.starts_with("ws://")) {
            return None;
        }
        Some(Self { url, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_provision_from_pool_config() {
        let pool_cfg = pproxy_core::PoolConfig {
            worker_url: Some("https://edge.ponyjob.top".into()),
            worker_secret: Some("sec123".into()),
            ..Default::default()
        };
        let p = TunnelProvision::from_pool_config_and_env(&pool_cfg).unwrap();
        assert_eq!(p.url, "wss://edge.ponyjob.top/ws");
        assert_eq!(p.token, "sec123");
    }
}
