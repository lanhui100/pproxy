//! 目标域名感知与端点智能排序策略。

/// 出口归账口径：gate 端点域名含 vercel/vgate → Vercel 出口，含 searchxai/rn. → NativeVps，其余（gate.example.com 等）→ CF。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Egress {
    Cf,
    Vercel,
    NativeVps,
}

pub fn classify_egress(url: &str) -> Egress {
    if is_native_vps_endpoint(url) {
        Egress::NativeVps
    } else if is_vercel_endpoint(url) {
        Egress::Vercel
    } else {
        Egress::Cf
    }
}

/// 端点是否属于原生独立 VPS 类出口（如 RackNerd VPS，拥有原生独立美区 IP 且无时长/配额约束）。
pub fn is_native_vps_endpoint(url: &str) -> bool {
    url.contains("searchxai") || url.contains("rn.") || url.contains("rn.example.com") || url.contains("192.210.231.8")
}

/// Google 核心 API 与基础子域
const GOOGLE_BASE_SUFFIXES: &[&str] = &[
    "googleapis.com",
    "gstatic.com",
    "googleusercontent.com",
    "deepmind.google",
    "antigravity.google",
    "labs.google",
    "g.co",
    "goog",
];

/// CF Workers 平台限制直连的目标（OpenAI / Anthropic 必须走 Vercel 美区物理出口）
const AI_TARGET_SUFFIXES: &[&str] = &[
    "openai.com",
    "chatgpt.com",
    "oaistatic.com",
    "oaiusercontent.com",
    "anthropic.com",
    "claude.ai",
];

/// 判定目标域名是否为 Google 系或平台受限的 AI 站点：
/// 1. Google 核心域名（含全球国别域与 agy 域名）；
/// 2. OpenAI / Anthropic 域名。
pub fn is_google_or_ai_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    // 1. Google 核心 API 与子域
    if GOOGLE_BASE_SUFFIXES.iter().any(|s| {
        if !h.ends_with(s) { return false; }
        let rest = &h[..h.len() - s.len()];
        rest.is_empty() || rest.ends_with('.')
    }) {
        return true;
    }
    // 2. Google 国别域 (google.com, google.com.hk, google.co.jp, google.de 等)
    if h == "google.com" || h.ends_with(".google.com")
        || h.starts_with("google.") || h.contains(".google.")
    {
        return true;
    }
    // 3. 受限 AI 站点
    if AI_TARGET_SUFFIXES.iter().any(|s| {
        if !h.ends_with(s) { return false; }
        let rest = &h[..h.len() - s.len()];
        rest.is_empty() || rest.ends_with('.')
    }) {
        return true;
    }
    false
}

#[inline]
pub fn is_google_host(host: &str) -> bool {
    is_google_or_ai_host(host)
}

/// 判定目标是否属于被 Cloudflare 平台硬性拉黑的受限 AI 站点（如 OpenAI/ChatGPT/Anthropic）。
pub fn is_strict_ai_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    AI_TARGET_SUFFIXES.iter().any(|s| {
        if !h.ends_with(s) { return false; }
        let rest = &h[..h.len() - s.len()];
        rest.is_empty() || rest.ends_with('.')
    })
}

/// 必须走"合规物理出口"的目标（Google Cloud Code / Code Assist 系）。
///
/// 这些接口按**请求来源 IP**做地区限制，来源落在不受支持地区时返回
/// `HTTP 400 FAILED_PRECONDITION: User location is not supported for the API use.`。
/// CF gate 的出口是轮换的 Cloudflare anycast IP 池（AS13335），地理归属不稳定，
/// 因此这三个 host 必须优先走真实机房出口（Vercel iad1 / 独立 VPS），
/// 与 `PPROXY_CONSERVE_VERCEL` 的省额度策略无关。
///
/// 与 `deploy/cf-gate-worker/gate-policy.mjs` 的 `COMPLIANT_EGRESS_SUFFIXES` 保持一致。
const COMPLIANT_EGRESS_SUFFIXES: &[&str] = &[
    "daily-cloudcode-pa.googleapis.com",
    "cloudcode-pa.googleapis.com",
    "cloudaicompanion.googleapis.com",
];

/// 目标是否属于"必须合规出口"的 host（后缀匹配，dot-boundary；容忍 FQDN 末尾点）。
pub fn requires_compliant_egress(host: &str) -> bool {
    let lowered = host.trim().to_ascii_lowercase();
    let h = lowered.trim_end_matches('.');
    if h.is_empty() {
        return false;
    }
    COMPLIANT_EGRESS_SUFFIXES.iter().any(|s| {
        if !h.ends_with(s) {
            return false;
        }
        let rest = &h[..h.len() - s.len()];
        rest.is_empty() || rest.ends_with('.')
    })
}

