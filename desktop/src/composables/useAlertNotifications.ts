// 告警通知轮询（M5 §5）：unread alerts → OS 通知；id 去重集 cap 5000 FIFO；
// 平台坑兜底：应用内未读横幅常驻 + 前台恢复立即刷新（F18）。
// M7：间隔响应式热生效；0 = 不自动轮询（仍保留前台恢复与手动刷新，不建 interval）。
import { onMounted, onUnmounted, ref, watch } from 'vue'

import { api } from '@/api/client'
import { alertLevelLabel } from '@/lib/format'
import { isTauri, pollIntervalMin } from '@/lib/config'

const DEDUP_KEY = 'pony-notified-alerts'
const DEDUP_CAP = 5000

function loadDedup(): number[] {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(DEDUP_KEY) ?? '[]')
    return Array.isArray(raw) ? raw.filter((v): v is number => typeof v === 'number') : []
  } catch {
    return []
  }
}

function saveDedup(ids: number[]): void {
  localStorage.setItem(DEDUP_KEY, JSON.stringify(ids.slice(-DEDUP_CAP)))
}

async function notifyOs(title: string, body: string): Promise<void> {
  if (!isTauri()) return // 浏览器 dev 仅横幅
  const mod = await import('@tauri-apps/plugin-notification')
  if ((await mod.isPermissionGranted()) === false) {
    const granted = await mod.requestPermission()
    if (granted !== 'granted') return
  }
  await mod.sendNotification({ title, body })
}

/** 应用级单例状态：未读数供侧栏/横幅展示。 */
const unreadCount = ref(0)
let timer: ReturnType<typeof setInterval> | null = null
let visibilityHandler: (() => void) | null = null

export function useAlertNotifications() {
  const lastError = ref('')

  async function pollOnce(): Promise<void> {
    try {
      // 后台轮询豁免 401：未授权只静默降级，绝不能把用户踢到设置页
      const r = await api.alerts(true, 50, { skipAuthRedirect: true })
      unreadCount.value = r.alerts.length
      lastError.value = ''
      const seen = new Set(loadDedup())
      for (const a of r.alerts) {
        if (seen.has(a.id)) continue
        seen.add(a.id)
        await notifyOs('用量告警', `${alertLevelLabel(a.level)} · ${a.message}`)
      }
      if (seen.size > DEDUP_CAP) saveDedup([...seen].slice(-DEDUP_CAP))
      else saveDedup([...seen])
    } catch (e) {
      lastError.value = String(e)
    }
  }

  /** 建/重建 interval；0 档只清不建（前台恢复与手动刷新仍可用）。 */
  function armTimer(): void {
    if (timer) clearInterval(timer)
    timer = null
    if (pollIntervalMin.value > 0) {
      timer = setInterval(() => void pollOnce(), pollIntervalMin.value * 60_000)
    }
  }

  function start(): void {
    void pollOnce()
    armTimer()
    visibilityHandler = () => {
      if (document.visibilityState === 'visible') void pollOnce()
    }
    document.addEventListener('visibilitychange', visibilityHandler)
  }

  function stop(): void {
    if (timer) clearInterval(timer)
    if (visibilityHandler) document.removeEventListener('visibilitychange', visibilityHandler)
    timer = null
  }

  // 设置页改档位即时生效：仅重建 timer，不动监听器
  watch(pollIntervalMin, () => armTimer())

  onMounted(start)
  onUnmounted(stop)
  return { unreadCount, lastError, refresh: pollOnce }
}
