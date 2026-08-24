// 未配置全局门槛（spec §3.1）：消费 lib/config.ts 的响应式源 backendUrlSaved，
// saveBackendUrl 写路径同步该 ref → 保存成功后门槛即时解锁（R2-UX-1/ENG-3 修复）。
import { computed, type ComputedRef } from 'vue'

import { backendUrlSaved } from '@/lib/config'

export function useBackendGate(): { configured: ComputedRef<boolean> } {
  const configured = computed(() => backendUrlSaved.value !== '')
  return { configured }
}