/// 端点优先级重排：Google/AI host → Vercel 合规出口优先；其他 host → CF 低延迟优先。
///
/// 额度保护机制（2026-09 专项）：默认开启节能模式（conserve_vercel = true），
/// 仅针对被 CF 平台严格拉黑的 AI 目标（`is_strict_ai_host`）优先分配 Vercel 出口；
/// 泛 Google 与常规大流量默认走 CF gate（CF 免费版无出站带宽上限且具备智能 Colo 守卫），
/// 避免普通网页/日常大流量刷爆 Vercel 10GB 出口流量与 CPU 配额。
/// 若显式设置环境变量 `PPROXY_CONSERVE_VERCEL=0`，则回退为宽松模式（所有 Google/AI 均优先 Vercel）。
///
/// **例外（2026-09-08，地区限制专项）**：`requires_compliant_egress` 的 host
/// 始终优先非 CF 出口——省额度不能以牺牲可用性为代价，
/// 且该例外只覆盖 3 个 Cloud Code host，Vercel 流量开销可控。
pub fn order_endpoints<'a>(urls: Vec<&'a str>, host: &str) -> Vec<&'a str> {
    let conserve_vercel = std::env::var("PPROXY_CONSERVE_VERCEL").ok().as_deref() != Some("0");
    order_endpoints_with(urls, host, conserve_vercel)
}

/// `order_endpoints` 的纯函数内核（不读环境变量，便于确定性单测）。
pub fn order_endpoints_with<'a>(
    urls: Vec<&'a str>,
    host: &str,
    conserve_vercel: bool,
) -> Vec<&'a str> {
    order_endpoints_with_offset(urls, host, conserve_vercel, 0)
}

/// 支持外部传入偏移量以支持单测确定性或多端点轮询
pub fn order_endpoints_with_offset<'a>(
    urls: Vec<&'a str>,
    host: &str,
    conserve_vercel: bool,
    offset: usize,
) -> Vec<&'a str> {
    let vercel_preferred = if requires_compliant_egress(host) {
        true
    } else if conserve_vercel {
        is_strict_ai_host(host)
    } else {
        is_google_or_ai_host(host)
    };

    let mut vps_list = Vec::new();
    let mut vercel_list = Vec::new();
    let mut cf_list = Vec::new();

    for u in urls {
        if is_native_vps_endpoint(u) {
            vps_list.push(u);
        } else if is_vercel_endpoint(u) {
            vercel_list.push(u);
        } else {
            cf_list.push(u);
        }
    }

    // 负载均衡轮询偏移
    if vps_list.len() > 1 && offset > 0 {
        let rot = offset % vps_list.len();
        vps_list.rotate_left(rot);
    }
    if vercel_list.len() > 1 && offset > 0 {
        let rot = offset % vercel_list.len();
        vercel_list.rotate_left(rot);
    }
    if cf_list.len() > 1 && offset > 0 {
        let rot = offset % cf_list.len();
        cf_list.rotate_left(rot);
    }

    let mut list = Vec::new();
    // 核心提速原则：只要配置了原生 VPS（出口R），其为拥有独立美区原生 IP、无冷启动、无时长限制的最佳出海口，
    // 始终占据绝对最高优先级（Top 1）！
    list.extend(vps_list);

    if vercel_preferred {
        list.extend(vercel_list);
        list.extend(cf_list);
    } else {
        list.extend(cf_list);
        list.extend(vercel_list);
    }
    list
}

