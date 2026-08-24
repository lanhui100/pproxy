// 全局 toast 单例队列（spec §9.1）：模块级 ref，壳层只挂一个 <ToastHost> 消费。
// success/info 3s、error 6s 自动消失；手动 dismiss(id)；id 自增；定时器随条目清理防泄漏。
import { ref } from 'vue'

export type ToastKind = 'success' | 'info' | 'error'

export interface ToastItem {
  id: number
  kind: ToastKind
  message: string
}

const DURATION_MS: Record<ToastKind, number> = { success: 3000, info: 3000, error: 6000 }

const toasts = ref<ToastItem[]>([])
let nextId = 1
const timers = new Map<number, ReturnType<typeof setTimeout>>()

function push(kind: ToastKind, message: string): number {
  const id = nextId++
  toasts.value.push({ id, kind, message })
  timers.set(
    id,
    setTimeout(() => dismiss(id), DURATION_MS[kind]),
  )
  return id
}

function dismiss(id: number): void {
  const t = timers.get(id)
  if (t) clearTimeout(t)
  timers.delete(id)
  toasts.value = toasts.value.filter((x) => x.id !== id)
}

export function useToast() {
  return {
    toasts,
    success: (message: string) => push('success', message),
    error: (message: string) => push('error', message),
    info: (message: string) => push('info', message),
    dismiss,
  }
}
