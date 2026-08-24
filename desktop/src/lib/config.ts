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

export function saveBackendUrl(url: string): void {
  localStorage.setItem(BACKEND_URL_KEY, url.replace(/\/$/, ''))
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

export async function clearAdminToken(): Promise<void> {
  if (isTauri()) {
    await invoke('credential_delete').catch(() => {})
  } else if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(DEV_TOKEN_KEY)
  }
}

/** 轮询间隔归一化：0 = 不自动轮询（合法档位）；负数/NaN 回落 5；其余下限 1。 */
export function normalizePollMin(v: number): number {
  if (!Number.isFinite(v)) return 5
  if (v === 0) return 0
  return Math.max(1, Math.floor(v))
}

function readPollMin(): number {
  if (typeof localStorage === 'undefined') return 5
  return normalizePollMin(Number(localStorage.getItem(POLL_INTERVAL_KEY)))
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
