//! 目标域名感知与端点智能排序策略。

/// 出口归账口径：gate 端点域名含 vercel/vgate → Vercel 出口，其余（gate.ponyjob.top 等）→ CF。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Egress {
    Cf,
    Vercel,
}

pub fn classify_egress(url: &str) -> Egress {
    if url.contains("vercel") || url.contains("vgate") || url.contains("/api/ws") {
        Egress::Vercel
    } else {
        Egress::Cf
    }
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

/// 端点优先级重排：Google/AI host → Vercel 合规出口优先；其他 host → CF 低延迟优先。
pub fn order_endpoints<'a>(urls: Vec<&'a str>, host: &str) -> Vec<&'a str> {
    let vercel_preferred = is_google_or_ai_host(host);
    let mut list = urls;
    list.sort_by_key(|u| {
        let is_vercel = u.contains("vercel") || u.contains("vgate") || u.contains("/api/ws");
        match (vercel_preferred, is_vercel) {
            (true, true) => 0,   // Google/AI + Vercel 最优先
            (true, false) => 1,  // Google/AI + CF 兜底
            (false, true) => 1,  // 其他 + Vercel 兜底
            (false, false) => 0, // 其他 + CF 最优先
        }
    });
    list
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
            "wss://gate.ponyjob.top/ws",
            "wss://vgate.ponyjob.top/api/ws",
        ];
        let ordered = order_endpoints(urls.clone(), "google.com.hk");
        assert_eq!(ordered[0], "wss://vgate.ponyjob.top/api/ws");

        let ordered = order_endpoints(urls.clone(), "api.openai.com");
        assert_eq!(ordered[0], "wss://vgate.ponyjob.top/api/ws");

        let ordered = order_endpoints(urls.clone(), "github.com");
        assert_eq!(ordered[0], "wss://gate.ponyjob.top/ws");
    }
}
