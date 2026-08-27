import { describe, expect, it } from 'vitest'
import {
  appendLatencyPoint,
  formatSlotTooltip,
  getLatencyTone,
  padLatencySlots,
  type LatencyPoint,
} from './latencyHistory'

describe('getLatencyTone', () => {
  it('正常延迟 <= 800ms 返回 ok (绿)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 120 })).toBe('ok')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 800 })).toBe('ok')
  })

  it('稍慢延迟 800ms ~ 2000ms 返回 warn (黄)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 801 })).toBe('warn')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 1999 })).toBe('warn')
  })

  it('超时 > 2000ms 或探测失败返回 error (红)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 2001 })).toBe('error')
    expect(getLatencyTone({ ts: 1000, ok: false, err: 'timeout' })).toBe('error')
  })

  it('未提供数据返回 empty (灰)', () => {
    expect(getLatencyTone(undefined)).toBe('empty')
  })
})

describe('appendLatencyPoint', () => {
  it('保持最多 12 个历史点', () => {
    let hist: LatencyPoint[] = []
    for (let i = 0; i < 15; i++) {
      hist = appendLatencyPoint(hist, { ts: i * 300_000, ok: true, ms: 100 + i })
    }
    expect(hist.length).toBe(12)
    expect(hist[0]?.ms).toBe(103)
    expect(hist[11]?.ms).toBe(114)
  })
})

describe('padLatencySlots', () => {
  it('不足 12 个点向前补齐 undefined', () => {
    const hist: LatencyPoint[] = [
      { ts: 1000, ok: true, ms: 100 },
      { ts: 2000, ok: true, ms: 150 },
    ]
    const padded = padLatencySlots(hist, 12)
    expect(padded.length).toBe(12)
    expect(padded[0]).toBeUndefined()
    expect(padded[9]).toBeUndefined()
    expect(padded[10]?.ms).toBe(100)
    expect(padded[11]?.ms).toBe(150)
  })
})

describe('formatSlotTooltip', () => {
  it('格式化正常延迟与异常信息', () => {
    const p1: LatencyPoint = { ts: new Date('2026-08-27T20:30:00Z').getTime(), ok: true, ms: 210 }
    expect(formatSlotTooltip(p1)).toContain('210ms')

    const p2: LatencyPoint = { ts: new Date('2026-08-27T20:30:00Z').getTime(), ok: false, err: '502 Bad Gateway' }
    expect(formatSlotTooltip(p2)).toContain('502 Bad Gateway')
  })
})
