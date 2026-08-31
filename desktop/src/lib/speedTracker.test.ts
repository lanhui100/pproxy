import { describe, expect, it } from 'vitest'
import {
  appendSpeedSample,
  calculateSpeed,
  formatSpeed,
  formatSpeedParts,
  generateSpeedWaveform,
  type SpeedSample,
} from './speedTracker'

describe('speedTracker: formatSpeed & formatSpeedParts', () => {
  it('formats bytes per second correctly (no decimals, up to 3 digits)', () => {
    expect(formatSpeed(0)).toBe('0 B/s')
    expect(formatSpeed(450)).toBe('450 B/s')
    expect(formatSpeedParts(450)).toEqual({ val: 450, unit: 'B/s' })
  })

  it('formats kilobytes per second correctly without decimals', () => {
    expect(formatSpeed(1024)).toBe('1 KB/s')
    expect(formatSpeed(1536)).toBe('2 KB/s')
    expect(formatSpeed(100 * 1024)).toBe('100 KB/s')
    expect(formatSpeedParts(100 * 1024)).toEqual({ val: 100, unit: 'KB/s' })
  })

  it('formats megabytes per second correctly without decimals', () => {
    expect(formatSpeed(1024 * 1024)).toBe('1 MB/s')
    expect(formatSpeed(3.45 * 1024 * 1024)).toBe('3 MB/s')
    expect(formatSpeedParts(3.45 * 1024 * 1024)).toEqual({ val: 3, unit: 'MB/s' })
  })

  it('formats gigabytes per second correctly without decimals', () => {
    expect(formatSpeed(1.25 * 1024 * 1024 * 1024)).toBe('1 GB/s')
  })

  it('handles negative, NaN, and infinity safely', () => {
    expect(formatSpeed(-100)).toBe('0 B/s')
    expect(formatSpeed(NaN)).toBe('0 B/s')
    expect(formatSpeed(Infinity)).toBe('0 B/s')
  })
})

describe('speedTracker: calculateSpeed', () => {
  it('calculates speed from byte increments over time delta', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 6000, down: 12000, ts: 12000 } // 2 seconds elapsed
    const result = calculateSpeed(prev, cur)
    expect(result.up).toBe(2500)
    expect(result.down).toBe(5000)
  })

  it('clamps to zero if counters reset or decrease', () => {
    const prev = { up: 10000, down: 20000, ts: 10000 }
    const cur = { up: 2000, down: 5000, ts: 12000 }
    const result = calculateSpeed(prev, cur)
    expect(result.up).toBe(0)
    expect(result.down).toBe(0)
  })

  it('clamps to zero if time delta is non-positive or too small', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 2000, down: 3000, ts: 10050 } // only 50ms
    const result = calculateSpeed(prev, cur)
    expect(result.up).toBe(0)
    expect(result.down).toBe(0)
  })
})

describe('speedTracker: appendSpeedSample', () => {
  it('appends samples and trims to maximum slots', () => {
    let history: SpeedSample[] = []
    for (let i = 0; i < 30; i++) {
      history = appendSpeedSample(history, { down: i * 100, up: i * 50, ts: 1000 + i * 1000 }, 20)
    }
    expect(history.length).toBe(20)
    expect(history[0].down).toBe(1000)
    expect(history[19].down).toBe(2900)
  })
})

describe('speedTracker: generateSpeedWaveform', () => {
  it('generates flat baseline when history is empty', () => {
    const wf = generateSpeedWaveform([], 100, 20, 20)
    expect(wf.maxSpeed).toBe(0)
    expect(wf.points).toContain('0,19')
    expect(wf.area).toContain('Z')
  })

  it('generates proportional points when history contains data', () => {
    const samples: SpeedSample[] = [
      { down: 1000, up: 100, ts: 1 },
      { down: 5000, up: 500, ts: 2 },
      { down: 2500, up: 250, ts: 3 },
    ]
    const wf = generateSpeedWaveform(samples, 100, 20, 10)
    expect(wf.maxSpeed).toBe(5000)
    expect(wf.points.split(' ').length).toBe(10)
    expect(wf.area).toContain('L 100,19')
  })
})
