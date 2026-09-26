import { ref } from 'vue'
import { isTauri } from '@/lib/config'

// 全局应用状态：是否已完成初始化配置
export const isConfigured = ref(false)

/**
 * 刷新全局配置状态
 */
export async function refreshAppConfigured(): Promise<boolean> {
  if (!isTauri()) {
    const devToken = typeof localStorage !== 'undefined' ? localStorage.getItem('pony-dev-tunnel-token') : null
    const devConfig = typeof localStorage !== 'undefined' ? localStorage.getItem('pony-app-config') : null
    let configured = Boolean(devToken)
    if (devConfig) {
      try {
        const parsed = JSON.parse(devConfig)
        if (typeof parsed.configured === 'boolean') {
          configured = parsed.configured
        }
      } catch {}
    }
    isConfigured.value = configured
    return configured
  }

  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const cfg = (await invoke('proxy_get_current_config')) as { configured?: boolean }
    const configured = Boolean(cfg?.configured)
    isConfigured.value = configured
    return configured
  } catch (e) {
    console.error('Failed to get current config:', e)
    return isConfigured.value
  }
}

/**
 * 设置全局配置状态
 */
export function setAppConfigured(val: boolean) {
  isConfigured.value = val
}
