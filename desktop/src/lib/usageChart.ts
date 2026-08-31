/**
 * 用量统计双轴图的纯函数工具：nice-number 刻度、计数格式化、字节单位、本地日期 key。
 * 抽离自 DashboardView.vue 以便单元测试。
 */

export interface NiceScale {
  step: number
  max: number
  ticks: number[]
}

/**
 * nice-number 刻度：按最大值自动选 1/2/5×10^n 步长，保证不同量级下刻度可读。
 * integerOnly 时步长不小于 1（调用次数轴不允许出现 "0.5 次"）。
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

/** 左轴（调用次数）刻度文案：大数值缩写为 k / M */
export function formatCount(v: number): string {
  if (v >= 1e6) return `${trimNum(v / 1e6)}M`
  if (v >= 1e3) return `${trimNum(v / 1e3)}k`
  return trimNum(v)
}

export interface ByteUnit {
  div: number
  suffix: string
}

/** 右轴（调用量）单位：按最大值动态选 B/KB/MB/GB，超出 GB 仍按 GB 显示 */
export function byteUnit(maxBytes: number): ByteUnit {
  if (maxBytes < 1024) return { div: 1, suffix: 'B' }
  if (maxBytes < 1024 ** 2) return { div: 1024, suffix: 'KB' }
  if (maxBytes < 1024 ** 3) return { div: 1024 ** 2, suffix: 'MB' }
  return { div: 1024 ** 3, suffix: 'GB' }
}

/** 本地时区 YYYY-MM-DD（与 weekDays 拼桶口径一致，勿用 toISOString 的 UTC 日） */
export function localDateKey(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}
