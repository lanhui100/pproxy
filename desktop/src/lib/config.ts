// 端点与本地偏好配置存取（单体架构）：
// - 隧道中继：端点走 tunnel.json（Rust 侧），令牌仅进 OS 凭据库（keyring 命令）
// - 浏览器 dev：端点/令牌走 localStorage 便利通道（打包产物不含此路径）
import { invoke } from '@tauri-apps/api/core'

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

// ---- 隧道中继配置（2026-08 审计整改：端点/令牌 opt-in，令牌仅进 OS 凭据库）----

export interface TunnelConfig {
  url: string
  hasToken: boolean
  credError?: string | null
  fingerprint?: string | null
  /** P0/E4：分源指纹（fallback 文件 vs keyring），分叉即 H2 实锤 */
  fpFallback?: string | null
  fpKeyring?: string | null
  /** P0/E4：当前有效值的来源（keyring / fallback / keyring(diverged) / none） */
  credWinner?: string | null
  /** P0/E7：上次写入审计（时间戳/来源/指纹） */
  credMeta?: { last_write_ts: number; source: string; fp8: string } | null
}

/** 读取隧道配置（Rust 侧文件 + 凭据库探测，不回传令牌明文）。 */
export async function loadTunnelConfig(): Promise<TunnelConfig> {
  if (!isTauri()) {
    const url = localStorage.getItem('pony-tunnel-url') ?? ''
    const hasToken = Boolean(localStorage.getItem('pony-dev-tunnel-token'))
    return { url, hasToken }
  }
  return mapTunnelConfig(await invoke<Record<string, unknown>>('proxy_tunnel_get'))
}

/** 纯函数：把 Rust `proxy_tunnel_get` 的 snake_case 响应映射为前端 camelCase（防字段漂移，可单测）。 */
export function mapTunnelConfig(raw: Record<string, unknown>): TunnelConfig {
  return {
    url: typeof raw.url === 'string' ? raw.url : '',
    hasToken: raw.has_token === true,
    credError: raw.cred_error != null ? String(raw.cred_error) : null,
    fingerprint: raw.fingerprint != null ? String(raw.fingerprint) : null,
    fpFallback: raw.fp_fallback != null ? String(raw.fp_fallback) : null,
    fpKeyring: raw.fp_keyring != null ? String(raw.fp_keyring) : null,
    credWinner: raw.cred_winner != null ? String(raw.cred_winner) : null,
    credMeta: (raw.cred_meta as TunnelConfig['credMeta']) ?? null,
  }
}

/** 校验：仅接受 wss:// 或 ws:// 且无空白。 */
export function normalizeTunnelUrl(url: string): string {
  return url.trim()
}

export function isValidTunnelUrl(url: string): boolean {
  const v = normalizeTunnelUrl(url)
  return v.length > 0 && v.length <= 200 && !/\s/.test(v) && (v.startsWith('wss://') || v.startsWith('ws://'))
}

/** 保存端点；token 非空时一并写入凭据库（留空沿用已保存令牌）。 */
export async function saveTunnelConfig(url: string, token: string): Promise<void> {
  const v = normalizeTunnelUrl(url)
  if (!isValidTunnelUrl(v)) throw new Error('隧道端点必须以 wss:// 或 ws:// 开头且不含空白')
  if (isTauri()) {
    await invoke('proxy_tunnel_set_url', { url: v })
    if (token.trim() !== '') await invoke('tunnel_token_save', { secret: token.trim() })
  } else {
    localStorage.setItem('pony-tunnel-url', v)
    if (token.trim() !== '') localStorage.setItem('pony-dev-tunnel-token', token.trim())
  }
}

/** 仅保存裸授权码（端点沿用已配置；无端点时后端自动补默认双 gate）。 */
export async function saveTunnelToken(secret: string): Promise<void> {
  const v = secret.trim()
  if (!v) throw new Error('授权码不能为空')
  if (isTauri()) {
    await invoke('tunnel_token_save', { secret: v })
  } else {
    localStorage.setItem('pony-dev-tunnel-token', v)
  }
}

/** 清除已保存的隧道令牌。 */
export async function clearTunnelToken(): Promise<boolean> {
  if (isTauri()) {
    try {
      await invoke('tunnel_token_clear')
      return true
    } catch {
      return false
    }
  }
  localStorage.removeItem('pony-dev-tunnel-token')
  return true
}

// ---- 连接口令（pony-gate://）：一个字符串同时携带端点与令牌，粘贴即完成方案 A 配置 ----

export interface GateInput {
  kind: 'code' | 'token'
  /** kind=code 时解析出的端点（用于预览/官方域名警示） */
  url?: string
  /** 端点是否为官方域名（example.com），非官方时前端应提示确认 */
  official?: boolean
}

function isOfficialGateUrl(url: string): boolean {
  return url
    .split(/[,;\n]/)
    .map((s) => s.trim())
    .filter(Boolean)
    .every((u) => {
      try {
        const h = new URL(u).hostname
        return h === 'example.com' || h.endsWith('.example.com')
      } catch {
        return false
      }
    })
}

