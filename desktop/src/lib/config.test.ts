// config 纯逻辑单测：隧道端点校验边界 / dev 便利通道（localStorage）存取往返。
// 远端管理面相关配置（网关地址/令牌、轮询间隔）已随单体化退役删除，不再有对应用例。
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
  stubStorage(init)
  return import('./config')
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.resetModules()
})

describe('isValidTunnelUrl', () => {
  it.each([
    ['wss://gate.ponyjob.top/ws', true],
    ['ws://127.0.0.1:9000/ws', true],
    ['wss://a.example.com/ws,wss://b.example.com/api/ws', true], // 多端点（逗号分隔）合法
    ['https://gate.ponyjob.top/ws', false], // 仅接受 ws/wss
    ['wss://has space/ws', false], // 禁空白
    ['', false],
    [`wss://${'a'.repeat(200)}`, false], // 超长拒绝（>200）
  ])('%s → %p', (input, expected) => {
    return loadConfig().then((mod) => {
      expect(mod.isValidTunnelUrl(input)).toBe(expected)
    })
  })
})

describe('dev 便利通道（浏览器 localStorage）隧道配置往返', () => {
  it('saveTunnelConfig 持久化端点与令牌标记，loadTunnelConfig 读回', async () => {
    const mod = await loadConfig()
    await mod.saveTunnelConfig('  wss://gate.example/ws  ', 'tok-1')
    const cfg = await mod.loadTunnelConfig()
    expect(cfg.url).toBe('wss://gate.example/ws')
    expect(cfg.hasToken).toBe(true)
  })

  it('clearTunnelToken 后 hasToken 归 false', async () => {
    const mod = await loadConfig()
    await mod.saveTunnelConfig('wss://gate.example/ws', 'tok-1')
    expect(await mod.clearTunnelToken()).toBe(true)
    const cfg = await mod.loadTunnelConfig()
    expect(cfg.hasToken).toBe(false)
    expect(cfg.url).toBe('wss://gate.example/ws') // 端点保留
  })

  it('非法端点保存直接抛错，不落盘', async () => {
    const mod = await loadConfig()
    await expect(mod.saveTunnelConfig('https://gate.example/ws', 't')).rejects.toThrow()
  })
})
