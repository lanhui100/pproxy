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
    /// 从环境变量装配：任一缺失/为空 → None（fail-closed，不猜默认）。
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("PPROXY_TUNNEL_GATE_URL").ok()?;
        let token = std::env::var("PPROXY_TUNNEL_TOKEN").ok()?;
        let url = url.trim().to_string();
        let token = token.trim().to_string();
        if url.is_empty() || token.is_empty() || !url.starts_with("wss://") && !url.starts_with("ws://") {
            return None;
        }
        Some(Self { url, token })
    }
}
