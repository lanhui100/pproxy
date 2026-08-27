import { getCurrentInstance, onUnmounted, ref } from 'vue'

export interface AdaptivePollOptions {
  baseIntervalMs?: number
  maxIntervalMs?: number
  immediate?: boolean
}

export function useAdaptivePoll(
  pollFn: () => Promise<void>,
  options: AdaptivePollOptions = {},
) {
  const baseInterval = options.baseIntervalMs ?? 15_000
  const maxInterval = options.maxIntervalMs ?? 300_000
  const immediate = options.immediate ?? true

  let timer: ReturnType<typeof setTimeout> | null = null
  let failureCount = 0
  let isRunning = false
  let inFlightPromise: Promise<void> | null = null
  let executionEpoch = 0
  const isPolling = ref(false)

  async function execute(isManual = false): Promise<void> {
    if (!isRunning && !isManual) return

    // 后台不可见且非手动触发时休眠，不消耗资源
    if (!isManual && typeof document !== 'undefined' && document.visibilityState !== 'visible') {
      return
    }

    // 互斥：已有在途请求时直接复用，杜绝多路并发重叠
    if (inFlightPromise) {
      return inFlightPromise
    }

    if (timer) {
      clearTimeout(timer)
      timer = null
    }

    const currentEpoch = ++executionEpoch
    isPolling.value = true

    inFlightPromise = (async () => {
      try {
        await pollFn()
        if (currentEpoch === executionEpoch && isRunning) {
          failureCount = 0
        }
      } catch (err) {
        if (currentEpoch === executionEpoch && isRunning) {
          failureCount++
        }
        throw err
      } finally {
        inFlightPromise = null
        if (currentEpoch === executionEpoch) {
          isPolling.value = false
          if (isRunning) {
            scheduleNext()
          }
        }
      }
    })()

    return inFlightPromise
  }

  function scheduleNext(): void {
    if (!isRunning) return
    if (timer) clearTimeout(timer)

    // 指数退避 (base * 1.5^failures) + 20% Jitter 抖动
    const backoff = Math.min(baseInterval * Math.pow(1.5, failureCount), maxInterval)
    const jitter = backoff * (0.8 + Math.random() * 0.4)

    timer = setTimeout(() => {
      void execute(false)
    }, jitter)
  }

  function onVisibilityChange(): void {
    if (typeof document !== 'undefined' && document.visibilityState === 'visible' && isRunning) {
      failureCount = 0
      void execute(false)
    }
  }

  function start(): void {
    if (isRunning) return
    isRunning = true
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange)
    }
    if (immediate) {
      void execute(false)
    } else {
      scheduleNext()
    }
  }

  function stop(): void {
    isRunning = false
    executionEpoch++
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
    isPolling.value = false
  }

  function refreshNow(): Promise<void> {
    failureCount = 0
    return execute(true).catch(() => {})
  }

  if (getCurrentInstance()) {
    onUnmounted(() => {
      stop()
    })
  }

  return {
    start,
    stop,
    refreshNow,
    isPolling,
  }
}
