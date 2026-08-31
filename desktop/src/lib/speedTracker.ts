/**
 * 实时网络速率追踪与波形图生成纯函数工具。
 * 抽离自 DashboardView.vue，用于单元测试与精准状态流转。
 */

export interface SpeedSample {
  down: number // bytes per second
  up: number // bytes per second
  ts: number
}

export interface SpeedCalcInput {
  up: number
  down: number
  ts: number
}

export interface SpeedWaveform {
  points: string
  area: string
  maxSpeed: number
}

export interface SpeedParts {
  val: number
  unit: string
}

/**
 * 将实时速率格式化为无小数的整数数值（0~999 三位数以内）和独立单位，便于定宽防抖布局
 */
export function formatSpeedParts(bytesPerSec: number): SpeedParts {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) {
    return { val: 0, unit: 'B/s' }
  }
  if (bytesPerSec < 1024) {
    return { val: Math.min(999, Math.round(bytesPerSec)), unit: 'B/s' }
  }
  if (bytesPerSec < 1024 ** 2) {
    return { val: Math.min(999, Math.round(bytesPerSec / 1024)), unit: 'KB/s' }
  }
  if (bytesPerSec < 1024 ** 3) {
    return { val: Math.min(999, Math.round(bytesPerSec / 1024 ** 2)), unit: 'MB/s' }
  }
  return { val: Math.min(999, Math.round(bytesPerSec / 1024 ** 3)), unit: 'GB/s' }
}

/**
 * 格式化字节速率为易读文本（无小数，如 "12 KB/s", "0 B/s"）
 */
export function formatSpeed(bytesPerSec: number): string {
  const parts = formatSpeedParts(bytesPerSec)
  return `${parts.val} ${parts.unit}`
}

/**
 * 计算两次采样之间的实时上下行速率（bytes/s）
 * 包含时钟回退、计数器归零重置的保护。
 */
export function calculateSpeed(
  prev: SpeedCalcInput,
  cur: SpeedCalcInput,
): { up: number; down: number } {
  const deltaMs = cur.ts - prev.ts
  if (deltaMs < 100) {
    // 采样间隔过短（< 100ms），避免分母过小导致速率剧烈抖动
    return { up: 0, down: 0 }
  }
  const deltaSec = deltaMs / 1000
  const dUp = cur.up - prev.up
  const dDown = cur.down - prev.down

  return {
    up: dUp > 0 ? Math.round(dUp / deltaSec) : 0,
    down: dDown > 0 ? Math.round(dDown / deltaSec) : 0,
  }
}

/**
 * 追加最新测速采样，保持滑动窗口最大容量
 */
export function appendSpeedSample(
  history: SpeedSample[] = [],
  sample: SpeedSample,
  maxSlots = 20,
): SpeedSample[] {
  const next = [...history, sample]
  if (next.length > maxSlots) {
    return next.slice(-maxSlots)
  }
  return next
}

/**
 * 根据历史采样点生成 SVG Sparkline 波形图（折线路径 + 封闭面积路径）
 * - 补齐固定槽位数，保持从右向左实时流动
 * - 支持自定义 topPad 与 bottomPad，保证波形基线精准贴合在图表基准线（如日期标签上方）
 */
export function generateSpeedWaveform(
  history: SpeedSample[] = [],
  width = 96,
  height = 18,
  maxSlots = 20,
  topPad = 2,
  bottomPad = 1,
): SpeedWaveform {
  const plotH = Math.max(1, height - topPad - bottomPad)

  // 截取并补齐到 maxSlots 个点
  const recent = history.slice(-maxSlots)
  const padded: number[] = []
  const emptyCount = Math.max(0, maxSlots - recent.length)
  for (let i = 0; i < emptyCount; i++) {
    padded.push(0)
  }
  for (const s of recent) {
    padded.push(Math.max(0, s.down))
  }

  const rawMax = Math.max(0, ...padded)
  // 设立最低基线（如 10 KB/s），避免 0 流量时微小抖动放大成满格
  const effectiveMax = Math.max(rawMax, 10 * 1024)

  const stepX = width / Math.max(1, maxSlots - 1)
  const coords: { x: number; y: number }[] = padded.map((v, i) => {
    const x = parseFloat((i * stepX).toFixed(1))
    const ratio = Math.min(1, v / effectiveMax)
    const y = parseFloat((topPad + plotH * (1 - ratio)).toFixed(1))
    return { x, y }
  })

  const pointsStr = coords.map((c) => `${c.x},${c.y}`).join(' ')
  const baselineY = height - bottomPad
  const firstX = coords[0]?.x ?? 0
  const lastX = coords[coords.length - 1]?.x ?? width
  const areaPath = `M ${firstX},${baselineY} L ${coords.map((c) => `${c.x},${c.y}`).join(' L ')} L ${lastX},${baselineY} Z`

  return {
    points: pointsStr,
    area: areaPath,
    maxSpeed: rawMax,
  }
}
