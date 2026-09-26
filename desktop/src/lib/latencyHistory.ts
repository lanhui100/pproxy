export interface LatencyPoint {
  ts: number
  ok: boolean
  ms?: number
  err?: string
}

export type LatencyTone = 'ok' | 'warn' | 'error' | 'empty'

const DEFAULT_FAST_MS = 2000
const DEFAULT_WARN_MS = 5000
const DEFAULT_MAX_SLOTS = 12

/** 展示窗口：仅保留最近 2 小时的采样点（10 分钟轮询 → 12 根柱条） */
export const WINDOW_MS = 2 * 60 * 60 * 1000
const STORAGE_PREFIX = 'pony-latency:'

/**
 * 评估单个采样点的颜色状态
 * - <= 2000ms: 绿色 (ok)
 * - 2000ms ~ 5000ms: 黄色 (warn)
 * - > 5000ms 或失败: 红色 (error)
 * - 空白/未探测: 灰色 (empty)
 */
export function getLatencyTone(
  point?: LatencyPoint,
  fastThreshold = DEFAULT_FAST_MS,
  warnThreshold = DEFAULT_WARN_MS,
): LatencyTone {
  if (!point) return 'empty'
  if (!point.ok) return 'error'
  const ms = point.ms ?? 0
  if (ms <= fastThreshold) return 'ok'
  if (ms <= warnThreshold) return 'warn'
  return 'error'
}

/**
 * 追加或更新最新时序点，保持连续历史记录（最多保留 maxSlots 个槽位）。
 */
export function appendLatencyPoint(
  history: LatencyPoint[] = [],
  point: LatencyPoint,
  maxSlots = DEFAULT_MAX_SLOTS,
): LatencyPoint[] {
  const next = [...history]
  const last = next[next.length - 1]
  if (last && Math.abs(point.ts - last.ts) < 150_000) {
    next[next.length - 1] = point
  } else {
    next.push(point)
  }
  // 保持最近 maxSlots 个采样点，连续保留历史
  return next.slice(-maxSlots)
}

/**
 * 补齐 12 个槽位的完整数组，空槽位以 undefined 填充
 */
export function padLatencySlots(
  history: LatencyPoint[] = [],
  totalSlots = DEFAULT_MAX_SLOTS,
): (LatencyPoint | undefined)[] {
  const result: (LatencyPoint | undefined)[] = []
  const emptyCount = Math.max(0, totalSlots - history.length)
  for (let i = 0; i < emptyCount; i++) {
    result.push(undefined)
  }
  for (const item of history.slice(-totalSlots)) {
    result.push(item)
  }
  return result
}

/**
 * 格式化柱条的 Tooltip 说明
 */
export function formatSlotTooltip(point?: LatencyPoint): string {
  if (!point) return '暂无探测记录'
  const d = new Date(point.ts)
  const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  if (!point.ok) {
    return `${time} · 探测异常: ${point.err || '超时'}`
  }
  return `${time} · 延迟 ${point.ms ?? 0}ms`
}

// ---- 本地持久化：每条时序（接口/站点 × 出网接口）一个 key，重启后保留历史记录 ----

function storageAvailable(): boolean {
  return typeof localStorage !== 'undefined'
}

/** 读取指定时序的历史点（保留最近 maxSlots 个历史点，保证重启后观测连续性） */
export function loadLatencySeries(key: string, maxSlots = DEFAULT_MAX_SLOTS): LatencyPoint[] {
  if (!storageAvailable()) return []
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + key)
    if (!raw) return []
    const arr = JSON.parse(raw) as unknown
    if (!Array.isArray(arr)) return []
    return arr
      .filter(
        (p): p is LatencyPoint =>
          Boolean(p) &&
          typeof (p as LatencyPoint).ts === 'number' &&
          typeof (p as LatencyPoint).ok === 'boolean',
      )
      .slice(-maxSlots)
  } catch {
    return []
  }
}

/** 保存指定时序的历史点（保留最近 maxSlots 个历史记录） */
export function saveLatencySeries(key: string, history: LatencyPoint[], maxSlots = DEFAULT_MAX_SLOTS): void {
  if (!storageAvailable()) return
  try {
    const trimmed = history.slice(-maxSlots)
    localStorage.setItem(STORAGE_PREFIX + key, JSON.stringify(trimmed))
  } catch {
    // 存储满/隐私模式下静默失败：时序只是展示增强，不影响功能
  }
}
