//! PAC 脚本生成（M6 spec §5，R4/F5）：纯函数。
//!
//! 返回串格式钉死：命中条目 → 单条 `PROXY 127.0.0.1:18900`（**无 DIRECT 兜底**，
//! 防隧道失败静默裸连的安全错觉）；未命中 → `DIRECT`。
//! T3: 首行注入 isPrivateHost + bypassSet 的 DIRECT 优先级最高；用 serde_json 转义。

use std::collections::BTreeSet;

pub const PROXY_HOST: &str = "127.0.0.1";
pub const PROXY_PORT: u16 = 18900;

/// 收集 bypass hosts：tunnel.json url + updater.endpoints + fallback
pub fn collect_bypass_hosts() -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    // 1. tunnel.json
    let tunnel_path = {
        #[cfg(windows)]
        {
            std::env::var("APPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("pony-desktop")
                .join("tunnel.json")
        }
        #[cfg(not(windows))]
        {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".pony-desktop"))
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("tunnel.json")
        }
    };
    if let Ok(s) = std::fs::read_to_string(&tunnel_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
                if let Ok(parsed) = url::Url::parse(u) {
                    if let Some(h) = parsed.host_str() {
                        set.insert(h.to_ascii_lowercase());
                    }
                }
            }
        }
    }
    // 2. updater.endpoints (tauri.conf.json 静态值)
    // Keep in sync with desktop/src-tauri/tauri.conf.json plugins.updater.endpoints
    for ep in ["https://access.ponyjob.top/dsk/latest.json"] {
        if let Ok(p) = url::Url::parse(ep) {
            if let Some(h) = p.host_str() {
                set.insert(h.to_ascii_lowercase());
            }
        }
        // also try to read live tauri.conf if exists? fallback static above is enough
    }
    // 3. fallback if empty
    if set.is_empty() {
        set.insert("access.ponyjob.top".to_string());
    }
    set
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Whitelist,
    Global,
}

impl std::str::FromStr for ProxyMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "global" => Ok(ProxyMode::Global),
            "whitelist" | "rule" | "pac" => Ok(ProxyMode::Whitelist),
            _ => Ok(ProxyMode::Whitelist),
        }
    }
}

/// 由白名单条目及工作模式生成 PAC 脚本文本。
pub fn generate_pac(entries: &[String], mode: ProxyMode) -> String {
    let bypass: Vec<String> = collect_bypass_hosts().into_iter().collect();
    generate_pac_with_bypass(entries, &bypass, mode)
}

