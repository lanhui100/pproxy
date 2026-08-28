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

/// 由白名单条目生成 PAC 脚本文本。条目按 serde_json 转义。
/// 区域别名：google.com 等会展开为 google.com.hk/.ph 等地区域名（whitelist::expand_entries）
pub fn generate_pac(entries: &[String]) -> String {
    let bypass: Vec<String> = collect_bypass_hosts().into_iter().collect();
    let expanded = super::whitelist::expand_entries(entries);
    generate_pac_with_bypass(&expanded, &bypass)
}

pub fn generate_pac_with_bypass(entries: &[String], bypass_hosts: &[String]) -> String {
    // Use serde_json for JS escaping (covers \b\f\n\r\t\u2028\u2029 etc.)
    // 再次展开以确保直接调用此函数也覆盖别名
    let expanded = super::whitelist::expand_entries(entries);
    let entries_json: Vec<String> = expanded.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    let bypass_json: Vec<String> = bypass_hosts.iter().map(|e| serde_json::to_string(e).unwrap()).collect();
    let entries_list = entries_json.join(",");
    let bypass_list = bypass_json.join(",");

    format!(
        r#"// pony-desktop PAC — 自动生成，请勿手改
function FindProxyForURL(url, host) {{
  var h = host.toLowerCase();
  while (h.endsWith('.')) {{ h = h.slice(0, -1); }}
  function isPrivateHost(host) {{
    if (host === "localhost" || host === "::1") return true;
    // IPv6 private: fc00::/7, fe80::/10, ::1
    if (host.includes(":")) {{
      if (host === "::1") return true;
      if (host.startsWith("fc") || host.startsWith("fd") || host.startsWith("fe80:")) return true;
      return false;
    }}
    // IPv4 private: must be dotted-quad IP before prefix checks (avoid 192.168.evil.com false positive)
    if (!/^\d+\.\d+\.\d+\.\d+$/.test(host)) return false;
    if (host.startsWith("127.")) return true;
    if (host.startsWith("10.")) return true;
    if (host.startsWith("192.168.")) return true;
    if (host.startsWith("172.")) {{
      var m = host.match(/^172\.(\d+)\./);
      if (m) {{ var n = parseInt(m[1], 10); if (n >= 16 && n <= 31) return true; }}
    }}
    return false;
  }}
  // DIRECT 优先级最高：plain host / localhost / private / bypass
  if (isPlainHostName(host) || h === "localhost" || isPrivateHost(h)) return 'DIRECT';
  var bypass = [{bypass_list}];
  for (var j = 0; j < bypass.length; j++) {{
    var b = bypass[j].toLowerCase();
    if (h === b || h.endsWith('.' + b)) return 'DIRECT';
  }}
  var entries = [{entries_list}];
  for (var i = 0; i < entries.length; i++) {{
    var e = entries[i].toLowerCase();
    if (h === e || h.endsWith('.' + e)) {{
      return 'PROXY {host}:{port}';
    }}
  }}
  return 'DIRECT';
}}
"#,
        bypass_list = bypass_list,
        entries_list = entries_list,
        host = PROXY_HOST,
        port = PROXY_PORT
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pac_contains_proxy_line_without_direct_fallback() {
        let pac = generate_pac(&["youtube.com".to_string()]);
        assert!(pac.contains("return 'PROXY 127.0.0.1:18900';"));
        assert!(
            !pac.contains("PROXY 127.0.0.1:18900'; DIRECT"),
            "命中分支不得携带 DIRECT 兜底"
        );
        assert!(pac.contains("return 'DIRECT';"), "未命中分支应为 DIRECT");
    }

    #[test]
    fn pac_escapes_entries() {
        let pac = generate_pac_with_bypass(&["weird'com".to_string(), "back\\slash.com".to_string()], &[]);
        // serde_json escapes: "weird'com" -> "\"weird'com\"" (single quote not escaped but ok), backslash -> "\\"
        assert!(pac.contains("weird'com") || pac.contains("weird\\'com"));
        assert!(pac.contains("back\\\\slash.com"));
    }

    #[test]
    fn pac_empty_whitelist_still_valid() {
        let pac = generate_pac(&[]);
        assert!(pac.contains("var entries = [];") || pac.contains("var entries = []"));
        assert!(pac.contains("return 'DIRECT';"));
    }

    #[test]
    fn pac_bypass_priority_before_whitelist() {
        let pac = generate_pac_with_bypass(&["access.ponyjob.top".to_string()], &["access.ponyjob.top".to_string()]);
        // bypass should appear before entries loop and directly return DIRECT
        let bypass_pos = pac.find("var bypass").expect("bypass var");
        let entries_pos = pac.find("var entries").expect("entries var");
        assert!(bypass_pos < entries_pos, "bypass must be before whitelist");
        assert!(pac.contains("isPrivateHost"));
    }

    #[test]
    fn pac_uses_serde_json_escaping() {
        let pac = generate_pac_with_bypass(&["a\"b".to_string()], &[]);
        assert!(pac.contains("\"a\\\"b\""), "serde_json should escape double quote");
    }

    #[test]
    fn collect_bypass_contains_updater_host() {
        let set = collect_bypass_hosts();
        assert!(set.contains("access.ponyjob.top"), "updater host should be in bypass");
    }
}