static GATE_URL_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 把逗号分隔的 gate 端点串按目标 host 重排为有序列表（数据面装配入口）。
///
/// 数据面（`crates/server` 与 `crates/engine` 两份 CONNECT 实现）此前各自按
/// 配置顺序建连，`order_endpoints` 只被桌面端调用——导致 agy 的 Cloud Code 请求
/// 固定先落 CF 出口。统一走本函数，避免再次分叉。
pub fn ordered_gate_urls(gate_url: &str, host: &str) -> Vec<String> {
    let urls: Vec<&str> = gate_url
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let offset = GATE_URL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let conserve_vercel = std::env::var("PPROXY_CONSERVE_VERCEL").ok().as_deref() != Some("0");
    order_endpoints_with_offset(urls, host, conserve_vercel, offset)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// 端点是否属于 Vercel 类出口（与 [`classify_egress`] 同判据，供
/// 合规出口过滤复用——三处数据面实现（server/engine/desktop）必须共用
/// 同一个判定，避免再分叉出第四种 URL 匹配语义）。
pub fn is_vercel_endpoint(url: &str) -> bool {
    url.contains("vercel") || url.contains("vgate") || url.contains("/api/ws")
}

/// 合规出口 host（`requires_compliant_egress` 为真）可用的端点列表：
/// **仅 Vercel 类**，过滤掉 CF（CF 出口是轮换 anycast，地理归属不稳定，
/// 被 Google 按来源 IP 判区后返回 `400 FAILED_PRECONDITION`）。
///
/// 返回空列表 = 配置里没有任何 Vercel 端点。调用方应 fail-closed
/// （直接 502 并告警），**不得**降级回 CF——降级回去客户端仍收到
/// Google 400，与故障现场不可区分，只会掩盖配置错误。
pub fn compliant_egress_endpoints<'a>(ordered: &[&'a str]) -> Vec<&'a str> {
    ordered
        .iter()
        .copied()
        .filter(|u| is_native_vps_endpoint(u) || is_vercel_endpoint(u))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_google_or_ai_host() {
        for h in [
            "google.com",
            "www.google.com",
            "google.com.hk",
            "www.google.co.jp",
            "accounts.google.com",
            "generativelanguage.googleapis.com",
            "antigravity.google",
            "api.antigravity.google",
            "openai.com",
            "api.openai.com",
            "chatgpt.com",
            "claude.ai",
            "anthropic.com",
        ] {
            assert!(is_google_or_ai_host(h), "应识别为 Google/AI: {h}");
        }

        for h in ["github.com", "youtube.com", "baidu.com", "notgoogleapis.com"] {
            assert!(!is_google_or_ai_host(h), "不应识别为 Google/AI: {h}");
        }
    }

    #[test]
    fn test_order_endpoints() {
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];
        let ordered = order_endpoints_with(urls.clone(), "google.com.hk", false);
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws");

        let ordered = order_endpoints_with(urls.clone(), "api.openai.com", false);
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws");

        let ordered = order_endpoints_with(urls.clone(), "github.com", false);
        assert_eq!(ordered[0], "wss://gate.example.com/ws");
    }

    #[test]
    fn test_requires_compliant_egress() {
        for h in [
            "daily-cloudcode-pa.googleapis.com",
            "cloudcode-pa.googleapis.com",
            "cloudaicompanion.googleapis.com",
            "DAILY-CLOUDCODE-PA.GOOGLEAPIS.COM",
            "daily-cloudcode-pa.googleapis.com.",
        ] {
            assert!(requires_compliant_egress(h), "应要求合规出口: {h}");
        }

        // 认证类与泛 Google 不要求合规出口（OAuth/userinfo 不受地区限制）
        for h in [
            "oauth2.googleapis.com",
            "accounts.google.com",
            "www.googleapis.com",
            "generativelanguage.googleapis.com",
            "cloudcode-pa.googleapis.com.evil.cn",
            "github.com",
            "",
        ] {
            assert!(!requires_compliant_egress(h), "不应要求合规出口: {h}");
        }
    }

    #[test]
    fn test_order_endpoints_compliant_egress_beats_conserve() {
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];

        // 省额度模式下仍必须优先合规出口（本专项的核心承诺）
        for h in [
            "daily-cloudcode-pa.googleapis.com",
            "cloudcode-pa.googleapis.com",
            "cloudaicompanion.googleapis.com",
        ] {
            let ordered = order_endpoints_with(urls.clone(), h, true);
            assert_eq!(
                ordered[0], "wss://vgate.example.com/api/ws",
                "省额度模式下 {h} 仍应优先合规出口"
            );
            assert_eq!(ordered[1], "wss://gate.example.com/ws", "{h} 需保留 CF 兜底");
        }

        // 认证类不受影响：省额度模式下继续 CF 优先
        let ordered = order_endpoints_with(urls.clone(), "oauth2.googleapis.com", true);
        assert_eq!(ordered[0], "wss://gate.example.com/ws");
    }

    #[test]
    fn test_ordered_gate_urls() {
        let gate = "wss://gate.example.com/ws , wss://vgate.example.com/api/ws";
        let ordered = ordered_gate_urls(gate, "daily-cloudcode-pa.googleapis.com");
        assert_eq!(
            ordered,
            vec![
                "wss://vgate.example.com/api/ws".to_string(),
                "wss://gate.example.com/ws".to_string(),
            ]
        );

        let ordered = ordered_gate_urls(gate, "github.com");
        assert_eq!(ordered[0], "wss://gate.example.com/ws");
        // 空串/纯空白端点被过滤，不产生空端点
        assert!(ordered_gate_urls(" , ", "github.com").is_empty());
    }

    #[test]
    fn test_is_strict_ai_host() {
        assert!(is_strict_ai_host("openai.com"));
        assert!(is_strict_ai_host("api.openai.com"));
        assert!(is_strict_ai_host("chatgpt.com"));
        assert!(is_strict_ai_host("claude.ai"));
        assert!(is_strict_ai_host("anthropic.com"));

        // Google 域名属于宽泛 Google/AI，但不属于严格 CF 平台拉黑目标
        assert!(!is_strict_ai_host("google.com"));
        assert!(!is_strict_ai_host("google.com.hk"));
        assert!(!is_strict_ai_host("generativelanguage.googleapis.com"));
        assert!(!is_strict_ai_host("github.com"));
    }

    #[test]
    fn test_order_endpoints_conserve_vercel() {
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];

        // 节能模式下：Google 优先走 CF gate
        let ordered = order_endpoints_with(urls.clone(), "google.com.hk", true);
        assert_eq!(ordered[0], "wss://gate.example.com/ws");

        // 节能模式下：严格受限 AI 站点依然优先走 Vercel
        let ordered = order_endpoints_with(urls.clone(), "api.openai.com", true);
        assert_eq!(ordered[0], "wss://vgate.example.com/api/ws");
    }

    #[test]
    fn test_order_endpoints_round_robin_offset() {
        let urls = vec![
            "wss://vgate1.example.com/api/ws",
            "wss://vgate2.example.com/api/ws",
            "wss://cf-gate.example.com/ws",
        ];
        // offset 0: vgate1 优先
        let ord0 = order_endpoints_with_offset(urls.clone(), "api.openai.com", true, 0);
        assert_eq!(ord0[0], "wss://vgate1.example.com/api/ws");
        assert_eq!(ord0[1], "wss://vgate2.example.com/api/ws");
        assert_eq!(ord0[2], "wss://cf-gate.example.com/ws");

        // offset 1: vgate2 优先（轮询）
        let ord1 = order_endpoints_with_offset(urls.clone(), "api.openai.com", true, 1);
        assert_eq!(ord1[0], "wss://vgate2.example.com/api/ws");
        assert_eq!(ord1[1], "wss://vgate1.example.com/api/ws");
        assert_eq!(ord1[2], "wss://cf-gate.example.com/ws");
    }

    #[test]
    fn test_order_endpoints_reads_env_boundary() {
        // 默认情况下（未设置环境变量）应为节能模式
        let urls = vec![
            "wss://gate.example.com/ws",
            "wss://vgate.example.com/api/ws",
        ];
        let ordered_default = order_endpoints(urls.clone(), "google.com.hk");
        assert_eq!(ordered_default[0], "wss://gate.example.com/ws");

        // 显式设为 0 时回退为宽松模式
        std::env::set_var("PPROXY_CONSERVE_VERCEL", "0");
        let ordered_loose = order_endpoints(urls.clone(), "google.com.hk");
        assert_eq!(ordered_loose[0], "wss://vgate.example.com/api/ws");
        std::env::remove_var("PPROXY_CONSERVE_VERCEL");
    }

    #[test]
    fn test_is_vercel_endpoint() {
        assert!(is_vercel_endpoint("wss://vgate.ponyjob.top/api/ws"));
        assert!(is_vercel_endpoint("wss://vercel.example.com/ws"));
        assert!(is_vercel_endpoint("ws://127.0.0.1:9000/api/ws"));
        assert!(!is_vercel_endpoint("wss://gate.ponyjob.top/ws"));
        // vedge（openai 数据面上游）不含 vercel/vgate//api/ws → 按 classify_egress 口径为 Cf
        assert!(!is_vercel_endpoint("wss://vedge.ponyjob.top/api/proxy"));
        assert!(!is_vercel_endpoint("ws://127.0.0.1:9000/ws"));
    }

    #[test]
    fn test_compliant_egress_endpoints_filters_to_vercel_only() {
        let ordered = [
            "wss://gate.ponyjob.top/ws",
            "wss://vgate.ponyjob.top/api/ws",
        ];
        let only = compliant_egress_endpoints(&ordered);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0], "wss://vgate.ponyjob.top/api/ws");

        // 无 Vercel 端点 → 空列表（调用方应 fail-closed）
        let cf_only = ["wss://gate.ponyjob.top/ws"];
        assert!(compliant_egress_endpoints(&cf_only).is_empty());

        // 空输入 → 空
        assert!(compliant_egress_endpoints(&[]).is_empty());
    }
}
