/**
 * 用量统计图表纯函数工具：格式化、字节单位、本地日期 key、极简单柱（CF橙黄）用量图模型构建。
 * 抽离自 DashboardView.vue 以便单元测试。
 */

export interface NiceScale {
  step: number
  max: number
  ticks: number[]
}

/**
 * nice-number 刻度：按最大值自动选 1/2/5×10^n 步长，保证不同量级下刻度可读。
 */
export function niceScale(max: number, tickCount = 4, integerOnly = false): NiceScale {
  if (!Number.isFinite(max) || max <= 0) {
    return { step: 1, max: tickCount, ticks: Array.from({ length: tickCount + 1 }, (_, i) => i) }
  }
  const raw = max / tickCount
  const mag = 10 ** Math.floor(Math.log10(raw))
  const norm = raw / mag
  let step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag
  if (integerOnly && step < 1) step = 1
  const n = Math.ceil(max / step)
  return { step, max: n * step, ticks: Array.from({ length: n + 1 }, (_, i) => i * step) }
}

/** 去掉 toFixed 后的多余尾零：1.0 -> "1"，1.5 -> "1.5" */
function trimNum(v: number): string {
  return `${parseFloat(v.toFixed(1))}`
}

/** 刻度文案：大数值缩写为 k / M */
export function formatCount(v: number): string {
  if (v >= 1e6) return `${trimNum(v / 1e6)}M`
  if (v >= 1e3) return `${trimNum(v / 1e3)}k`
  return trimNum(v)
}

export interface ByteUnit {
  div: number
  suffix: string
}

/** 字节单位：按最大值动态选 B/KB/MB/GB，超出 GB 仍按 GB 显示 */
export function byteUnit(maxBytes: number): ByteUnit {
  if (maxBytes < 1024) return { div: 1, suffix: 'B' }
  if (maxBytes < 1024 ** 2) return { div: 1024, suffix: 'KB' }
  if (maxBytes < 1024 ** 3) return { div: 1024 ** 2, suffix: 'MB' }
  return { div: 1024 ** 3, suffix: 'GB' }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`
  return `${(n / 1024 ** 3).toFixed(2)} GB`
}

/** 本地时区 YYYY-MM-DD（与 weekDays 拼桶口径一致，勿用 toISOString 的 UTC 日） */
export function localDateKey(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export interface MergedUsageBar {
  x: number
  y: number
  w: number
  h: number
  bytes: number
  reqs: number
  title: string
}

export interface MergedUsageDay {
  label: string
  x: number
  isToday: boolean
}

export interface MergedUsageChartModel {
  W: number
  H: number
  T: number
  B: number
  baselineY: number
  bars: MergedUsageBar[]
  days: MergedUsageDay[]
  maxBytes: number
}

/**
 * 构建极简单柱（经典 CF 橙黄色）7 日用量图模型
 */
export function buildMergedUsageChart(
  days: { date: string; label: string; cfReq: number; vReq: number; cfBytes: number; vBytes: number }[],
  W = 320,
  H = 72,
): MergedUsageChartModel {
  const T = 6 // 顶部微边距
  const B = 16 // 底部日期文字空间
  const ph = H - T - B
  const baselineY = T + ph // 严格基准线（柱底与波形底部）

  const dailyTotals = days.map((d) => ({
    ...d,
    totalBytes: d.cfBytes + d.vBytes,
    totalReqs: d.cfReq + d.vReq,
  }))

  const maxBytes = Math.max(1, ...dailyTotals.map((d) => d.totalBytes))
  const n = Math.max(1, days.length)

  // 每天单根经典柱体（CF橙黄色，柱宽 8px，天间距 10px，紧凑居中）
  const barW = 8
  const dayGap = 10
  const totalSpan = n * barW + (n - 1) * dayGap
  const startX = Math.max(6, (W - totalSpan) / 2)

  const bars: MergedUsageBar[] = []
  const dayLabels: MergedUsageDay[] = []

  dailyTotals.forEach((d, i) => {
    const barX = startX + i * (barW + dayGap)
    const cx = barX + barW / 2
    const isToday = i === days.length - 1

    const barH = d.totalBytes > 0 ? Math.max(2, (d.totalBytes / maxBytes) * ph) : 0

    bars.push({
      x: barX,
      y: baselineY - barH,
      w: barW,
      h: barH,
      bytes: d.totalBytes,
      reqs: d.totalReqs,
      title: `${d.date}\n总用量: ${formatBytes(d.totalBytes)}\n调用次数: ${d.totalReqs} 次 (CF: ${d.cfReq} · Vercel: ${d.vReq})`,
    })

    dayLabels.push({
      label: isToday ? '今天' : d.label,
      x: cx,
      isToday,
    })
  })

  return {
    W,
    H,
    T,
    B,
    baselineY,
    bars,
    days: dayLabels,
    maxBytes,
  }
}
