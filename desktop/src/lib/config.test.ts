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

  it('clearTunnelToken 后 hasToken 归 false（成功返回 true）', async () => {
    const mod = await loadConfig()
    await mod.saveTunnelConfig('wss://gate.example/ws', 'tok-1')
    await expect(mod.clearTunnelToken()).resolves.toBe(true)
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
    const r = mod.parseGateInput(makeCode('wss://evil.otherdomain.com/ws', 'tok'))
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
  it('映射正常凭据与指纹（含 data_dir 缺字段兜底）', async () => {
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
      fpFallback: null,
      fpKeyring: null,
      credWinner: null,
      credMeta: null,
      dataDir: null,
      dataDirTmpFallback: false,
      effectiveUrl: null,
      userClaims: null,
    })
  })

  it('映射 effective_url（引擎实际端点串）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelConfig({
      url: '',
      has_token: true,
      effective_url: 'wss://vgate.example.com/api/ws,wss://gate.example.com/ws',
    })
    expect(c.effectiveUrl).toBe('wss://vgate.example.com/api/ws,wss://gate.example.com/ws')
  })

  it('映射 data_dir 与 temp 回退标记', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelConfig({
      url: 'wss://gate.example.com/ws',
      has_token: true,
      data_dir: 'C:\\Users\\x\\AppData\\Roaming\\pony',
      data_dir_tmp_fallback: true,
    })
    expect(c.dataDir).toBe('C:\\Users\\x\\AppData\\Roaming\\pony')
    expect(c.dataDirTmpFallback).toBe(true)
  })

  it('映射 P0 分源指纹与写入审计（H2 实锤三值）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelConfig({
      url: 'wss://gate.example.com/ws',
      has_token: true,
      cred_error: null,
      fingerprint: 'aaaa1111',
      fp_fallback: 'bbbb2222',
      fp_keyring: 'aaaa1111',
      cred_winner: 'keyring(diverged)',
      cred_meta: { last_write_ts: 123, source: 'tunnel_token_save', fp8: 'aaaa1111' },
      user_claims: null,
    })
    expect(c.fpFallback).toBe('bbbb2222')
    expect(c.fpKeyring).toBe('aaaa1111')
    expect(c.credWinner).toBe('keyring(diverged)')
    expect(c.credMeta).toEqual({ last_write_ts: 123, source: 'tunnel_token_save', fp8: 'aaaa1111' })
    expect(c.userClaims).toBeNull()
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
      fpFallback: null,
      fpKeyring: null,
      credWinner: null,
      credMeta: null,
      dataDir: null,
      dataDirTmpFallback: false,
      effectiveUrl: null,
      userClaims: null,
    })
  })
})

// ---- mapTunnelSelfCheck：snake → camel（Must-fix A 回归：无映射时 H2 告警恒不可见）----
describe('mapTunnelSelfCheck', () => {
  it('全 snake 输入映射为 camel（含分叉三值与审计）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelSelfCheck({
      fingerprint: 'aaaa1111',
      cred_ok: true,
      cred_error: null,
      gates: [{ name: 'cf', url: 'wss://gate.example.com/ws', ok: true, ms: 1, kind: 'ok' }],
      fp_fallback: 'bbbb2222',
      fp_keyring: 'aaaa1111',
      cred_winner: 'keyring(diverged)',
      cred_meta: { last_write_ts: 7, source: 'tunnel_token_save', fp8: 'aaaa1111' },
    })
    expect(c.credOk).toBe(true)
    expect(c.credError).toBeNull()
    expect(c.fpFallback).toBe('bbbb2222')
    expect(c.fpKeyring).toBe('aaaa1111')
    expect(c.credWinner).toBe('keyring(diverged)')
    expect(c.credMeta).toEqual({ last_write_ts: 7, source: 'tunnel_token_save', fp8: 'aaaa1111' })
  })

  it('缺字段兜底且 gates 非数组时为空数组', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelSelfCheck({})
    expect(c.fingerprint).toBeNull()
    expect(c.credOk).toBe(false)
    expect(c.gates).toEqual([])
    expect(c.credMeta).toBeNull()
  })

  it('映射 kind_upgrade/kind_bind 双针（保留旧 kind）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelSelfCheck({
      fingerprint: 'f',
      cred_ok: false,
      cred_error: null,
      gates: [
        { name: 'cf', url: 'wss://gate.example.com/ws', ok: false, kind: 'auth401', kind_upgrade: 'websocket', kind_bind: 'bad_frame' },
      ],
    })
    expect(c.gates[0]?.kind).toBe('auth401')
    expect(c.gates[0]?.kindUpgrade).toBe('websocket')
    expect(c.gates[0]?.kindBind).toBe('bad_frame')
  })

  it('缺 kind_upgrade/kind_bind 时为 undefined（兜底）', async () => {
    const mod = await loadConfig()
    const c = mod.mapTunnelSelfCheck({
      fingerprint: 'f',
      cred_ok: true,
      cred_error: null,
      gates: [{ name: 'cf', url: 'wss://gate.example.com/ws', ok: true, ms: 1, kind: 'ok' }],
    })
    expect(c.gates[0]?.kindUpgrade).toBeUndefined()
    expect(c.gates[0]?.kindBind).toBeUndefined()
  })
})

