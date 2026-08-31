// gate-policy — colo 门禁纯函数（无 CF 依赖，node 可直接单测）。
// worker.js 在收到首帧 {host,port} 后、connect() 前调用 shouldBlockColo：
// 仅「Google 系 host ∧ colo 黑名单」才拒（防 HKG colo 下 Gemini 400 location 错误），
// 非 Google host 全放行（防全量流量倾泻到 Render 兜底打爆免费额度）。

export const DEFAULT_BLOCKED_COLOS = ['HKG', 'MFM']

const GOOGLE_SUFFIXES = [
  'google.com',
  'googleapis.com',
  'gstatic.com',
  'googleusercontent.com',
  'g.co',
  'goog',
]

export function normalizeHost(h) {
  return String(h || '').trim().toLowerCase().replace(/\.$/, '')
}

function suffixMatch(host, entry) {
  if (!host.endsWith(entry)) return false
  const rest = host.slice(0, host.length - entry.length)
  return rest === '' || rest.endsWith('.')
}

export function isGoogleHost(host) {
  const h = normalizeHost(host)
  if (!h) return false
  return GOOGLE_SUFFIXES.some((s) => suffixMatch(h, s))
}

export function parseBlockedColos(envValue) {
  if (typeof envValue !== 'string' || !envValue.trim()) return [...DEFAULT_BLOCKED_COLOS]
  return envValue.split(',').map((s) => s.trim().toUpperCase()).filter(Boolean)
}

// 返回 null（放行）或拒绝 reason 字符串。colo 缺失/空 → fail-open 放行（维持现状语义）。
export function shouldBlockColo(colo, host, blockedColos = DEFAULT_BLOCKED_COLOS) {
  const c = String(colo || '').trim().toUpperCase()
  if (!c) return null
  if (!blockedColos.includes(c)) return null
  if (!isGoogleHost(host)) return null
  return `unsupported_colo:${c}`
}
