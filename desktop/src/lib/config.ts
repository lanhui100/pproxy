// 凭据与端点配置存取（M5 §4 Settings / §6.4 安全横切）：
// - Tauri 环境：admin token 走 OS 凭据库（keyring 命令），地址走 localStorage
// - 浏览器 dev：两者均 localStorage（便利通道，打包产物不含此路径）
import { invoke } from '@tauri-apps/api/core'

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

const BACKEND_URL_KEY = 'pony-backend-url'
const DEV_TOKEN_KEY = 'pony-dev-admin-token'
const POLL_INTERVAL_KEY = 'pony-poll-interval-min'

export function loadBackendUrl(): string {
  if (typeof localStorage === 'undefined') return ''
  return localStorage.getItem(BACKEND_URL_KEY) ?? ''
}

export function saveBackendUrl(url: string): void {
  localStorage.setItem(BACKEND_URL_KEY, url.replace(/\/$/, ''))
}

/** admin token 写入：Tauri→凭据库；浏览器 dev→localStorage。 */
export async function saveAdminToken(token: string): Promise<void> {
  if (isTauri()) {
    await invoke('credential_set', { secret: token })
  } else if (typeof localStorage !== 'undefined') {
    localStorage.setItem(DEV_TOKEN_KEY, token)
  }
}

export async function clearAdminToken(): Promise<void> {
  if (isTauri()) {
    await invoke('credential_delete').catch(() => {})
  } else if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(DEV_TOKEN_KEY)
  }
}

export function loadPollIntervalMin(): number {
  if (typeof localStorage === 'undefined') return 5
  const v = Number(localStorage.getItem(POLL_INTERVAL_KEY))
  return Number.isFinite(v) && v >= 1 ? v : 5
}

export function savePollIntervalMin(min: number): void {
  localStorage.setItem(POLL_INTERVAL_KEY, String(Math.max(1, Math.floor(min))))
}
