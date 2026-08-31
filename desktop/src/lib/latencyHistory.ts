export interface LatencyPoint {
  ts: number
  ok: boolean
  ms?: number
  err?: string
}

export type LatencyTone = 'ok' | 'warn' | 'error' | 'empty'

const DEFAULT_FAST_MS = 800
const DEFAULT_WARN_MS = 2000
const DEFAULT_MAX_SLOTS = 12

/** 展示窗口：仅保留最近 2 小时的采样点（10 分钟轮询 → 12 根柱条） */
export const WINDOW_MS = 2 * 60 * 60 * 1000
/** localStorage 持久化上限（多于展示槽位，重启后仍覆盖完整窗口） */
const STORAGE_MAX_POINTS = 24
const STORAGE_PREFIX = 'pony-latency:'

/**
 * 评估单个采样点的颜色状态
 * - <= 800ms: 绿色 (ok)
 * - 800ms ~ 2000ms: 黄色 (warn)
 * - > 2000ms 或失败: 红色 (error)
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
 * 追加或更新最新时序点，保持最近 2 小时（相对最新采样点裁剪，窗口外旧点丢弃）。
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
  // 窗口裁剪：相对最新点回溯 2 小时
  const cutoff = point.ts - WINDOW_MS
  return next.filter((p) => p.ts >= cutoff).slice(-maxSlots)
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

// ---- 本地持久化：每条时序（接口/站点 × 出网接口）一个 key，重启后保留窗口内记录 ----

function storageAvailable(): boolean {
  return typeof localStorage !== 'undefined'
}

/** 读取指定时序的历史点（自动丢弃 2 小时窗口外的旧记录；异常一律返回空数组） */
export function loadLatencySeries(key: string): LatencyPoint[] {
  if (!storageAvailable()) return []
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + key)
    if (!raw) return []
    const arr = JSON.parse(raw) as unknown
    if (!Array.isArray(arr)) return []
    const cutoff = Date.now() - WINDOW_MS
    return arr.filter(
      (p): p is LatencyPoint =>
        Boolean(p) && typeof (p as LatencyPoint).ts === 'number' && (p as LatencyPoint).ts >= cutoff,
    )
  } catch {
    return []
  }
}

/** 保存指定时序的历史点（只保留窗口内且不超过存储上限） */
export function saveLatencySeries(key: string, history: LatencyPoint[]): void {
  if (!storageAvailable()) return
  try {
    const cutoff = Date.now() - WINDOW_MS
    const trimmed = history.filter((p) => p.ts >= cutoff).slice(-STORAGE_MAX_POINTS)
    localStorage.setItem(STORAGE_PREFIX + key, JSON.stringify(trimmed))
  } catch {
    // 存储满/隐私模式下静默失败：时序只是展示增强，不影响功能
  }
}
