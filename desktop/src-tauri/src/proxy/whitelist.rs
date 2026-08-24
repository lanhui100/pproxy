//! 白名单域名后缀匹配（M6 spec §4）：纯函数，无 IO。
//!
//! 匹配规则：条目 `youtube.com` 命中 `youtube.com` 与 `*.youtube.com`；
//! 输入规范化（大小写折叠、末尾点剥离、IDN→punycode 由调用方在 UI 层处理，
//! 此处再做一次防御性小写化）。

/// 规范化 host：小写化 + 剥离末尾点。
pub fn normalize_host(host: &str) -> String {
    let mut h = host.trim().to_ascii_lowercase();
    while h.ends_with('.') {
        h.pop();
    }
    h
}

/// host 是否命中白名单（任一条目后缀匹配即命中）。
pub fn matches(host: &str, entries: &[String]) -> bool {
    let h = normalize_host(host);
    if h.is_empty() {
        return false;
    }
    entries.iter().any(|e| suffix_match(&h, &normalize_host(e)))
}

fn suffix_match(host: &str, entry: &str) -> bool {
    if entry.is_empty() || !host.ends_with(entry) {
        return false;
    }
    // 命中条件：host == entry，或剩余前缀以 '.' 结尾（真正的子域）
    let rest = &host[..host.len() - entry.len()];
    rest.is_empty() || rest.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exact_and_subdomain_match() {
        let e = entries(&["youtube.com"]);
        assert!(matches("youtube.com", &e));
        assert!(matches("www.youtube.com", &e));
        assert!(matches("a.b.youtube.com", &e));
    }

    #[test]
    fn suffix_trap_rejected() {
        // notyoutube.com 不得因 youtube.com 命中
        let e = entries(&["youtube.com"]);
        assert!(!matches("notyoutube.com", &e));
        assert!(!matches("youtube.com.evil.cn", &e));
    }

    #[test]
    fn case_and_trailing_dot_normalized() {
        let e = entries(&["GitHub.com"]);
        assert!(matches("GITHUB.COM", &e));
        assert!(matches("api.Github.com.", &e));
    }

    #[test]
    fn empty_host_never_matches() {
        let e = entries(&["github.com"]);
        assert!(!matches("", &e));
    }

    #[test]
    fn multiple_entries_any_hit() {
        let e = entries(&["google.com", "github.com", "googlevideo.com"]);
        assert!(matches("upload.youtube.com", &e) == false);
        assert!(matches("www.googlevideo.com", &e));
        assert!(matches("gist.github.com", &e));
    }
}
