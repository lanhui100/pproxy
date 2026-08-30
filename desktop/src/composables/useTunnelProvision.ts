// 隧道配置就绪检查（单体架构）：只读本地（tunnel.json + 系统凭据库）。
// 旧版会从远端网关拉取下发值——远端管理面已随单体化废弃，
// 隧道配置现在完全由用户在「设置 → 隧道中继」手动维护。
import { loadTunnelConfig } from '@/lib/config'

export type ProvisionOutcome =
  | 'ready' // 本地端点 + 令牌齐备
  | 'unavailable' // 缺少端点或令牌

/** 隧道是否已就绪；不抛错，返回状态枚举。 */
export async function provisionTunnel(): Promise<ProvisionOutcome> {
  try {
    const local = await loadTunnelConfig()
    return local.url && local.hasToken ? 'ready' : 'unavailable'
  } catch {
    return 'unavailable'
  }
}

/** 本地隧道是否已就绪（仅供展示；不做网络请求）。 */
export async function localTunnelReady(): Promise<boolean> {
  try {
    const local = await loadTunnelConfig()
    return Boolean(local.url && local.hasToken)
  } catch {
    return false
  }
}
