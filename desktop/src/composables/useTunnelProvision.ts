// 隧道中继自动配置（唯一出口）：网关下发 → 本机装配，用户零理解成本。
// 流程：读本地（loadTunnelConfig）→ 缺失则拉网关 /api/tunnel/config →
// 有值即经 saveTunnelConfig 落盘（文件 + 系统凭据库）。
// 任何一环失败都不打扰，返回状态由调用方决定提示方式。
import { api } from '@/api/client'
import { loadTunnelConfig, saveTunnelConfig } from '@/lib/config'

export type ProvisionOutcome =
  | 'ready' // 已就绪（含本次自动补齐）
  | 'unavailable' // 网关未下发隧道配置
  | 'unsupported' // 非 Tauri 环境（浏览器 dev）

/** 装配隧道配置；不抛错，返回状态枚举。 */
export async function provisionTunnel(): Promise<ProvisionOutcome> {
  try {
    const local = await loadTunnelConfig()
    if (local.url && local.hasToken) return 'ready'

    // 本地缺配置 → 向网关拉取下发值
    const remote = await api.tunnelConfig({ skipAuthRedirect: true })
    if (!remote.url || !remote.token) return 'unavailable'

    // 二者齐备才落盘（与引擎「部分配置不启用」语义一致）
    await saveTunnelConfig(remote.url, remote.token)
    return 'ready'
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
