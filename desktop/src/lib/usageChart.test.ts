import { describe, expect, it } from 'vitest'
import { byteUnit, formatCount, localDateKey, niceScale } from './usageChart'

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

  it('大值量级：tick 数受控且 top >= max', () => {
    const s = niceScale(1.4e8, 4)
    expect(s.ticks.length).toBeLessThanOrEqual(6)
    expect(s.max).toBeGreaterThanOrEqual(1.4e8)
    for (let i = 1; i < s.ticks.length; i++) {
      expect(s.ticks[i]).toBeGreaterThan(s.ticks[i - 1])
    }
  })

  it('max=1 时 ticks 单调不重叠', () => {
    const s = niceScale(1, 4, true)
    expect(s.ticks).toEqual([0, 1])
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
    expect(byteUnit(1024 ** 4).suffix).toBe('GB') // 超出 GB 仍封顶 GB
  })
})

describe('localDateKey', () => {
  it('使用本地时区年月日，补齐前导零', () => {
    const d = new Date(2025, 0, 5, 2, 30) // 本地 2025-01-05 02:30
    expect(localDateKey(d)).toBe('2025-01-05')
  })
})
