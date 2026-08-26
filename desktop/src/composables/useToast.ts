// 全局 toast 单例队列（spec §9.1）：模块级 ref，壳层只挂一个 <ToastHost> 消费。
// success/info 3s 自动消失；error 不自动消失——仅用户关闭或「复制错误」后延时消失，
// 便于把错误信息粘贴给 AI 排障。id 自增；定时器随条目清理防泄漏。
import { ref } from 'vue'

export type ToastKind = 'success' | 'info' | 'error'

export interface ToastItem {
  id: number
  kind: ToastKind
  message: string
  /** 复制用的完整细节（如原始报错）；缺省复制 message 本身。 */
  detail?: string
}

const DURATION_MS: Record<ToastKind, number> = { success: 3000, info: 3000, error: 0 } // 0=不自动消失
export const COPY_DISMISS_DELAY_MS = 1600

const toasts = ref<ToastItem[]>([])
let nextId = 1
const timers = new Map<number, ReturnType<typeof setTimeout>>()

function push(kind: ToastKind, message: string, detail?: string): number {
  const id = nextId++
  toasts.value.push({ id, kind, message, detail })
  const ms = DURATION_MS[kind]
  if (ms > 0) arm(id, ms)
  return id
}

function arm(id: number, ms: number): void {
  timers.set(
    id,
    setTimeout(() => dismiss(id), ms),
  )
}

function dismiss(id: number): void {
  const t = timers.get(id)
  if (t) clearTimeout(t)
  timers.delete(id)
  toasts.value = toasts.value.filter((x) => x.id !== id)
}

/** 复制后延时消失：给用户「已复制」的确认窗口。 */
function dismissAfterCopy(id: number): void {
  disarm(id)
  arm(id, COPY_DISMISS_DELAY_MS)
}

function disarm(id: number): void {
  const t = timers.get(id)
  if (t) clearTimeout(t)
  timers.delete(id)
}

export function useToast() {
  return {
    toasts,
    success: (message: string, detail?: string) => push('success', message, detail),
    error: (message: string, detail?: string) => push('error', message, detail),
    info: (message: string, detail?: string) => push('info', message, detail),
    dismiss,
    dismissAfterCopy,
  }
}
