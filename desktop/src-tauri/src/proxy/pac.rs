//! PAC 脚本生成（M6 spec §5，R4/F5）：纯函数。
//!
//! 返回串格式钉死：命中条目 → 单条 `PROXY 127.0.0.1:18900`（**无 DIRECT 兜底**，
//! 防隧道失败静默裸连的安全错觉）；未命中 → `DIRECT`。

use super::whitelist;

pub const PROXY_HOST: &str = "127.0.0.1";
pub const PROXY_PORT: u16 = 18900;

/// 由白名单条目生成 PAC 脚本文本。条目按 JS 字符串字面量转义。
pub fn generate_pac(entries: &[String]) -> String {
    let list = entries
        .iter()
        .map(|e| format!("'{}'", e.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"// pony-desktop PAC — 自动生成，请勿手改
function FindProxyForURL(url, host) {{
  var entries = [{list}];
  var h = host.toLowerCase();
  while (h.endsWith('.')) {{ h = h.slice(0, -1); }}
  for (var i = 0; i < entries.length; i++) {{
    var e = entries[i];
    if (h === e || h.indexOf('.' + e) === h.length - e.length - 1) {{
      return 'PROXY {host}:{port}';
    }}
  }}
  return 'DIRECT';
}}
"#,
        list = list,
        host = PROXY_HOST,
        port = PROXY_PORT
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pac_contains_proxy_line_without_direct_fallback() {
        // R4/F5 回归锚点：命中分支绝不允许 DIRECT 兜底
        let pac = generate_pac(&["youtube.com".to_string()]);
        // R4/F5 回归锚点：命中返回串为单条 PROXY，其后紧跟分号（无 DIRECT 兜底）
        assert!(pac.contains("return 'PROXY 127.0.0.1:18900';"));
        assert!(
            !pac.contains("PROXY 127.0.0.1:18900'; DIRECT"),
            "命中分支不得携带 DIRECT 兜底"
        );
        assert!(pac.contains("return 'DIRECT';"), "未命中分支应为 DIRECT");
    }

    #[test]
    fn pac_escapes_entries() {
        let pac = generate_pac(&["weird'com".to_string(), "back\\slash.com".to_string()]);
        assert!(pac.contains("'weird\\'com'"));
        assert!(pac.contains("'back\\\\slash.com'"));
    }

    #[test]
    fn pac_empty_whitelist_still_valid() {
        let pac = generate_pac(&[]);
        assert!(pac.contains("var entries = [];"));
        assert!(pac.contains("return 'DIRECT';"));
    }
}
