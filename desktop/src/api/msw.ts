// MSW handlers：形状按 docs/ops/API.md + schemas.ts 钉住；覆盖查询串分支
// （hours 边界 400、limit>500 钳制、unread 过滤）供契约测试与浏览器 dev 复用。
import { HttpResponse, http } from 'msw'

const now = 1_787_356_800
const ADMIN_BEARER = 'Bearer pony_admin_test'

/** 受保护端点统一鉴权（401 同体防枚举）；info 端点不鉴权故不在 mock 内。
 * 参数类型对齐 MSW ResponseResolverInfo（PathParams 值可为 string[]|undefined，
 * 统一摊平为 string 再交 handler）。 */
function withAuth(
  handler: (input: { request: Request; params: Record<string, string> }) => Response | Promise<Response>,
) {
  return async (info: {
    request: Request
    params?: Record<string, string | readonly string[] | undefined>
  }): Promise<Response> => {
    const { request } = info
    const flat: Record<string, string> = Object.fromEntries(
      Object.entries(info.params ?? {}).map(([k, v]) => {
        if (Array.isArray(v)) return [k, v.join('/')]
        if (typeof v === 'string') return [k, v]
        return [k, '']
      }),
    )
    if (request.headers.get('Authorization') !== ADMIN_BEARER) {
      return HttpResponse.json({ error: 'unauthorized' }, { status: 401 })
    }
    return handler({ request, params: flat })
  }
}

export const fixtures = {
  tokens: [
    { id: 1, name: '__admin__', created_at: now, expires_at: null, revoked_at: null, last_used_at: null, status: 'active' as const },
    { id: 2, name: 'ci-token', created_at: now, expires_at: null, revoked_at: null, last_used_at: now, status: 'active' as const },
    { id: 3, name: 'old', created_at: now, expires_at: now - 100, revoked_at: now - 50, last_used_at: now - 60, status: 'revoked' as const },
  ],
  routes: [
    { name: 'gemini', target_host: 'generativelanguage.googleapis.com', upstream: null, override_upstream: null, enabled: true, created_at: now, effective_upstream: 'worker' },
    { name: 'openai', target_host: 'api.openai.com', upstream: 'vercel', override_upstream: null, enabled: false, created_at: now, effective_upstream: 'vercel' },
  ],
  usageRows: [
    { route: 'openai', token_id: 2, requests: 10, bytes_in: 1000, bytes_out: 2000 },
    { route: 'gemini', token_id: 2, requests: 3, bytes_in: 300, bytes_out: 600 },
  ],
  alerts: [
    { id: 7, ts: now, level: 'warning' as const, message: 'cf requests_daily at 85.0% (85000/100000)', read_at: null },
    { id: 6, ts: now - 3600, level: 'critical' as const, message: 'vercel bandwidth at 96.1% (961/1000)', read_at: now - 1800 },
  ],
  quota: {
    snapshots: [
      { ts: now, upstream: 'cf', metric: 'requests_daily', used: 85_000, quota: 100_000, pct: 85.0 },
      { ts: now, upstream: 'vercel', metric: 'bandwidth', used: 4096, quota: -1, pct: -1.0 },
    ],
    sources: [
      { name: 'cf', state: 'ok' as const, last_ok: now },
      { name: 'vercel', state: 'unsupported_plan' as const, last_ok: null },
    ],
  },
  health: {
    status: 'ok',
    routes: { gemini: { enabled: true, upstream: 'worker' }, openai: { enabled: false, upstream: 'vercel' } },
    tokens_active: 2,
    db: 'ok',
  },
}

let createdTokens: Array<{ id: number; name: string; token: string }> = []
export function resetFixtures(): void {
  createdTokens = []
}

