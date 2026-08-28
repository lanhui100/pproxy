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

/// 区域别名表：base -> 地区域名
/// 为满足“google.com 自动覆盖 google.com.hk/.ph 等”需求，维护 google 系别名。
/// 其余 .com 域名通过通用 `*.com.xx` 规则在 alias_match 中兜底，无需枚举。
const GOOGLE_ALIASES: &[&str] = &[
    "google.com.hk",
    "google.com.ph",
    "google.com.tw",
    "google.com.sg",
    "google.com.au",
    "google.co.jp",
    "google.co.uk",
    "google.co.kr",
    "google.de",
    "google.fr",
    "google.it",
    "google.es",
    "google.nl",
    "google.com.br",
    "google.ca",
    "google.com.mx",
    "google.com.ar",
    "google.ch",
    "google.se",
    "google.be",
    "google.pl",
    "google.com.my",
    "google.co.nz",
    "google.co.in",
    "google.com.tr",
];

fn aliases_for(entry: &str) -> &'static [&'static str] {
    match entry {
        "google.com" => GOOGLE_ALIASES,
        // 未来可扩展：youtube.com / googleapis.com 等如有地区站可在此追加
        _ => &[],
    }
}

/// 对单个 entry 的别名命中（显式别名表 + 通用 .com.xx 规则）
fn alias_match(host: &str, entry: &str) -> bool {
    // 显式别名
    for alias in aliases_for(entry) {
        if suffix_match(host, alias) {
            return true;
        }
    }
    // 通用规则：entry 以 .com 结尾时，host 以 entry + ".<2-3小写字母>" 结尾即视为地区变体
    // 例如 google.com -> google.com.hk / google.com.ph（含子域 www.google.com.hk）
    // 覆盖用户“不同地区国家域名自动加入”诉求，对任意 .com 域名生效，副作用为更宽覆盖（可接受）
    if entry.ends_with(".com") {
        // host == entry + ".xx" 或 host ends_with ".entry.xx"
        // 构造 host 与 entry 的区域变体后缀判断
        // 遍历 host 可能的 ".xx" 后缀：只需判断 host 去掉最后 ".xx" 后是否 suffix_match entry
        if let Some(dot) = host.rfind('.') {
            let suffix = &host[dot + 1..];
            if suffix.len() >= 2 && suffix.len() <= 3 && suffix.chars().all(|c| c.is_ascii_lowercase()) {
                let base = &host[..dot];
                if suffix_match(base, entry) {
                    return true;
                }
            }
        }
    }
    false
}

/// 计算 entry 在 PAC/匹配时展开的所有形态（自身 + 别名）
pub fn expand_entries(entries: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for e in entries {
        let norm = normalize_host(e);
        if seen.insert(norm.clone()) {
            out.push(norm.clone());
        }
        for alias in aliases_for(&norm) {
            let a = normalize_host(alias);
            if seen.insert(a.clone()) {
                out.push(a);
            }
        }
        // 通用 .com.xx 展开不在此处显式枚举，由 alias_match 运行时兜底，避免爆炸
    }
    out
}

/// host 是否命中白名单（任一条目后缀匹配即命中，含区域别名）。
pub fn matches(host: &str, entries: &[String]) -> bool {
    let h = normalize_host(host);
    if h.is_empty() {
        return false;
    }
    entries.iter().any(|e| {
        let ne = normalize_host(e);
        suffix_match(&h, &ne) || alias_match(&h, &ne)
    })
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

    #[test]
    fn regional_alias_google_com() {
        let e = entries(&["google.com"]);
        // 显式别名
        assert!(matches("google.com.hk", &e));
        assert!(matches("www.google.com.hk", &e));
        assert!(matches("maps.google.com.hk", &e));
        assert!(matches("google.com.ph", &e));
        assert!(matches("www.google.com.ph", &e));
        assert!(matches("google.co.jp", &e));
        assert!(matches("www.google.co.jp", &e));
        assert!(matches("google.co.uk", &e));
        // 通用 .com.xx
        assert!(matches("google.com.sg", &e));
        // 子域 + 区域：通过别名 google.com.hk 的子域匹配
        assert!(matches("one.google.com.hk", &e));
        assert!(matches("one.google.com.hk", &entries(&["google.com.hk"])));
        // google.com 自身仍命中
        assert!(matches("google.com", &e));
        assert!(matches("www.google.com", &e));
        // 反向：notgoogle.com.hk 不得命中
        assert!(!matches("notgoogle.com.hk", &e));
    }

    #[test]
    fn regional_generic_com_xx() {
        // 任意 .com 域名其 .com.xx 区域变体应被通用规则覆盖
        let e = entries(&["example.com"]);
        assert!(matches("example.com.hk", &e));
        assert!(matches("www.example.com.hk", &e));
        assert!(matches("example.com.ph", &e));
        // .com.evil 不应命中（非 2-3 字母或含非字母）
        assert!(!matches("example.com.evil", &e));
    }

    #[test]
    fn expand_entries_contains_aliases() {
        let e = entries(&["google.com"]);
        let expanded = expand_entries(&e);
        assert!(expanded.contains(&"google.com".to_string()));
        assert!(expanded.contains(&"google.com.hk".to_string()));
        assert!(expanded.contains(&"google.com.ph".to_string()));
        assert!(expanded.contains(&"google.co.jp".to_string()));
    }
}
