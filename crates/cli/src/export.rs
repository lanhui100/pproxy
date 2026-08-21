//! 配置导出模板（M2 §4.7）：service → env 片段。纯函数，无 IO。

/// 模板表条目。
pub struct ServiceTemplate {
    pub name: &'static str,
    /// 默认路由名（= service 名，`--route` 可覆盖）
    pub env_prefix: EnvPrefix,
}

/// env 前缀三态：有标准约定的全量导出；无约定仅 BASE_URL 注释示例。
#[derive(Clone, Copy, PartialEq)]
pub enum EnvPrefix {
    Anthropic,
    Openai,
    Opencode,
    CommentOnly,
}

/// 内置模板表（M2 §4.7）。
pub const TEMPLATES: &[ServiceTemplate] = &[
    ServiceTemplate { name: "anthropic", env_prefix: EnvPrefix::Anthropic },
    ServiceTemplate { name: "openai", env_prefix: EnvPrefix::Openai },
    ServiceTemplate { name: "opencode", env_prefix: EnvPrefix::Opencode },
    ServiceTemplate { name: "google", env_prefix: EnvPrefix::CommentOnly },
    ServiceTemplate { name: "github", env_prefix: EnvPrefix::CommentOnly },
    ServiceTemplate { name: "x", env_prefix: EnvPrefix::CommentOnly },
    ServiceTemplate { name: "facebook", env_prefix: EnvPrefix::CommentOnly },
];

/// 查模板；None = 未知 service（调用方列出可用项退出 1）。
pub fn lookup(service: &str) -> Option<&'static ServiceTemplate> {
    TEMPLATES.iter().find(|t| t.name == service)
}

/// 渲染 env 片段。
///
/// - `token`: Some(明文) 嵌入输出；None 用占位 `<your-pony-token-here>`
/// - `data_plane_base`: 形如 `http://127.0.0.1:8899`
pub fn render(
    tpl: &ServiceTemplate,
    route: &str,
    token: Option<&str>,
    data_plane_base: &str,
) -> String {
    let token = token.unwrap_or("<your-pony-token-here>");
    let base_url = format!("{data_plane_base}/{token}/{route}");
    match tpl.env_prefix {
        EnvPrefix::Anthropic => format!(
            "# pony proxy — anthropic\nexport ANTHROPIC_BASE_URL={base_url}\nexport ANTHROPIC_API_KEY=<your-upstream-key>\n"
        ),
        EnvPrefix::Openai => format!(
            "# pony proxy — openai\nexport OPENAI_BASE_URL={base_url}\nexport OPENAI_API_KEY=<your-upstream-key>\n"
        ),
        EnvPrefix::Opencode => format!(
            "# pony proxy — opencode (zen)\nexport OPENCODE_BASE_URL={base_url}\nexport OPENCODE_API_KEY=<your-upstream-key>\n"
        ),
        // 无标准 env 约定的服务：仅注释示例（M2 §4.7 模板表）
        EnvPrefix::CommentOnly => format!(
            "# pony proxy — {}（无标准 env 约定，按所用 SDK 手动配置 base url）\n# base_url = {base_url}\n",
            tpl.name
        ),
    }
}

/// 可用 service 列表（未知 service 报错用）。
pub fn available() -> Vec<&'static str> {
    TEMPLATES.iter().map(|t| t.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_full_output_with_token() {
        let tpl = lookup("anthropic").unwrap();
        let out = render(tpl, "anthropic", Some("pony_abc"), "http://127.0.0.1:8899");
        assert!(out.contains("export ANTHROPIC_BASE_URL=http://127.0.0.1:8899/pony_abc/anthropic"));
        assert!(out.contains("export ANTHROPIC_API_KEY=<your-upstream-key>"));
        assert!(!out.contains("<your-pony-token-here>"));
    }

    #[test]
    fn openai_placeholder_without_token() {
        let tpl = lookup("openai").unwrap();
        let out = render(tpl, "openai", None, "http://127.0.0.1:8899");
        assert!(out.contains("export OPENAI_BASE_URL=http://127.0.0.1:8899/<your-pony-token-here>/openai"));
    }

    #[test]
    fn opencode_prefix() {
        let tpl = lookup("opencode").unwrap();
        let out = render(tpl, "opencode", None, "http://x:8899");
        assert!(out.contains("OPENCODE_BASE_URL="));
    }

    #[test]
    fn comment_only_services_emit_no_exports() {
        for svc in ["google", "github", "x", "facebook"] {
            let tpl = lookup(svc).unwrap();
            let out = render(tpl, svc, None, "http://x:8899");
            assert!(!out.contains("export "), "{svc} should not emit exports");
            assert!(
                out.contains(&format!("# base_url = http://x:8899/<your-pony-token-here>/{svc}")),
                "{svc}: {out}"
            );
        }
    }

    #[test]
    fn custom_route_overrides_default() {
        let tpl = lookup("openai").unwrap();
        let out = render(tpl, "oai-mirror", None, "http://127.0.0.1:8899");
        assert!(out.contains("/oai-mirror"));
    }

    #[test]
    fn unknown_service_not_found_and_available_listed() {
        assert!(lookup("nosuch").is_none());
        let avail = available();
        assert_eq!(avail.len(), 7);
        assert!(avail.contains(&"anthropic"));
    }

    #[test]
    fn placeholder_literal_matches_spec() {
        let tpl = lookup("anthropic").unwrap();
        let out = render(tpl, "anthropic", None, "http://127.0.0.1:8899");
        assert!(out.contains("<your-pony-token-here>"));
    }
}
