// 未配置全局门槛（spec §3.1）：每次视图 setup 时新鲜读取 localStorage，
// Settings 保存后跨页导航自然刷新，无需事件总线。key 与 lib/config.ts 的
// BACKEND_URL_KEY 对齐（该 key 属 F2 所有权，此处按 spec 冻结字面量读取）。
import { computed, type ComputedRef } from 'vue'

export function useBackendGate(): { configured: ComputedRef<boolean> } {
  const configured = computed(() => {
    if (typeof localStorage === 'undefined') return false
    return Boolean(localStorage.getItem('pony-backend-url'))
  })
  return { configured }
}
