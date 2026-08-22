// 告警通知轮询（M5 §5）：unread alerts → OS 通知；id 去重集 cap 5000 FIFO；
// 平台坑兜底：应用内未读横幅常驻 + 前台恢复立即刷新（F18）。
import { onMounted, onUnmounted, ref } from 'vue'

import { api } from '@/api/client'
import { isTauri } from '@/lib/config'

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

export function useAlertNotifications(pollIntervalMinRef: () => number) {
  const lastError = ref('')

  async function pollOnce(): Promise<void> {
    try {
      const r = await api.alerts(true, 50)
      unreadCount.value = r.alerts.length
      lastError.value = ''
      const seen = new Set(loadDedup())
      for (const a of r.alerts) {
        if (seen.has(a.id)) continue
        seen.add(a.id)
        await notifyOs(`配额告警 [${a.level}]`, a.message)
      }
      if (seen.size > DEDUP_CAP) saveDedup([...seen].slice(-DEDUP_CAP))
      else saveDedup([...seen])
    } catch (e) {
      lastError.value = String(e)
    }
  }

  function start(): void {
    void pollOnce()
    timer = setInterval(() => void pollOnce(), pollIntervalMinRef() * 60_000)
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

  onMounted(start)
  onUnmounted(stop)
  return { unreadCount, lastError, refresh: pollOnce }
}
