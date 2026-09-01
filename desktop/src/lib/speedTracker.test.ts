import { describe, expect, it } from 'vitest'
import {
  appendSpeedSample,
  calculateSmoothedSpeed,
  calculateSpeed,
  formatSpeed,
  formatSpeedParts,
  generateSpeedWaveform,
  type SpeedSample,
} from './speedTracker'

describe('speedTracker: formatSpeed & formatSpeedParts', () => {
  it('formats bytes per second correctly without decimals', () => {
    expect(formatSpeed(0)).toBe('0 B/s')
    expect(formatSpeed(450)).toBe('450 B/s')
    expect(formatSpeedParts(450)).toEqual({ val: '450', unit: 'B/s' })
    expect(formatSpeed(1000)).toBe('1000 B/s')
    expect(formatSpeedParts(1000)).toEqual({ val: '1000', unit: 'B/s' })
  })

  it('rolls over to 1.0 KB/s when bytes round up to 1024', () => {
    expect(formatSpeed(1023.6)).toBe('1.0 KB/s')
    expect(formatSpeedParts(1023.6)).toEqual({ val: '1.0', unit: 'KB/s' })
  })

  it('formats kilobytes per second correctly with 1 decimal when < 10, integer when >= 10', () => {
    expect(formatSpeed(1024)).toBe('1.0 KB/s')
    expect(formatSpeed(1536)).toBe('1.5 KB/s')
    expect(formatSpeedParts(1536)).toEqual({ val: '1.5', unit: 'KB/s' })
    expect(formatSpeed(9.8 * 1024)).toBe('9.8 KB/s')
    expect(formatSpeed(9.96 * 1024)).toBe('10 KB/s')
    expect(formatSpeedParts(9.96 * 1024)).toEqual({ val: '10', unit: 'KB/s' })
    expect(formatSpeed(10 * 1024)).toBe('10 KB/s')
    expect(formatSpeed(100 * 1024)).toBe('100 KB/s')
    expect(formatSpeedParts(100 * 1024)).toEqual({ val: '100', unit: 'KB/s' })
  })

  it('rolls over to 1.0 MB/s when kilobytes round up to 1024', () => {
    expect(formatSpeed(1023.6 * 1024)).toBe('1.0 MB/s')
    expect(formatSpeedParts(1023.6 * 1024)).toEqual({ val: '1.0', unit: 'MB/s' })
  })

  it('formats megabytes per second correctly with 1 decimal when < 10, integer when >= 10', () => {
    expect(formatSpeed(1024 * 1024)).toBe('1.0 MB/s')
    expect(formatSpeed(1.4 * 1024 * 1024)).toBe('1.4 MB/s')
    expect(formatSpeed(3.45 * 1024 * 1024)).toBe('3.5 MB/s')
    expect(formatSpeedParts(3.45 * 1024 * 1024)).toEqual({ val: '3.5', unit: 'MB/s' })
    expect(formatSpeed(9.96 * 1024 * 1024)).toBe('10 MB/s')
    expect(formatSpeedParts(9.96 * 1024 * 1024)).toEqual({ val: '10', unit: 'MB/s' })
    expect(formatSpeed(12.4 * 1024 * 1024)).toBe('12 MB/s')
    expect(formatSpeedParts(12.4 * 1024 * 1024)).toEqual({ val: '12', unit: 'MB/s' })
  })

  it('formats gigabytes per second correctly', () => {
    expect(formatSpeed(1.25 * 1024 * 1024 * 1024)).toBe('1.3 GB/s')
    expect(formatSpeed(12 * 1024 * 1024 * 1024)).toBe('12 GB/s')
  })

  it('handles negative, NaN, and infinity safely', () => {
    expect(formatSpeed(-100)).toBe('0 B/s')
    expect(formatSpeed(NaN)).toBe('0 B/s')
    expect(formatSpeed(Infinity)).toBe('0 B/s')
    expect(formatSpeedParts(-5)).toEqual({ val: '0', unit: 'B/s' })
  })
})

describe('speedTracker: calculateSpeed & calculateSmoothedSpeed', () => {
  it('calculates instantaneous speed from byte increments over time delta', () => {
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

  it('clamps to zero if time delta is excessively large (sleep/suspension gap protection)', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 600000, down: 1200000, ts: 70000 } // 60s elapsed
    const result = calculateSpeed(prev, cur)
    expect(result.up).toBe(0)
    expect(result.down).toBe(0)
  })

  it('applies EMA smoothing when previous speed exists', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 3000, down: 6000, ts: 11000 } // instant: up=2000, down=4000
    const prevSpeed = { up: 1000, down: 2000 }
    // alpha = 0.65 -> down = 0.65*4000 + 0.35*2000 = 2600 + 700 = 3300
    const smoothed = calculateSmoothedSpeed(prev, cur, prevSpeed, 0.65)
    expect(smoothed.down).toBe(3300)
    expect(smoothed.up).toBe(1650)
  })

  it('uses instantaneous speed on cold start (no EMA lag when starting from zero)', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 3000, down: 12000, ts: 11000 } // instant: up=2000, down=10000
    const prevSpeed = { up: 0, down: 0 }
    const smoothed = calculateSmoothedSpeed(prev, cur, prevSpeed, 0.65)
    expect(smoothed.down).toBe(10000)
    expect(smoothed.up).toBe(2000)
  })

  it('decays rapidly and zeros out in 2~3 cycles on sudden traffic cessation', () => {
    const prev = { up: 1000, down: 2000, ts: 10000 }
    const cur = { up: 1000, down: 2000, ts: 11000 } // instant: 0

    // Cycle 1: starting from 100MB/s
    let speed = calculateSmoothedSpeed(prev, cur, { up: 10 * 1024 * 1024, down: 100 * 1024 * 1024 })
    expect(speed.down).toBe(15 * 1024 * 1024) // 100MB * 0.15 = 15MB

    // Cycle 2
    speed = calculateSmoothedSpeed(prev, cur, speed)
    expect(speed.down).toBe(Math.round(15 * 1024 * 1024 * 0.15)) // ~2.25MB

    // Cycle 3
    speed = calculateSmoothedSpeed(prev, cur, speed)
    expect(speed.down).toBe(Math.round(2359296 * 0.15)) // ~353KB

    // Cycle 4: <= 1024 -> 0
    speed = calculateSmoothedSpeed(prev, cur, { up: 500, down: 800 })
    expect(speed.down).toBe(0)
    expect(speed.up).toBe(0)
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

  it('aggregates down and up traffic for total load representation', () => {
    const samples: SpeedSample[] = [
      { down: 1000, up: 500, ts: 1 }, // total 1500
      { down: 5000, up: 2000, ts: 2 }, // total 7000
      { down: 2500, up: 500, ts: 3 }, // total 3000
    ]
    const wf = generateSpeedWaveform(samples, 100, 20, 10)
    expect(wf.maxSpeed).toBe(7000) // down(5000) + up(2000)
    expect(wf.points.split(' ').length).toBe(10)
    expect(wf.area).toContain('L 100,19')
  })
})
