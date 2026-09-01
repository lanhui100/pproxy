//! 渲染层：表格对齐、字节人性化、时间戳本地化（M2 §7 单测覆盖的纯函数）。

/// 列宽取各行最大宽的简单对齐表。
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// 剥离 ANSI 转义序列（如终端颜色码 \x1b[32m 等），用于真实视觉列宽计算。纯函数可测。
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(&'[') = chars.peek() {
                chars.next(); // consume '['
                while let Some(sc) = chars.next() {
                    if sc.is_ascii_alphabetic() || sc == 'm' {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// 计算单个字符在终端中的显示列宽（East Asian Width 与常见 Emoji 支持）。纯函数可测。
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    // 控制字符与零宽字符（包含零宽连接符与 Emoji 变体选择符）
    if u < 0x20
        || (0x7F..=0x9F).contains(&u)
        || u == 0x200B
        || u == 0x200D
        || u == 0xFEFF
        || u == 0xFE0F
    {
        return 0;
    }
    // 宽字符范围（CJK 统一表意文字、全角标点、日韩假名、全角 ASCII、Emoji 等）
    if (0x1100..=0x115F).contains(&u)
        || (0x231A..=0x231B).contains(&u)
        || (0x23E9..=0x23EC).contains(&u)
        || (0x23F0..=0x23F3).contains(&u)
        || (0x25FD..=0x25FE).contains(&u)
        || (0x2600..=0x27BF).contains(&u) // Symbols, Dingbats, ✨, ⚠️, ⚡, ✅, ❌
        || (0x2B50..=0x2B55).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE10..=0xFE19).contains(&u)
        || (0xFE30..=0xFE6F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F000..=0x1FFFF).contains(&u) // Emoji, 🚀, 🎉 等
        || (0x20000..=0x3FFFD).contains(&u)
    {
        2
    } else {
        1
    }
}

/// 计算字符串在终端中的总显示列宽（自动剔除 ANSI 颜色转义字符）。纯函数可测。
pub fn display_width(s: &str) -> usize {
    strip_ansi(s).chars().map(char_width).sum()
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    /// 渲染为对齐文本（支持中英混排与 CJK 等宽对齐）。
    pub fn render(&self) -> String {
        let ncols = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| display_width(h)).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate().take(ncols) {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
        let mut out = String::new();
        let header_line = self
            .headers
            .iter()
            .zip(&widths)
            .map(|(h, w)| pad(h, *w))
            .collect::<Vec<_>>()
            .join("  ");
        out.push_str(header_line.trim_end());
        out.push('\n');
        let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        out.push_str(&sep.join("  "));
        out.push('\n');
        for row in &self.rows {
            let line = (0..ncols)
                .map(|i| row.get(i).map(String::as_str).unwrap_or(""))
                .zip(&widths)
                .map(|(c, w)| pad(c, *w))
                .collect::<Vec<_>>()
                .join("  ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

fn pad(s: &str, width: usize) -> String {
    let len = display_width(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// 字节人性化：B → KB → MB → GB（1024 进制，1 位小数；<1024 原样 + B）。
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// UTC unix 秒 → 本地时区 `%Y-%m-%d %H:%M`。None/0 → "-"。
///
/// core 的"无 chrono"约定只约束落库；CLI 展示需要时区。不引依赖：
/// unix 下经 libc `localtime_r` 取本地分量，失败回落 UTC 手算。
pub fn fmt_ts(ts: Option<u64>) -> String {
    let Some(ts) = ts.filter(|t| *t > 0) else {
        return "-".to_string();
    };
    let (y, mo, d, h, mi) = local_parts(ts);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

fn local_parts(ts: u64) -> (i64, u32, u32, u32, u32) {
    #[cfg(unix)]
    if let Some(p) = libc_localtime(ts) {
        return p;
    }
    utc_parts(ts)
}

#[cfg(unix)]
#[allow(non_camel_case_types)]
fn libc_localtime(ts: u64) -> Option<(i64, u32, u32, u32, u32)> {
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }
    #[repr(C)]
    struct TimeT(i64);
    extern "C" {
        fn localtime_r(timep: *const TimeT, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    let t = TimeT(ts.min(i64::MAX as u64) as i64);
    let ok = unsafe { localtime_r(&t, &mut tm) };
    if ok.is_null() {
        return None;
    }
    // 安全边界：分量异常时回落 UTC（负年份等极端 TZ 不崩）
    if !(0..=59).contains(&tm.tm_min)
        || !(0..=23).contains(&tm.tm_hour)
        || !(1..=31).contains(&tm.tm_mday)
        || !(0..=11).contains(&tm.tm_mon)
    {
        return None;
    }
    Some((
        tm.tm_year as i64 + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
    ))
}

/// UTC 平推手算（civil_from_days 算法），libc 不可用时兜底。
fn utc_parts(ts: u64) -> (i64, u32, u32, u32, u32) {
    let days = ts / 86_400;
    let rem = ts % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_alignment_widest_column() {
        let mut t = Table::new(&["name", "status"]);
        t.push(vec!["anthropic".into(), "active".into()]);
        t.push(vec!["x".into(), "revoked".into()]);
        let rendered = t.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 4); // header + sep + 2 rows
        // 列宽 = max(5, 9, 1) = 9
        assert!(lines[0].starts_with("name      "));
        assert!(lines[0].contains("status"));
        assert!(lines[2].starts_with("anthropic "));
        assert!(lines[3].starts_with("x         "));
        // 行内两列对齐位置一致
        let col2_header = lines[0].find("status").unwrap();
        let col2_row = lines[2].find("active").unwrap();
        assert_eq!(col2_header, col2_row);
    }

    #[test]
    fn empty_table_renders_headers_only() {
        let t = Table::new(&["a"]);
        let r = t.render();
        assert_eq!(r.lines().count(), 2);
    }

    #[test]
    fn human_bytes_boundaries() {
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(human_bytes(0), "0 B");
        // 超大值停在 GB
        let tb = human_bytes(2048_u64 * 1024 * 1024 * 1024);
        assert!(tb.ends_with("GB"));
        assert_eq!(tb, "2048.0 GB");
    }

    #[test]
    fn fmt_ts_utc_fallback_and_zero() {
        assert_eq!(fmt_ts(None), "-");
        assert_eq!(fmt_ts(Some(0)), "-");
        // 已知时刻：2026-08-21 00:00:00 UTC = 1787270400（本机 TZ=UTC）
        // 本地时区可能偏移，只断言格式与年份合理性
        let s = fmt_ts(Some(1_787_270_400));
        assert_eq!(s.len(), 16);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], " ");
        assert_eq!(&s[13..14], ":");
    }

    #[test]
    fn utc_parts_known_values() {
        // 1970-01-01 00:00 UTC
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0));
        // 2026-08-21 00:00:00 UTC = 1787270400
        assert_eq!(utc_parts(1_787_270_400), (2026, 8, 21, 0, 0));
        // 闰年：2024-02-29 12:34 UTC = 1709210040
        assert_eq!(utc_parts(1_709_210_040), (2024, 2, 29, 12, 34));
    }

    #[test]
    fn test_display_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width("中国"), 4);
        assert_eq!(display_width("pproxy 代理"), 11); // 6 + 1 + 4 = 11
        assert_eq!(display_width("✨ 测试"), 7); // ✨ 宽 2 + 空格 1 + 测试 4 = 7
        assert_eq!(display_width("🚀 部署成功"), 11); // 🚀 宽 2 + 空格 1 + 8 = 11
        // ANSI 颜色代码不增加显示宽度
        assert_eq!(display_width("\x1b[1;32mhello\x1b[0m"), 5);
        assert_eq!(display_width("\x1b[31m中国\x1b[0m"), 4);
    }

    #[test]
    fn test_table_cjk_alignment() {
        let mut t = Table::new(&["服务名", "状态"]);
        t.push(vec!["anthropic".into(), "\x1b[32m已启用\x1b[0m".into()]);
        t.push(vec!["本地网关".into(), "禁用".into()]);
        let rendered = t.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 4);
        // 表头 "服务名" 宽 6, "anthropic" 宽 9, "本地网关" 宽 8 => 第一列列宽 9
        // 行 2 "\x1b[32m已启用\x1b[0m" 实际显示宽 6, 表头 "状态" 宽 4 => 第二列列宽 6
        assert!(lines[0].starts_with("服务名   ")); // 6 + 3 spaces = 9
        assert!(lines[2].starts_with("anthropic")); // 9
        assert!(lines[3].starts_with("本地网关 ")); // 8 + 1 space = 9
    }
}
