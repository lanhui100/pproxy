// 凭据与端点配置存取（M5 §4 Settings / §6.4 安全横切）：
// - Tauri 环境：admin token 走 OS 凭据库（keyring 命令），地址走 localStorage
// - 浏览器 dev：两者均 localStorage（便利通道，打包产物不含此路径）
import { ref } from 'vue'

import { invoke } from '@tauri-apps/api/core'

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

const BACKEND_URL_KEY = 'pony-backend-url'
const DATA_PLANE_URL_KEY = 'pony-data-plane-url'
const DEV_TOKEN_KEY = 'pony-dev-admin-token'
const POLL_INTERVAL_KEY = 'pony-poll-interval-min'

export function loadBackendUrl(): string {
  if (typeof localStorage === 'undefined') return ''
  return localStorage.getItem(BACKEND_URL_KEY) ?? ''
}

/** 管理面地址响应式源：saveBackendUrl 写路径同步，驱动壳层未配置门槛热解锁（R2-UX-1/ENG-3）。 */
export const backendUrlSaved = ref<string>(loadBackendUrl())

export function saveBackendUrl(url: string): void {
  const v = url.replace(/\/$/, '')
  localStorage.setItem(BACKEND_URL_KEY, v)
  backendUrlSaved.value = v
}

/** 数据面地址（接入 base_url 的底座，默认 8899 或公网入口）；管理面无需公网可达。 */
export function loadDataPlaneUrl(): string {
  if (typeof localStorage === 'undefined') return ''
  return localStorage.getItem(DATA_PLANE_URL_KEY) ?? ''
}

export function saveDataPlaneUrl(url: string): void {
  localStorage.setItem(DATA_PLANE_URL_KEY, url.trim().replace(/\/$/, ''))
}

/** admin token 写入：Tauri→凭据库；浏览器 dev→localStorage。 */
export async function saveAdminToken(token: string): Promise<void> {
  if (isTauri()) {
    await invoke('credential_set', { secret: token })
  } else if (typeof localStorage !== 'undefined') {
    localStorage.setItem(DEV_TOKEN_KEY, token)
  }
}

/** admin token 清除：返回是否确认成功（keyring 删除失败返回 false，调用方如实反馈）。 */
export async function clearAdminToken(): Promise<boolean> {
  if (isTauri()) {
    try {
      await invoke('credential_delete')
      return true
    } catch {
      return false
    }
  }
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(DEV_TOKEN_KEY)
  }
  return true
}

/** 轮询间隔归一化：0 = 不自动轮询（合法档位）；负数/NaN 回落 5；其余下限 1。 */
export function normalizePollMin(v: number): number {
  if (!Number.isFinite(v) || v < 0) return 5
  if (v === 0) return 0
  return Math.max(1, Math.floor(v))
}

function readPollMin(): number {
  if (typeof localStorage === 'undefined') return 5
  const raw = localStorage.getItem(POLL_INTERVAL_KEY)
  // 仅显式存储的值参与往返（"0"=手动档）；key 缺失/空串回落默认 5（R2-ENG-2）
  return raw === null || raw.trim() === '' ? 5 : normalizePollMin(Number(raw))
}

/** 轮询间隔模块级单例（响应式）：设置页改动即时持久化并热生效。 */
export const pollIntervalMin = ref<number>(readPollMin())

/**
 * @deprecated 兼容旧调用；请直接消费响应式 pollIntervalMin。
 */
export function loadPollIntervalMin(): number {
  return pollIntervalMin.value
}

export function savePollIntervalMin(min: number): void {
  const v = normalizePollMin(min)
  pollIntervalMin.value = v
  localStorage.setItem(POLL_INTERVAL_KEY, String(v))
}

// ---- 隧道中继配置（2026-08 审计整改：端点/令牌 opt-in，令牌仅进 OS 凭据库）----

export interface TunnelConfig {
  url: string
  hasToken: boolean
}

/** 读取隧道配置（Rust 侧文件 + 凭据库探测，不回传令牌明文）。 */
export async function loadTunnelConfig(): Promise<TunnelConfig> {
  if (!isTauri()) {
    const url = localStorage.getItem('pony-tunnel-url') ?? ''
    const hasToken = Boolean(localStorage.getItem('pony-dev-tunnel-token'))
    return { url, hasToken }
  }
  return invoke<TunnelConfig>('proxy_tunnel_get')
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

// ---- auto_proxy 首次询问（Fix4 v0.2）：默认 false，缺字段时弹窗询问 ----
export interface AutoProxyConfig { auto_proxy?: boolean; dont_ask?: boolean }

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
