import { describe, expect, it, vi } from 'vitest'
import {
  appendLatencyPoint,
  formatSlotTooltip,
  getLatencyTone,
  loadLatencySeries,
  padLatencySlots,
  saveLatencySeries,
  WINDOW_MS,
  type LatencyPoint,
} from './latencyHistory'

// vitest 运行于 node 环境：为持久化用例提供最小内存 localStorage
const memStore = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStore.get(k) ?? null,
  setItem: (k: string, v: string) => void memStore.set(k, v),
  removeItem: (k: string) => void memStore.delete(k),
})

describe('getLatencyTone', () => {
  it('正常延迟 <= 2000ms 返回 ok (绿)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 120 })).toBe('ok')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 800 })).toBe('ok')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 2000 })).toBe('ok')
  })

  it('稍慢延迟 2000ms ~ 5000ms 返回 warn (黄)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 2001 })).toBe('warn')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 4999 })).toBe('warn')
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 5000 })).toBe('warn')
  })

  it('超时 > 5000ms 或探测失败返回 error (红)', () => {
    expect(getLatencyTone({ ts: 1000, ok: true, ms: 5001 })).toBe('error')
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

  it('跨较长时间间隔依然连续保留历史采样点（不因绝对时间清空）', () => {
    let hist: LatencyPoint[] = [{ ts: 0, ok: true, ms: 100 }]
    // 即使间隔超过 2 小时，依然连续推入
    hist = appendLatencyPoint(hist, { ts: WINDOW_MS + 60_000, ok: true, ms: 200 })
    expect(hist.length).toBe(2)
    expect(hist[0]?.ms).toBe(100)
    expect(hist[1]?.ms).toBe(200)
  })

  it('间隔小于 150 秒的点合并更新而不是新增', () => {
    let hist: LatencyPoint[] = [{ ts: 1_000_000, ok: true, ms: 100 }]
    hist = appendLatencyPoint(hist, { ts: 1_000_000 + 60_000, ok: false })
    expect(hist.length).toBe(1)
    expect(hist[0]?.ok).toBe(false)
  })
})

describe('时序持久化', () => {
  it('保存后可按 key 读回', () => {
    const now = Date.now()
    const hist: LatencyPoint[] = [
      { ts: now - 600_000, ok: true, ms: 123 },
      { ts: now, ok: false, err: 'timeout' },
    ]
    saveLatencySeries('test-key', hist)
    const loaded = loadLatencySeries('test-key')
    expect(loaded.length).toBe(2)
    expect(loaded[0]?.ms).toBe(123)
    expect(loaded[1]?.ok).toBe(false)
  })

  it('读取时保留跨会话历史点（不因超过 2 小时而被强行清空）', () => {
    const now = Date.now()
    saveLatencySeries('test-persisted', [
      { ts: now - WINDOW_MS - 3_600_000, ok: true, ms: 100 },
      { ts: now, ok: true, ms: 200 },
    ])
    const loaded = loadLatencySeries('test-persisted')
    expect(loaded.length).toBe(2)
    expect(loaded[0]?.ms).toBe(100)
    expect(loaded[1]?.ms).toBe(200)
  })

  it('未知 key 返回空数组', () => {
    expect(loadLatencySeries('no-such-key')).toEqual([])
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
