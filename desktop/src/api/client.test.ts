// API 契约测试（M5 spec §7.1.1/§7.1.2）：MSW 按 API.md 形状 mock，钉住
// 请求方法/路径/查询分支与响应解包；401 分流与豁免语义。
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest'

import { HttpResponse, http } from 'msw'
import { setupServer } from 'msw/node'

import { api, errorMessage, isNetworkError, isUnauthorized, onUnauthorized, setBaseUrlProvider, setTokenProvider } from './client'
import { handlers, resetFixtures } from './msw'

const server = setupServer(...handlers)

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }))
afterEach(() => {
  server.resetHandlers()
  resetFixtures()
})
afterAll(() => server.close())

beforeEach(() => {
  setBaseUrlProvider(() => 'http://mock:8900')
  setTokenProvider(async () => 'pony_admin_test')
})

describe('契约：tokens', () => {
  it('list 解包 tokens 数组且 status 枚举合法', async () => {
    const r = await api.listTokens()
    expect(r.tokens).toHaveLength(3)
    expect(r.tokens.map((t) => t.status)).toEqual(['active', 'active', 'revoked'])
  })

  it('create 返回明文 token 且仅一次', async () => {
    const r = await api.createToken({ name: 'dev' })
    expect(r.token).toMatch(/^pony_/)
    expect(r.expires_at).toBeNull()
  })

  it('create 非法名透传服务端固定文案（api 类错误）', async () => {
    const err = await api.createToken({ name: 'pony_bad' }).catch((e) => e)
    expect(err).toMatchObject({ kind: 'api', status: 400, error: 'invalid token name' })
    expect(errorMessage(err)).toBe('invalid token name')
  })

  it('revoke admin 行 400 cannot revoke admin', async () => {
    const err = await api.revokeToken(1).catch((e) => e)
    expect(err).toMatchObject({ kind: 'api', error: 'cannot revoke admin' })
  })
})

describe('契约：routes 三态 PATCH', () => {
  it('enabled=true 序列化为字段出现', async () => {
    const r = await api.patchRoute('gemini', { enabled: true })
    expect(r.enabled).toBe(true)
  })

  it('override_upstream=null（清除）与缺席（不改）均可序列化', async () => {
    // null → Some(None) 清除；字段缺席 → None 不改——两种请求都应被服务端接受
    const r1 = await api.patchRoute('openai', { override_upstream: null })
    expect(r1.name).toBe('x')
    const r2 = await api.patchRoute('openai', {})
    expect(r2.enabled).toBe(true)
  })

  it('upstream 快照字段出现即前置拒绝（C-P2-9）', () => {
    expect(() =>
      api.patchRoute('x', { upstream: 'worker' } as unknown as Parameters<typeof api.patchRoute>[1]),
    ).toThrow()
  })

  it('test 超时变体形状完整', async () => {
    const r = await api.testRoute('timeout')
    expect(r.ok).toBe(false)
    expect(r.error).toContain('timeout')
  })
})

describe('契约：usage 查询串分支', () => {
  it('hours 边界内正常返回 total 聚合', async () => {
    const r = await api.usage({ hours: 24 })
    expect(r.hours).toBe(24)
    expect(r.total.requests).toBe(13)
  })

  it('hours=0 / >720 服务端 400', async () => {
    for (const hours of [0, 721]) {
      const err = await api.usage({ hours }).catch((e) => e)
      expect(err).toMatchObject({ kind: 'api', status: 400 })
    }
  })
})

describe('契约：quota / alerts / health / monitor config', () => {
  it('quota 哨兵语义可解包（pct=-1/quota=-1）', async () => {
    const r = await api.quota()
    const vercel = r.snapshots.find((s) => s.upstream === 'vercel')
    expect(vercel?.pct).toBe(-1)
    expect(r.sources.map((s) => s.state)).toContain('unsupported_plan')
  })

  it('alerts unread=1 过滤 + limit 钳制 + 倒序', async () => {
    const all = await api.alerts(false, 500)
    expect(all.alerts.map((a) => a.id)).toEqual([7, 6])
    const unread = await api.alerts(true)
    expect(unread.alerts).toHaveLength(1)
    expect(unread.alerts[0].id).toBe(7)
  })

  it('mark read 幂等 200 与不存在 404', async () => {
    expect((await api.markAlertRead(7)).read).toBe(true)
    expect(await api.markAlertRead(999).catch((e) => e)).toMatchObject({ kind: 'api', status: 404 })
  })

  it('monitor/config 恰好两个字段（白名单端点契约）', async () => {
    const r = await api.monitorConfig()
    expect(Object.keys(r).sort()).toEqual(['poll_interval_sec', 'threshold_pct'])
  })

  it('health 状态灯数据可解包', async () => {
    const r = await api.health({ skipAuthRedirect: false })
    expect(r.db).toBe('ok')
    expect(r.routes.gemini.enabled).toBe(true)
  })
})

describe('错误分流表（R7/F8）', () => {
  it('无 token → 401 unauthorized；监听器触发且去抖', async () => {
    setTokenProvider(async () => null)
    let fired = 0
    const off = onUnauthorized(() => fired++)
    const results = await Promise.allSettled([api.health(), api.listTokens()])
    expect(results.every((r) => r.status === 'rejected')).toBe(true)
    for (const r of results) expect(isUnauthorized((r as PromiseRejectedResult).reason)).toBe(true)
    // 去抖窗口内合并为一次导航通知
    await new Promise((r) => setTimeout(r, 100))
    expect(fired).toBe(1)
    off()
    setTokenProvider(async () => 'pony_admin_test')
  })

  it('skipAuthRedirect 豁免：Settings 连接测试 401 不触发全局导航', async () => {
    setTokenProvider(async () => null)
    let fired = 0
    const off = onUnauthorized(() => fired++)
    const err = await api.health({ skipAuthRedirect: true }).catch((e) => e)
    expect(isUnauthorized(err)).toBe(true)
    await new Promise((r) => setTimeout(r, 100))
    expect(fired).toBe(0)
    off()
    setTokenProvider(async () => 'pony_admin_test')
  })

  it('网络不可达归为 network 类（不误判 401）', async () => {
    // MSW 按路径拦截（无视端口），故用 HttpResponse.error() 模拟链路层失败
    server.use(http.get('*/api/health', () => HttpResponse.error()))
    const err = await api.health().catch((e) => e)
    expect(isNetworkError(err)).toBe(true)
    expect(errorMessage(err)).not.toContain('token')
  })

  it('401 不清除凭据提供者（轮换期保护语义由调用方保证，此处锁行为）', async () => {
    setTokenProvider(async () => null)
    await api.health().catch(() => {})
    expect(await import('./client').then((m) => m.api.health().catch((e) => e.kind))).toBe('unauthorized')
    setTokenProvider(async () => 'pony_admin_test')
  })
})
