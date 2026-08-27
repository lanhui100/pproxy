import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { useAdaptivePoll } from './useAdaptivePoll'

describe('useAdaptivePoll', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('正常启动并立即执行一次', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { start, stop } = useAdaptivePoll(fn, { baseIntervalMs: 1000, immediate: true })

    start()
    expect(fn).toHaveBeenCalledTimes(1)
    stop()
  })

  it('手动触发 refreshNow 重置退避并执行', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { start, stop, refreshNow } = useAdaptivePoll(fn, { baseIntervalMs: 1000, immediate: false })

    start()
    expect(fn).toHaveBeenCalledTimes(0)
    await refreshNow()
    expect(fn).toHaveBeenCalledTimes(1)
    stop()
  })
})