// 注意：特定路径处理器必须先于通配 :name 注册（MSW 首个匹配生效）
export const handlers = [
  // routes 特定：timeout 变体（先注册，避免被 :name/test 通配抢先）
  http.post('*/api/routes/timeout/test', withAuth(() =>
    HttpResponse.json({ ok: false, status: null, latency_ms: null, error: 'timeout after 10s' }))),

  // tokens
  http.get('*/api/tokens', withAuth(() => HttpResponse.json({ tokens: fixtures.tokens }))),
  http.post('*/api/tokens', withAuth(async ({ request }) => {
    const body = (await request.json()) as { name?: string; expires_days?: number }
    if (!body.name || body.name.startsWith('pony_')) {
      return HttpResponse.json({ error: 'invalid token name' }, { status: 400 })
    }
    const id = 100 + createdTokens.length
    const token = `pony_${String(id).repeat(32).slice(0, 32)}`
    createdTokens.push({ id, name: body.name, token })
    return HttpResponse.json({ id, name: body.name, token, expires_at: null }, { status: 201 })
  })),
  http.delete('*/api/tokens/:id', withAuth(({ params }) => {
    if (Number(params.id) === 1) return HttpResponse.json({ error: 'cannot revoke admin' }, { status: 400 })
    return HttpResponse.json({ revoked: true })
  })),

  // routes
  http.get('*/api/routes', withAuth(() => HttpResponse.json({ routes: fixtures.routes }))),
  http.post('*/api/routes', withAuth(async ({ request }) => {
    const body = (await request.json()) as { name?: string; target_host?: string }
    if (!body.name || body.name.includes('.')) {
      return HttpResponse.json({ error: 'invalid route name' }, { status: 400 })
    }
    return HttpResponse.json({ name: body.name, upstream: 'worker' }, { status: 201 })
  })),
  http.patch('*/api/routes/:name', withAuth(async ({ request }) => {
    const body = (await request.json()) as { override_upstream?: string | null; enabled?: boolean; upstream?: unknown }
    if ('upstream' in body && body.upstream !== undefined) {
      return HttpResponse.json({ error: 'invalid upstream' }, { status: 400 })
    }
    return HttpResponse.json({ name: 'x', enabled: body.enabled ?? true })
  })),
  http.delete('*/api/routes/:name', withAuth(() => HttpResponse.json({ deleted: true }))),
  http.post('*/api/routes/:name/test', withAuth(() =>
    HttpResponse.json({ ok: true, status: 200, latency_ms: 120, error: null }))),

  // usage（hours 边界分支：非 1..=720 → 400）
  http.get('*/api/usage', withAuth(({ request }) => {
    const url = new URL(request.url)
    const hours = Number(url.searchParams.get('hours') ?? 24)
    if (!Number.isInteger(hours) || hours < 1 || hours > 720) {
      return HttpResponse.json({ error: 'bad_request' }, { status: 400 })
    }
    const rows = fixtures.usageRows
    return HttpResponse.json({
      hours,
      since_hour: now - hours * 3600,
      rows,
      total: {
        requests: rows.reduce((a, r) => a + r.requests, 0),
        bytes_in: rows.reduce((a, r) => a + r.bytes_in, 0),
        bytes_out: rows.reduce((a, r) => a + r.bytes_out, 0),
      },
    })
  })),

  // quota / alerts / health / monitor config
  http.get('*/api/quota', withAuth(() => HttpResponse.json(fixtures.quota))),
  http.get('*/api/alerts', withAuth(({ request }) => {
    const url = new URL(request.url)
    const unread = url.searchParams.get('unread') === '1'
    const limit = Math.min(Math.max(Number(url.searchParams.get('limit') ?? 50), 1), 500)
    let list = [...fixtures.alerts].sort((a, b) => b.id - a.id) // 倒序
    if (unread) list = list.filter((a) => a.read_at === null)
    return HttpResponse.json({ alerts: list.slice(0, limit) })
  })),
  http.post('*/api/alerts/:id/read', withAuth(({ params }) => {
    if (Number(params.id) === 999) return HttpResponse.json({ error: 'not_found' }, { status: 404 })
    return HttpResponse.json({ read: true })
  })),
  http.get('*/api/health', withAuth(() => HttpResponse.json(fixtures.health))),
  http.get('*/api/monitor/config', withAuth(() =>
    HttpResponse.json({ threshold_pct: 80.0, poll_interval_sec: 3600 }))),
]