pub fn generate_pac_with_bypass(entries: &[String], bypass_hosts: &[String], mode: ProxyMode) -> String {
    let expanded = super::whitelist::expand_entries(entries);
    let entries_json: Vec<String> = expanded.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    let bypass_json: Vec<String> = bypass_hosts.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    let entries_list = entries_json.join(",");
    let bypass_list = bypass_json.join(",");

    let routing_body = match mode {
        ProxyMode::Global => format!(
            r#"  // 全局模式：除内网与直连名单外，所有流量均走代理
  return 'PROXY {host}:{port}';"#,
            host = PROXY_HOST,
            port = PROXY_PORT
        ),
        ProxyMode::Whitelist => format!(
            r#"  var entries = [{entries_list}];
  for (var i = 0; i < entries.length; i++) {{
    var e = entries[i].toLowerCase();
    if (h === e || strEndsWith(h, '.' + e)) {{
      return 'PROXY {host}:{port}';
    }}
    // 区域别名收敛支持（如 google.com -> google.com.hk / accounts.google.com.hk）
    if (e === 'google.com') {{
      var dot = h.lastIndexOf('.');
      if (dot > 0) {{
        var base = h.substring(0, dot);
        var tld = h.substring(dot + 1);
        if (/^[a-z]{{2,3}}$/.test(tld) && (base === e || strEndsWith(base, '.' + e))) {{
          return 'PROXY {host}:{port}';
        }}
      }}
    }}
  }}
  return 'DIRECT';"#,
            entries_list = entries_list,
            host = PROXY_HOST,
            port = PROXY_PORT
        ),
    };

    format!(
        r#"// pony-desktop PAC — 自动生成，请勿手改
function FindProxyForURL(url, host) {{
  // ES3 辅助函数（兼容 Windows JScript 5.8 / WinINET）
  function strStartsWith(s, prefix) {{
    return s.indexOf(prefix) === 0;
  }}
  function strEndsWith(s, suffix) {{
    return s.length >= suffix.length && s.substr(s.length - suffix.length) === suffix;
  }}
  function strIncludes(s, sub) {{
    return s.indexOf(sub) !== -1;
  }}

  var h = host.toLowerCase();
  while (strEndsWith(h, '.')) {{ h = h.slice(0, -1); }}

  function isPrivateHost(target) {{
    var t = target;
    if (strStartsWith(t, '[') && strEndsWith(t, ']')) {{ t = t.slice(1, -1); }}
    if (t === "localhost" || t === "::1" || t === "0.0.0.0") return true;
    if (strStartsWith(t, "::ffff:")) {{ t = t.substring(7); }}

    // IPv6 私网检测
    if (strIncludes(t, ":")) {{
      if (t === "::1") return true;
      var low = t.toLowerCase();
      if (strStartsWith(low, "fc") || strStartsWith(low, "fd") || strStartsWith(low, "fe8") || strStartsWith(low, "fe9") || strStartsWith(low, "fea") || strStartsWith(low, "feb")) return true;
      return false;
    }}

    // IPv4 私网检测（必须是合法点分十进制 IP）
    if (!/^\d+\.\d+\.\d+\.\d+$/.test(t)) return false;
    if (strStartsWith(t, "127.") || strStartsWith(t, "10.") || strStartsWith(t, "192.168.") || strStartsWith(t, "169.254.")) return true;
    if (strStartsWith(t, "172.")) {{
      var m = t.match(/^172\.(\d+)\./);
      if (m) {{ var n = parseInt(m[1], 10); if (n >= 16 && n <= 31) return true; }}
    }}
    if (strStartsWith(t, "100.")) {{
      var m2 = t.match(/^100\.(\d+)\./);
      if (m2) {{ var n2 = parseInt(m2[1], 10); if (n2 >= 64 && n2 <= 127) return true; }}
    }}
    return false;
  }}

  // DIRECT 优先级最高：plain host / localhost / private / bypass
  if (isPlainHostName(h) || h === "localhost" || isPrivateHost(h)) return 'DIRECT';
  var bypass = [{bypass_list}];
  for (var j = 0; j < bypass.length; j++) {{
    var b = bypass[j].toLowerCase();
    if (h === b || strEndsWith(h, '.' + b)) return 'DIRECT';
  }}
{routing_body}
}}
"#,
        bypass_list = bypass_list,
        routing_body = routing_body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pac_contains_proxy_line_without_direct_fallback() {
        let pac = generate_pac(&["youtube.com".to_string()], ProxyMode::Whitelist);
        assert!(pac.contains("return 'PROXY 127.0.0.1:18900';"));
        assert!(
            !pac.contains("PROXY 127.0.0.1:18900'; DIRECT"),
            "命中分支不得携带 DIRECT 兜底"
        );
        assert!(pac.contains("return 'DIRECT';"), "未命中分支应为 DIRECT");
    }

    #[test]
    fn pac_global_mode_returns_proxy_for_all() {
        let pac = generate_pac(&[], ProxyMode::Global);
        assert!(pac.contains("全局模式"));
        assert!(pac.contains("return 'PROXY 127.0.0.1:18900';"));
        assert!(!pac.contains("return 'DIRECT';\n}"));
    }

    #[test]
    fn pac_escapes_entries() {
        let pac = generate_pac_with_bypass(&["weird'com".to_string(), "back\\slash.com".to_string()], &[], ProxyMode::Whitelist);
        assert!(pac.contains("weird'com") || pac.contains("weird\\'com"));
        assert!(pac.contains("back\\\\slash.com"));
    }

    #[test]
    fn pac_empty_whitelist_still_valid() {
        let pac = generate_pac(&[], ProxyMode::Whitelist);
        assert!(pac.contains("return 'DIRECT';"));
    }

    #[test]
    fn pac_bypass_priority_before_whitelist() {
        let pac = generate_pac_with_bypass(&["access.ponyjob.top".to_string()], &["access.ponyjob.top".to_string()], ProxyMode::Whitelist);
        let bypass_pos = pac.find("var bypass").expect("bypass var");
        let entries_pos = pac.find("var entries").expect("entries var");
        assert!(bypass_pos < entries_pos, "bypass must be before whitelist");
        assert!(pac.contains("isPrivateHost"));
    }

    #[test]
    fn pac_uses_serde_json_escaping() {
        let pac = generate_pac_with_bypass(&["a\"b".to_string()], &[], ProxyMode::Whitelist);
        assert!(pac.contains("\"a\\\"b\""), "serde_json should escape double quote");
    }

    #[test]
    fn collect_bypass_contains_updater_host() {
        let set = collect_bypass_hosts();
        assert!(set.contains("access.ponyjob.top"), "updater host should be in bypass");
    }
}
