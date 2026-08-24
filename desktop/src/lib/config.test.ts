// config 纯逻辑单测（R2-ENG-7）：normalizePollMin 全分支 / readPollMin 三态 /
// saveBackendUrl 驱动响应式源（壳层门槛热解锁）/ savePollIntervalMin 同步 ref。
import { afterEach, describe, expect, it, vi } from 'vitest'

type Store = Map<string, string>

function stubStorage(init: Record<string, string>): Store {
  const store = new Map(Object.entries(init))
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  })
  return store
}

async function loadConfig(init: Record<string, string> = {}) {
  vi.resetModules()
  const store = stubStorage(init)
  const mod = await import('./config')
  return { mod, store }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.resetModules()
})

describe('normalizePollMin', () => {
  it.each([
    [0, 0], // 0 = 手动档，合法往返
    [-5, 5], // 负数回落默认（R2-ENG-8：对齐冻结接口卡）
    [NaN, 5],
    [Infinity, 5],
    [0.5, 1],
    [7.9, 7],
    [15, 15],
    [5, 5],
  ])('%p → %p', (input, expected) => {
    return loadConfig().then(({ mod }) => {
      expect(mod.normalizePollMin(input)).toBe(expected)
    })
  })
})

describe('readPollMin 三态（模块加载初始化）', () => {
  it('key 缺失 → 默认 5（R2-ENG-2）', async () => {
    const { mod } = await loadConfig()
    expect(mod.pollIntervalMin.value).toBe(5)
  })

  it('显式存储 "0" → 手动档 0', async () => {
    const { mod } = await loadConfig({ 'pony-poll-interval-min': '0' })
    expect(mod.pollIntervalMin.value).toBe(0)
  })

  it('存量 "15" → 15', async () => {
    const { mod } = await loadConfig({ 'pony-poll-interval-min': '15' })
    expect(mod.pollIntervalMin.value).toBe(15)
  })
})

describe('save 写路径同步响应式源', () => {
  it('savePollIntervalMin 同步 ref 并持久化', async () => {
    const { mod, store } = await loadConfig()
    mod.savePollIntervalMin(15)
    expect(mod.pollIntervalMin.value).toBe(15)
    expect(store.get('pony-poll-interval-min')).toBe('15')
  })

  it('saveBackendUrl 同步 backendUrlSaved（门槛热解锁依据）', async () => {
    const { mod, store } = await loadConfig()
    expect(mod.backendUrlSaved.value).toBe('')
    mod.saveBackendUrl('http://gw:8900/')
    expect(mod.backendUrlSaved.value).toBe('http://gw:8900')
    expect(store.get('pony-backend-url')).toBe('http://gw:8900')
  })

  it('保存后 useBackendGate 即时翻转（R2-UX-1 回归钉）', async () => {
    const { mod } = await loadConfig()
    const gate = await import('../composables/useBackendGate')
    const { configured } = gate.useBackendGate()
    expect(configured.value).toBe(false)
    mod.saveBackendUrl('http://gw:8900')
    expect(configured.value).toBe(true)
  })
})
