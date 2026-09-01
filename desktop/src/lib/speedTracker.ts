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
  val: string
  unit: string
}

/**
 * 将实时速率格式化为自适应精度的数值与独立单位：
 * - < 10 KB/s、< 10 MB/s、< 10 GB/s 保留 1 位小数（如 1.4 MB/s、9.8 KB/s），消除 50%~100% 的整数跳变失真
 * - >= 10 时显示纯整数（如 12 MB/s、105 KB/s），定宽防抖
 */
export function formatSpeedParts(bytesPerSec: number): SpeedParts {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) {
    return { val: '0', unit: 'B/s' }
  }

  // < 1024 B/s: 纯整数，四舍五入达 1024 时进位至 1.0 KB/s
  if (bytesPerSec < 1024) {
    const rounded = Math.round(bytesPerSec)
    if (rounded >= 1024) {
      return { val: '1.0', unit: 'KB/s' }
    }
    return { val: String(rounded), unit: 'B/s' }
  }

  // < 1024 KB/s
  if (bytesPerSec < 1024 * 1024) {
    const kb = bytesPerSec / 1024
    const rounded1Dec = Math.round(kb * 10) / 10
    if (rounded1Dec < 10) {
      return { val: rounded1Dec.toFixed(1), unit: 'KB/s' }
    }
    const roundedInt = Math.round(kb)
    if (roundedInt >= 1024) {
      return { val: '1.0', unit: 'MB/s' }
    }
    return { val: String(roundedInt), unit: 'KB/s' }
  }

  // < 1024 MB/s
  if (bytesPerSec < 1024 * 1024 * 1024) {
    const mb = bytesPerSec / (1024 * 1024)
    const rounded1Dec = Math.round(mb * 10) / 10
    if (rounded1Dec < 10) {
      return { val: rounded1Dec.toFixed(1), unit: 'MB/s' }
    }
    const roundedInt = Math.round(mb)
    if (roundedInt >= 1024) {
      return { val: '1.0', unit: 'GB/s' }
    }
    return { val: String(roundedInt), unit: 'MB/s' }
  }

  // >= 1 GB/s
  const gb = bytesPerSec / (1024 * 1024 * 1024)
  const rounded1Dec = Math.round(gb * 10) / 10
  if (rounded1Dec < 10) {
    return { val: rounded1Dec.toFixed(1), unit: 'GB/s' }
  }
  return { val: String(Math.round(gb)), unit: 'GB/s' }
}

/**
 * 格式化字节速率为易读文本（如 "1.4 MB/s", "12 MB/s", "0 B/s"）
 */
export function formatSpeed(bytesPerSec: number): string {
  const parts = formatSpeedParts(bytesPerSec)
  return `${parts.val} ${parts.unit}`
}

/**
 * 计算两次采样之间的瞬时上下行速率（bytes/s）
 * 包含时钟回退、计数器归零重置及休眠长间隔唤醒保护。
 */
export function calculateSpeed(
  prev: SpeedCalcInput,
  cur: SpeedCalcInput,
): { up: number; down: number } {
  const deltaMs = cur.ts - prev.ts
  // 采样间隔异常保护：
  // 1. < 100ms：过短或时间倒流，避免分母过小引起速率爆表
  // 2. > 5000ms：休眠/挂起唤醒或轮询失步，避免过大分母稀释速率
  if (!Number.isFinite(deltaMs) || deltaMs < 100 || deltaMs > 5000) {
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
 * 计算平滑速率（结合瞬时差分与指数移动加权平滑 EMA）。
 * - prev/cur: 两次累积字节快照
 * - prevSpeed: 上一周期已平滑速率
 * - alpha: 平滑因子（0~1，默认 0.65，既保留突发感知又过滤单秒抖动）
 */
export function calculateSmoothedSpeed(
  prev: SpeedCalcInput,
  cur: SpeedCalcInput,
  prevSpeed?: { up: number; down: number } | null,
  alpha = 0.65,
): { up: number; down: number } {
  const instant = calculateSpeed(prev, cur)
  if (!prevSpeed) {
    return instant
  }

  const smooth = (inst: number, prevSp: number): number => {
    if (inst <= 0) {
      // 断流快速衰减：衰减后低于 1KB/s 直接归零，2~3 秒内彻底清零，杜绝幽灵残影
      if (prevSp <= 1024) return 0
      const decayed = Math.round(prevSp * 0.15)
      return decayed <= 1024 ? 0 : decayed
    }
    // 冷启动直接采用瞬时速率，消除首秒 35% 的 EMA 迟滞失真
    if (prevSp <= 0) {
      return inst
    }
    return Math.round(alpha * inst + (1 - alpha) * prevSp)
  }

  return {
    up: smooth(instant.up, prevSpeed.up),
    down: smooth(instant.down, prevSpeed.down),
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
 * - 聚合下行与上行总流量，真实反映网络吞吐负荷
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
    const totalSpeed = (s.down || 0) + (s.up || 0)
    padded.push(Math.max(0, totalSpeed))
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
