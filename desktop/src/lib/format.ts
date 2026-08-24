// 数字与时间人性化格式化（spec §2-2/§9.1）：非法输入统一回退 '—'，未来时间戳回退绝对日期。
// 全部纯函数，node 环境可测；日期格式手工拼接，避免 locale 差异影响测试与展示一致性。

/** 字节 → B→KB→MB→GB；<10 保 2 位小数否则 1 位；负数/非有限数 → '—'。 */
export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '—'
  const units = ['B', 'KB', 'MB', 'GB'] as const
  let v = n
  let u = 0
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024
    u++
  }
  if (u === 0) return `${Math.round(v)} B`
  const digits = v < 10 ? 2 : 1
  return `${v.toFixed(digits)} ${units[u]}`
}

/** 计数：≥10000 → 'x.x 万'；其余千分位；负数/非有限数 → '—'。 */
export function fmtCount(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '—'
  if (n >= 10000) return `${(n / 10000).toFixed(1)} 万`
  return Math.round(n).toLocaleString('en-US')
}

/** 本地日期 YYYY-MM-DD；无效时间戳 → '—'。 */
export function fmtDate(ts: number): string {
  const d = new Date(ts)
  if (!Number.isFinite(d.getTime())) return '—'
  const mm = String(d.getMonth() + 1).padStart(2, '0')
  const dd = String(d.getDate()).padStart(2, '0')
  return `${d.getFullYear()}-${mm}-${dd}`
}

/** 本地日期时间 YYYY-MM-DD HH:mm。 */
export function fmtDateTime(ts: number): string {
  const d = new Date(ts)
  if (!Number.isFinite(d.getTime())) return '—'
  const hh = String(d.getHours()).padStart(2, '0')
  const mi = String(d.getMinutes()).padStart(2, '0')
  return `${fmtDate(ts)} ${hh}:${mi}`
}

/**
 * 相对时间：刚刚 / N分钟前 / N小时前 / 昨天；超过 2 天或未来时间戳回退 fmtDate。
 * now 可注入便于单测。
 */
export function fmtRelative(ts: number, now: number = Date.now()): string {
  const diff = now - ts
  if (!Number.isFinite(diff)) return fmtDate(ts) // 含 NaN 时间戳 → '—'
  if (diff < 0) return fmtDate(ts)
  const MIN = 60_000
  const HOUR = 3_600_000
  const DAY = 86_400_000
  if (diff < MIN) return '刚刚'
  if (diff < HOUR) return `${Math.floor(diff / MIN)}分钟前`
  if (diff < DAY) return `${Math.floor(diff / HOUR)}小时前`
  if (diff < 2 * DAY) return '昨天'
  return fmtDate(ts)
}

/** 告警级别中文词（OS 通知前缀与徽章共用，spec §3.2-F2）。 */
export function alertLevelLabel(level: 'warning' | 'critical'): '警告' | '严重' {
  return level === 'critical' ? '严重' : '警告'
}