/** 判定输入是 pony-gate:// 连接口令还是裸授权码；口令附带端点预览与官方域名判定。 */
export function parseGateInput(raw: string): GateInput | null {
  const v = raw.trim()
  if (!v) return null
  if (!v.startsWith('pony-gate://')) return { kind: 'token' }
  try {
    const b64 = v.slice('pony-gate://'.length).replace(/-/g, '+').replace(/_/g, '/')
    const bin = atob(b64)
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0))
    const json = JSON.parse(new TextDecoder().decode(bytes))
    if (typeof json?.u !== 'string' || typeof json?.t !== 'string') return null
    return { kind: 'code', url: json.u, official: isOfficialGateUrl(json.u) }
  } catch {
    return null
  }
}

/** 导入 pony-gate:// 连接口令（端点 + 令牌一步到位，即时生效）。 */
export async function importConnectCode(code: string): Promise<{ url: string; fingerprint?: string; message?: string }> {
  if (!isTauri()) {
    const parsed = parseGateInput(code)
    if (parsed?.kind !== 'code' || !parsed.url) throw new Error('连接口令格式不正确')
    localStorage.setItem('pony-tunnel-url', parsed.url)
    localStorage.setItem('pony-dev-tunnel-token', 'dev-placeholder')
    return { url: parsed.url }
  }
  return invoke('tunnel_connect_code_import', { code: code.trim() })
}

export interface GateCheckResult {
  name: string
  url: string
  ok: boolean
  ms?: number
  error?: string
  /** P0/E10：错误分类（ok / auth401 / denied / timeout / closed / no_token / other） */
  kind?: string
}

export interface TunnelSelfCheck {
  fingerprint: string | null
  credOk: boolean
  credError: string | null
  gates: GateCheckResult[]
  fpFallback?: string | null
  fpKeyring?: string | null
  credWinner?: string | null
  credMeta?: { last_write_ts: number; source: string; fp8: string } | null
}

/** 隧道健康自检：凭据可读性 + token 指纹 + 逐 gate 实测。 */
export async function tunnelSelfCheck(): Promise<TunnelSelfCheck> {
  if (!isTauri()) {
    return {
      fingerprint: 'dev00dev',
      credOk: true,
      credError: null,
      gates: [
        { name: 'cf', url: 'wss://gate.example.com/ws', ok: true, ms: 120 },
        { name: 'vercel', url: 'wss://vgate.example.com/api/ws', ok: true, ms: 210 },
      ],
    }
  }
  // 常态区不展示分叉，仅自检区可见为已知取舍（分叉三值只在这里暴露）
  return mapTunnelSelfCheck(await invoke<Record<string, unknown>>('tunnel_self_check'))
}

/** 纯函数：把 Rust `tunnel_self_check` 的 snake_case 响应映射为前端 camelCase。
 * 无此映射时分叉告警（fpFallback/fpKeyring）与写入展示（credMeta）在 UI 恒不可见。 */
export function mapTunnelSelfCheck(raw: Record<string, unknown>): TunnelSelfCheck {
  const meta = raw.cred_meta as Record<string, unknown> | null | undefined
  return {
    fingerprint: raw.fingerprint != null ? String(raw.fingerprint) : null,
    credOk: (raw.cred_ok as boolean) === true,
    credError: raw.cred_error != null ? String(raw.cred_error) : null,
    gates: Array.isArray(raw.gates) ? (raw.gates as GateCheckResult[]) : [],
    fpFallback: raw.fp_fallback != null ? String(raw.fp_fallback) : null,
    fpKeyring: raw.fp_keyring != null ? String(raw.fp_keyring) : null,
    credWinner: raw.cred_winner != null ? String(raw.cred_winner) : null,
    credMeta:
      meta && typeof meta === 'object'
        ? {
            last_write_ts: typeof meta.last_write_ts === 'number' ? meta.last_write_ts : 0,
            source: typeof meta.source === 'string' ? meta.source : '',
            fp8: typeof meta.fp8 === 'string' ? meta.fp8 : '',
          }
        : null,
  }
}

// ---- auto_proxy 偏好配置：默认 true（启动即开启智能模式代理） ----
export interface AutoProxyConfig {
  auto_proxy?: boolean
  dont_ask?: boolean
  proxy_mode?: 'whitelist' | 'global'
}

export async function loadAutoProxyConfig(): Promise<AutoProxyConfig> {
  if (isTauri()) {
    try {
      const cfg = await invoke<AutoProxyConfig>('app_config_get')
      return cfg ?? {}
    } catch { return {} }
  }
  if (typeof localStorage !== 'undefined') {
    try { return JSON.parse(localStorage.getItem('pony-app-config') ?? '{}') as AutoProxyConfig } catch { return {} }
  }
  return {}
}

export async function saveAutoProxyConfig(patch: AutoProxyConfig): Promise<void> {
  if (isTauri()) {
    await invoke('app_config_set', { patch })
  } else if (typeof localStorage !== 'undefined') {
    const cur = await loadAutoProxyConfig()
    localStorage.setItem('pony-app-config', JSON.stringify({ ...cur, ...patch }))
  }
}
