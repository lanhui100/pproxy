// gate-policy — Google 官方支持区域精准门禁与白名单机制（纯函数，node 可直接单测）。
// worker.js 在收到首帧 {host,port} 后、connect() 前调用 shouldBlockColo：
// 1. 非 Google 系 host：全部放行，直连享受 CF 全球 Anycast 低延迟与高吞吐；
// 2. Google 系 host（Gemini / Cloud Code API 等）：
//    - 严格拦截非官方支持区域（香港 HKG、澳门 MFM、中国大陆各节点 PEK/PVG/CAN 等、受制裁国）；
//    - 仅放行明确属于 Google 官方 Gemini/API 支持区域（美洲、欧洲、日韩新台澳等）的合规 Colo；
//    - 未命中放行区域或处于黑名单时，立即返回 unsupported_colo:<colo>，
//      触发客户端（桌面端 engine_tunnel）毫秒级无缝 Failover 至 Vercel iad1 美区原生出口。

export const GOOGLE_SUFFIXES = [
  'google.com',
  'googleapis.com',
  'gstatic.com',
  'googleusercontent.com',
  'deepmind.google',
  // Antigravity CLI（agy）的官方域名：桌面端白名单默认含这两项，必须纳入 Google 系
  // 否则 CF gate 会把它们当非 Google 流量全量放行，落在 HKG/MFM 等区域时 Google 仍会拒绝，
  // 且不触发 failover 到 Vercel 合规出口。
  'antigravity.google',
  'labs.google',
  'g.co',
  'goog',
]

/**
 * Google 官方明确封禁 / 不支持区域的高危 Cloudflare 数据中心（Colo）：
 * - 中国香港 (HKG)、中国澳门 (MFM)
 * - 中国大陆主要节点 (PEK, PVG, CAN, CGO, SZX, CTU, WUH, NKG, TNA, TAO, HGH, FOC, XMN, CKG 等)
 * - 俄罗斯及制裁国 (DME, SVO, LED, OVB, SVX, MSQ, THR, DAM, HAV, RGN 等)
 */
export const DEFAULT_BLOCKED_COLOS = [
  // 港澳
  'HKG', 'MFM',
  // 中国大陆节点
  'PEK', 'PVG', 'CAN', 'CGO', 'SZX', 'CTU', 'WUH', 'NKG',
  'TNA', 'TAO', 'HGH', 'FOC', 'XMN', 'CKG', 'CSX', 'SJW',
  'TYX', 'SHE', 'HRB', 'KMG', 'NNG', 'XIY', 'INC', 'LHW', 'URC',
  // 俄罗斯及受制裁地区
  'DME', 'SVO', 'LED', 'OVB', 'SVX', 'MSQ', 'THR', 'DAM', 'HAV', 'RGN',
]

/**
 * Google 官方 Gemini / Cloud Code API 明确支持并验证合规的 Cloudflare 数据中心（Colo）：
 * - 美加北美原生区 (IAD, SJC, LAX, ORD, DFW, SEA, ATL, MIA, DEN, EWR, BOS, PHX, PDX 等)
 * - 欧洲全境合规区 (LHR, FRA, AMS, CDG, DUB, ZRH, ARN, MAD, MXP, WAW, OSL, HEL, CPH 等)
 * - 亚太已支持合规区 (NRT, HND, KIX, ICN, SIN, TPE, KHH, SYD, MEL, BNE, PER, AKL 等)
 */
