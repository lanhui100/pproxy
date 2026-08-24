import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { _setClipboardDepsForTest, copySecret, releaseAll } from './useSecretCopy'

describe('useSecretCopy', () => {
  let writes: string[]
  let readValue: string

  beforeEach(() => {
    vi.useFakeTimers()
    writes = []
    readValue = ''
    _setClipboardDepsForTest({
      write: async (t) => {
        writes.push(t)
        readValue = t
      },
      read: async () => readValue,
    })
  })

  afterEach(() => {
    releaseAll()
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('复制后 60s 自动清空剪贴板', async () => {
    await copySecret('pony_secret')
    expect(writes).toEqual(['pony_secret'])
    await vi.advanceTimersByTimeAsync(60_000)
    expect(writes.at(-1)).toBe('')
  })

  it('新复制取消旧定时器：只有最后一次的值会被清理', async () => {
    await copySecret('first')
    await vi.advanceTimersByTimeAsync(30_000)
    await copySecret('second') // 重设窗口
    await vi.advanceTimersByTimeAsync(31_000) // 距 first 已超 60s，但其定时器已被取消
    expect(writes.filter((w) => w === '')).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(29_000) // second 的 60s 到期
    expect(writes.at(-1)).toBe('')
  })

  it('releaseAll 立即取消待清任务并尽力清一次', async () => {
    await copySecret('leak-risk')
    releaseAll()
    await vi.advanceTimersByTimeAsync(120_000)
    // 唯一一次清空写发生在 releaseAll 同步路径，60s 定时器未再触发第二次
    const emptyWrites = writes.filter((w) => w === '')
    expect(emptyWrites).toHaveLength(1)
  })

  it('剪贴板已被用户覆盖时不误伤', async () => {
    await copySecret('mine')
    readValue = 'user-copied-something' // 用户在 60s 内复制了别的内容
    await vi.advanceTimersByTimeAsync(60_000)
    expect(writes).toEqual(['mine']) // 读回不一致 → 不写空
  })
})
