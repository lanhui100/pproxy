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