export const DEFAULT_ALLOWED_COLOS = [
  // 北美核心 (US / CA)
  'IAD', 'SJC', 'LAX', 'ORD', 'DFW', 'SEA', 'ATL', 'MIA', 'DEN', 'EWR',
  'BOS', 'PHX', 'PDX', 'CLT', 'MSP', 'SLC', 'DTW', 'PHL', 'SAN', 'TPA',
  'MCI', 'CMH', 'IND', 'BNA', 'RDU', 'AUS', 'SAT', 'IAH', 'OAK', 'SFO',
  'YYZ', 'YVR', 'YUL', 'YYC',
  // 欧洲核心 (GB / DE / NL / FR / IE / CH / SE / ES / IT / PL / NO / FI / DK)
  'LHR', 'FRA', 'AMS', 'CDG', 'DUB', 'ZRH', 'ARN', 'MAD', 'MXP', 'WAW',
  'OSL', 'HEL', 'CPH', 'VIE', 'BRU', 'MUC', 'BER', 'MAN', 'EDI',
  // 亚太与大洋洲合规核心 (JP / KR / SG / TW / AU / NZ)
  'NRT', 'HND', 'KIX', 'FUK', 'OKA',
  'ICN',
  'SIN',
  'TPE', 'KHH',
  'SYD', 'MEL', 'BNE', 'PER', 'ADL', 'AKL',
  // 拉美合规核心 (BR / MX / CL)
  'GRU', 'GIG', 'MEX', 'QRO', 'SCL',
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

/**
 * 必须走"合规物理出口"的目标（Google Cloud Code / Code Assist 系）。
 *
 * 这些接口按**请求来源 IP** 做地区限制：来源落在不受支持地区时返回
 * `HTTP 400 FAILED_PRECONDITION: User location is not supported for the API use.`
 * 入站 colo 门禁管不住它们——`connect()` 的出站 egress IP 由 Cloudflare 另行分配，
 * 与握手 colo 不保证同地区（实测出口在 104.28.158/165.x 间轮换）。因此对这些 host
 * 追加「出站 IP 地理」门禁（见 shouldBlockEgress）。
 *
 * 与 crates/transport/src/route.rs 的 COMPLIANT_EGRESS_SUFFIXES 保持一致。
 */
export const COMPLIANT_EGRESS_SUFFIXES = [
  'daily-cloudcode-pa.googleapis.com',
  'cloudcode-pa.googleapis.com',
  'cloudaicompanion.googleapis.com',
]

export function requiresCompliantEgress(host) {
  const h = normalizeHost(host)
  if (!h) return false
  return COMPLIANT_EGRESS_SUFFIXES.some((s) => suffixMatch(h, s))
}

/**
 * Google 官方支持地区的国家码白名单（出站 IP 地理判定用）。
 * 保守取"北美 + 欧洲 + 亚太合规区 + 拉美合规区"，与 DEFAULT_ALLOWED_COLOS 同口径。
 */
export const DEFAULT_ALLOWED_EGRESS_COUNTRIES = [
  // 北美
  'US', 'CA',
  // 欧洲
  'GB', 'IE', 'FR', 'DE', 'NL', 'BE', 'AT', 'CH', 'SE', 'NO', 'FI', 'DK',
  'ES', 'IT', 'PL', 'PT', 'CZ',
  // 亚太与大洋洲
  'JP', 'KR', 'SG', 'TW', 'AU', 'NZ',
  // 拉美
  'BR', 'MX', 'CL',
]

export function parseAllowedEgressCountries(envValue) {
  if (typeof envValue !== 'string' || !envValue.trim()) {
    return [...DEFAULT_ALLOWED_EGRESS_COUNTRIES]
  }
  return envValue.split(',').map((s) => s.trim().toUpperCase()).filter(Boolean)
}

/**
 * 出站地理门禁（A 方案）：仅对 COMPLIANT_EGRESS_SUFFIXES 生效，fail-closed。
 *
 * 探测失败/尚未得出结果（status='unknown'）时同样拒绝——宁可让客户端
 * failover 到真实机房出口，也不让请求从不确定地区打到 Google。
 * 非合规出口 host 一律放行，避免探测异常时把泛 Google 流量倾泻到 Vercel。
 *
 * @param {{country?: string, ip?: string, status?: 'fresh'|'stale'|'unknown'}} egress
 * @param {string} host 目标主机名
 * @param {{allowedCountries?: string[]}} [options]
 * @returns {null|string} null 放行；string 为拒绝原因（如 "unsupported_egress:HK"）
 */
export function shouldBlockEgress(egress, host, options = {}) {
  if (!requiresCompliantEgress(host)) return null

  const allowed =
    Array.isArray(options.allowedCountries) && options.allowedCountries.length
      ? options.allowedCountries
      : DEFAULT_ALLOWED_EGRESS_COUNTRIES

  const status = egress && typeof egress.status === 'string' ? egress.status : 'unknown'
  const country = String((egress && egress.country) || '').trim().toUpperCase()

  if (status === 'unknown' || !country) return 'unsupported_egress:UNKNOWN'
  if (!allowed.includes(country)) return `unsupported_egress:${country}`
  return null
}

export function parseBlockedColos(envValue) {
  if (typeof envValue !== 'string' || !envValue.trim()) return [...DEFAULT_BLOCKED_COLOS]
  return envValue.split(',').map((s) => s.trim().toUpperCase()).filter(Boolean)
}

export function parseAllowedColos(envValue) {
  if (typeof envValue !== 'string' || !envValue.trim()) return [...DEFAULT_ALLOWED_COLOS]
  return envValue.split(',').map((s) => s.trim().toUpperCase()).filter(Boolean)
}

/**
 * 严格白名单开关判定（P1-2 修复）：fail-closed 默认开启，
 * 仅当显式设置为 'false' / '0' 时才关闭。
 * @param {string|undefined} envValue STRICT_GOOGLE_WHITELIST 的值
 * @returns {boolean}
 */
export function strictGoogleEnabled(envValue) {
  return !(envValue === 'false' || envValue === '0')
}

/**
 * 门禁判定：
 * @param {string} colo Cloudflare 数据中心代码（如 IAD, HKG, NRT）
 * @param {string} host 目标主机名（如 daily-cloudcode-pa.googleapis.com, github.com）
 * @param {Array<string>|{blockedColos?: string[], allowedColos?: string[], strictWhitelist?: boolean}} [options]
 * @returns {null|string} null 表示放行；string 表示拒绝原因（如 "unsupported_colo:HKG"）
 */
export function shouldBlockColo(colo, host, options = DEFAULT_BLOCKED_COLOS) {
  // 1. 非 Google host：全量放行（维持通用大流量性能与稳定性）
  if (!isGoogleHost(host)) return null

  const c = String(colo || '').trim().toUpperCase()

  // 2. 解析门禁选项（向前兼容数组形式参数）
  let blocked = DEFAULT_BLOCKED_COLOS
  let allowed = null
  let strict = false

  if (Array.isArray(options)) {
    blocked = options
  } else if (options && typeof options === 'object') {
    if (Array.isArray(options.blockedColos)) blocked = options.blockedColos
    if (Array.isArray(options.allowedColos)) allowed = options.allowedColos
    strict = Boolean(options.strictWhitelist)
  }

  // 3. colo 缺失时：针对 Google 请求 fail-secure 拦截，触发 Failover 到 Vercel 美区原生出口
  if (!c) return 'unsupported_colo:MISSING'

  // 4. 显式黑名单拦截（香港、澳门、大陆等 Google 封禁区域）
  if (blocked.includes(c)) {
    return `unsupported_colo:${c}`
  }

  // 5. 严格白名单模式：仅允许明确列入 Google 支持区域的 Colo
  if (strict) {
    const effectiveAllowed = allowed || DEFAULT_ALLOWED_COLOS
    if (!effectiveAllowed.includes(c)) {
      return `unsupported_colo:${c}`
    }
  }

  return null
}