// ---- GATE_KIND_TEXT：gate 错误分类中文文案 ----
describe('GATE_KIND_TEXT', () => {
  it('七类 kind 均有中文映射', async () => {
    const mod = await loadConfig()
    expect(mod.GATE_KIND_TEXT).toEqual({
      auth401: '令牌无效，请重贴授权码',
      denied: '被远端门禁拒绝，非令牌错误',
      timeout: '网络超时',
      closed: '连接被关闭',
      no_token: '未配置令牌',
      ok: '正常',
      other: '未知错误',
    })
  })
})

// ---- provisionReadiness：ready / need_token / cred_error / diverged ----
describe('provisionReadiness', () => {
  it('端点+令牌齐备 → ready', async () => {
    const mod = await loadConfig()
    expect(mod.provisionReadiness({ url: 'wss://gate.example.com/ws', hasToken: true })).toBe('ready')
  })

  it('缺端点或令牌 → need_token', async () => {
    const mod = await loadConfig()
    expect(mod.provisionReadiness({ url: '', hasToken: true })).toBe('need_token')
    expect(mod.provisionReadiness({ url: 'wss://gate.example.com/ws', hasToken: false })).toBe('need_token')
  })

  it('有 credError → cred_error', async () => {
    const mod = await loadConfig()
    expect(
      mod.provisionReadiness({ url: 'wss://gate.example.com/ws', hasToken: true, credError: '凭据损坏' }),
    ).toBe('cred_error')
  })

  it('fpFallback 与 fpKeyring 不一致 → diverged（优先于 credError）', async () => {
    const mod = await loadConfig()
    expect(
      mod.provisionReadiness({
        url: 'wss://gate.example.com/ws',
        hasToken: true,
        credError: 'x',
        fpFallback: 'bbbb2222',
        fpKeyring: 'aaaa1111',
      }),
    ).toBe('diverged')
  })

  it('fp 一致时不判分叉', async () => {
    const mod = await loadConfig()
    expect(
      mod.provisionReadiness({
        url: 'wss://gate.example.com/ws',
        hasToken: true,
        fpFallback: 'aaaa1111',
        fpKeyring: 'aaaa1111',
      }),
    ).toBe('ready')
  })
})

// ---- clearTunnelToken（Tauri 抛错语义：失败抛错带后端原文，不再返回 false）----
describe('clearTunnelToken（Tauri 抛错语义）', () => {
  it('invoke 失败时抛错并携带后端原文', async () => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    vi.doMock('@tauri-apps/api/core', () => ({
      invoke: () => Promise.reject(new Error('keyring locked')),
    }))
    try {
      const mod = await loadConfig()
      await expect(mod.clearTunnelToken()).rejects.toThrow('keyring locked')
    } finally {
      vi.doUnmock('@tauri-apps/api/core')
    }
  })

  it('invoke 以字符串抛错时同样透出原文', async () => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    vi.doMock('@tauri-apps/api/core', () => ({
      // eslint-disable-next-line @typescript-eslint/no-base-to-string
      invoke: () => Promise.reject('凭据库不可写'),
    }))
    try {
      const mod = await loadConfig()
      await expect(mod.clearTunnelToken()).rejects.toThrow('凭据库不可写')
    } finally {
      vi.doUnmock('@tauri-apps/api/core')
    }
  })
})

// ---- tunnelConfigSet dev 分支：localStorage 兼容 + null 沿用语义 ----
describe('tunnelConfigSet（dev 分支）', () => {
  it('写入端点与令牌并回显 url', async () => {
    const mod = await loadConfig()
    const res = await mod.tunnelConfigSet('wss://gate.example.com/ws', 'tok-1')
    expect(res.url).toBe('wss://gate.example.com/ws')
    const cfg = await mod.loadTunnelConfig()
    expect(cfg.url).toBe('wss://gate.example.com/ws')
    expect(cfg.hasToken).toBe(true)
  })

  it('null 表示沿用，不覆盖已存值', async () => {
    const mod = await loadConfig({ 'pony-tunnel-url': 'wss://old.example/ws', 'pony-dev-tunnel-token': 'old-tok' })
    const res = await mod.tunnelConfigSet(null, null)
    expect(res.url).toBe('wss://old.example/ws')
    const cfg = await mod.loadTunnelConfig()
    expect(cfg.url).toBe('wss://old.example/ws')
    expect(cfg.hasToken).toBe(true)
  })
})

// ---- importConnectCode dev 分支：token 改为 dev-mock-token ----
describe('importConnectCode（dev 分支）', () => {
  it('写入可识别的 dev-mock-token', async () => {
    const mod = await loadConfig()
    const json = JSON.stringify({ v: 1, u: 'wss://gate.example.com/ws', t: 'tok' })
    const code = `pony-gate://${Buffer.from(json, 'utf8').toString('base64url')}`
    const res = await mod.importConnectCode(code)
    expect(res.url).toBe('wss://gate.example.com/ws')
    const cfg = await mod.loadTunnelConfig()
    expect(cfg.hasToken).toBe(true)
  })
})

// ---- tunnelSelfCheck dev 分支：恒绿结果带 mock 标记 ----
describe('tunnelSelfCheck（dev 分支）', () => {
  it('返回 mock: true', async () => {
    const mod = await loadConfig()
    const c = await mod.tunnelSelfCheck()
    expect(c.mock).toBe(true)
    expect(c.credOk).toBe(true)
    expect(c.gates.length).toBe(2)
  })
})
