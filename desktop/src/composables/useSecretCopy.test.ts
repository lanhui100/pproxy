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

  it('forcePurgeClipboard 立即取消待清任务并尽力清一次', async () => {
    const { forcePurgeClipboard } = await import('./useSecretCopy')
    await copySecret('leak-risk')
    forcePurgeClipboard()
    await vi.advanceTimersByTimeAsync(120_000)
    const emptyWrites = writes.filter((w) => w === '')
    expect(emptyWrites).toHaveLength(1)
  })

  it('剪贴板已被用户覆盖时不误伤', async () => {
    await copySecret('mine')
    readValue = 'user-copied-something' // 用户在 60s 内复制了别的内容
    await vi.advanceTimersByTimeAsync(60_000)
    expect(writes).toEqual(['mine']) // 读回不一致 → 不写空
  })

  it('在途写入期间 releaseAll：写入完成后不复活定时器并立即擦除', async () => {
    let resolveWrite!: () => void
    _setClipboardDepsForTest({
      write: async (t) => {
        writes.push(t)
        readValue = t
        await new Promise<void>((res) => {
          resolveWrite = res
        })
      },
      read: async () => readValue,
    })
    const pending = copySecret('in-flight') // 写入已发起但仍挂起
    releaseAll() // 用户此刻关闭对话框
    expect(writes).toContain('in-flight')
    resolveWrite() // 挂起的写入完成
    await pending.catch(() => {})
    await vi.advanceTimersByTimeAsync(120_000)
    // 唯一一次空写发生在写入完成后的即时擦除；无定时器复活产生第二次
    expect(writes.filter((w) => w === '')).toHaveLength(1)
  })

  it('第二次复制失败：旧值立即获得清理，不再无限期滞留', async () => {
    let failNext = false
    _setClipboardDepsForTest({
      write: async (t) => {
        if (failNext && t === 'B') throw new Error('denied')
        writes.push(t)
        readValue = t
      },
      read: async () => readValue,
    })
    await copySecret('A')
    failNext = true
    await expect(copySecret('B')).rejects.toThrow('denied')
    await vi.advanceTimersByTimeAsync(0)
    expect(writes.filter((w) => w === '')).toHaveLength(1) // A 被即时清理
  })
})
