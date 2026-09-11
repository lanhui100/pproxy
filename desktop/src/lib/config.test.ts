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
    ['wss://gate.example.com/ws', true],
    ['ws://127.0.0.1:9000/ws', true],
    ['wss://a.example.com/ws,wss://b.example.com/api/ws', true], // 多端点（逗号分隔）合法
    ['https://gate.example.com/ws', false], // 仅接受 ws/wss
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

// ---- pony-gate:// 连接口令解析（与 deploy/gen-connect-code.mjs 输出对齐）----
function makeCode(url: string, token: string): string {
  const json = JSON.stringify({ v: 1, u: url, t: token })
  return `pony-gate://${Buffer.from(json, 'utf8').toString('base64url')}`
}

describe('parseGateInput', () => {
  it('识别裸授权码', async () => {
    const mod = await loadConfig()
    expect(mod.parseGateInput('some-raw-token-123')).toEqual({ kind: 'token' })
  })

  it('解析合法连接口令并判定官方域名', async () => {
    const mod = await loadConfig()
    const r = mod.parseGateInput(makeCode('wss://gate.example.com/ws,wss://vgate.example.com/api/ws', 'tok'))
    expect(r?.kind).toBe('code')
    expect(r?.url).toBe('wss://gate.example.com/ws,wss://vgate.example.com/api/ws')
    expect(r?.official).toBe(true)
  })

  it('非官方域名端点标记为非官方（前端警示）', async () => {
    const mod = await loadConfig()
    const r = mod.parseGateInput(makeCode('wss://evil.example.com/ws', 'tok'))
    expect(r?.kind).toBe('code')
    expect(r?.official).toBe(false)
  })

  it('仿冒域名（含 example.com 子串）不得误判为官方', async () => {
    const mod = await loadConfig()
    for (const u of ['wss://evil-example.com.attacker.com/ws', 'wss://example.com.evil.com/ws']) {
      const r = mod.parseGateInput(makeCode(u, 'tok'))
      expect(r?.kind).toBe('code')
      expect(r?.official).toBe(false)
    }
  })

  it('损坏/缺字段的连接口令返回 null', async () => {
    const mod = await loadConfig()
    expect(mod.parseGateInput('pony-gate://!!!bad')).toBeNull()
    expect(mod.parseGateInput(`pony-gate://${Buffer.from('{"u":"wss://x/ws"}').toString('base64url')}`)).toBeNull()
  })

  it('空输入返回 null', async () => {
    const mod = await loadConfig()
    expect(mod.parseGateInput('')).toBeNull()
    expect(mod.parseGateInput('   ')).toBeNull()
  })
})

// ---- mapTunnelConfig：Rust snake_case → 前端 camelCase（review B P1-1 回归）----
describe('mapTunnelConfig', () => {
  it('映射正常凭据与指纹', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelConfig({
      url: 'wss://gate.example.com/ws',
      has_token: true,
      cred_error: null,
      fingerprint: 'deadbeef',
    })
    expect(c).toEqual({
      url: 'wss://gate.example.com/ws',
      hasToken: true,
      credError: null,
      fingerprint: 'deadbeef',
    })
  })

  it('凭据损坏时透传 cred_error（曾因字段名漂移静默失效）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelConfig({
      url: '',
      has_token: false,
      cred_error: '凭据损坏或编码不兼容',
      fingerprint: null,
    })
    expect(c.hasToken).toBe(false)
    expect(c.credError).toContain('凭据损坏')
    expect(c.fingerprint).toBeNull()
  })

  it('缺失字段按空值兜底（不抛错）', async () => {
    const mod = await loadConfig()
    expect(mod.mapTunnelConfig({})).toEqual({
      url: '',
      hasToken: false,
      credError: null,
      fingerprint: null,
    })
  })
})
