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
 * 追加或更新最新时序点，保持最近 1 小时（12 个点）
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
