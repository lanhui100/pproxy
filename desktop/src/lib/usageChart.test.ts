import { describe, expect, it } from 'vitest'
import {
  buildMergedUsageChart,
  byteUnit,
  formatBytes,
  formatCount,
  localDateKey,
  niceScale,
} from './usageChart'

describe('niceScale', () => {
  it('max<=0 / NaN / Infinity 回落到 0..tickCount', () => {
    for (const bad of [0, -5, NaN, Infinity, -Infinity]) {
      const s = niceScale(bad, 4)
      expect(s.ticks).toEqual([0, 1, 2, 3, 4])
      expect(s.max).toBe(4)
    }
  })

  it('max 恰等于 step 整数倍时不多进一格', () => {
    expect(niceScale(8, 4).ticks).toEqual([0, 2, 4, 6, 8])
    expect(niceScale(10, 5).ticks).toEqual([0, 2, 4, 6, 8, 10])
  })

  it('小数值 max 允许小数步长（非 integerOnly）', () => {
    const s = niceScale(2, 4)
    expect(s.step).toBe(0.5)
    expect(s.max).toBeGreaterThanOrEqual(2)
  })

  it('integerOnly 强制整数步长，不出现 0.5 次', () => {
    const s = niceScale(2, 4, true)
    expect(s.step).toBe(1)
    expect(s.ticks.every((t) => Number.isInteger(t))).toBe(true)
    expect(s.max).toBeGreaterThanOrEqual(2)
  })
})

describe('formatCount', () => {
  it('边界：999 / 1000 / 1500 / 1e6 / 2.5e6', () => {
    expect(formatCount(999)).toBe('999')
    expect(formatCount(1000)).toBe('1k')
    expect(formatCount(1500)).toBe('1.5k')
    expect(formatCount(1e6)).toBe('1M')
    expect(formatCount(2.5e6)).toBe('2.5M')
  })
})

describe('byteUnit', () => {
  it('按量级选 B/KB/MB/GB', () => {
    expect(byteUnit(512).suffix).toBe('B')
    expect(byteUnit(2048).suffix).toBe('KB')
    expect(byteUnit(5 * 1024 ** 2).suffix).toBe('MB')
    expect(byteUnit(3 * 1024 ** 3).suffix).toBe('GB')
    expect(byteUnit(1024 ** 4).suffix).toBe('GB')
  })
})

describe('formatBytes', () => {
  it('格式化字节大小', () => {
    expect(formatBytes(500)).toBe('500 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
    expect(formatBytes(1.5 * 1024 * 1024 * 1024)).toBe('1.50 GB')
  })
})

describe('localDateKey', () => {
  it('使用本地时区年月日，补齐前导零', () => {
    const d = new Date(2025, 0, 5, 2, 30) // 本地 2025-01-05 02:30
    expect(localDateKey(d)).toBe('2025-01-05')
  })
})

describe('buildMergedUsageChart', () => {
  it('生成单根经典用量柱模型', () => {
    const days = [
      { date: '2026-08-25', label: '25', cfReq: 10, vReq: 5, cfBytes: 1000, vBytes: 500 },
      { date: '2026-08-26', label: '26', cfReq: 20, vReq: 8, cfBytes: 2000, vBytes: 800 },
      { date: '2026-08-27', label: '27', cfReq: 30, vReq: 12, cfBytes: 4000, vBytes: 1200 },
    ]
    const m = buildMergedUsageChart(days, 320, 72)
    expect(m.bars.length).toBe(3) // 3 days -> 3 single bars
    expect(m.days.length).toBe(3)
    expect(m.days[0].label).toBe('25')
    expect(m.days[2].label).toBe('今天')
    expect(m.days[2].isToday).toBe(true)
    expect(m.maxBytes).toBe(5200) // 4000 + 1200
    expect(m.bars[2].bytes).toBe(5200)
    expect(m.bars[2].title).toContain('总用量: 5.1 KB')
    expect(m.bars[2].title).toContain('调用次数: 42 次')
    expect(m.baselineY).toBe(56) // 6 + (72 - 6 - 16)
  })
})
