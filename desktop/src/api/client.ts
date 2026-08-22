// API client：双后适配器（Tauri→plugin-http / 浏览器→原生 fetch）+ 错误分流表
// （spec §4）+ 401 全局拦截（豁免/去抖/不清凭据）。契约由 schemas.ts 钉住。
import { invoke } from '@tauri-apps/api/core'

import {
  AlertsRespSchema,
  ApiErrorBodySchema,
  CreateRouteReqSchema,
  CreateTokenReqSchema,
  CreateTokenRespSchema,
  CreateRouteRespSchema,
  HealthRespSchema,
  MonitorConfigRespSchema,
  PatchRouteReqSchema,
  QuotaRespSchema,
  RoutesRespSchema,
  TestRouteRespSchema,
  TokensRespSchema,
  UsageRespSchema,
} from './schemas'
import { z } from 'zod'

// 视图层类型回导（统一入口）
export type {
  AlertDto,
  HealthResp,
  MonitorConfigResp,
  QuotaResp,
  QuotaSourceState,
  RouteDto,
  TokenDto,
  TokenStatus,
  UsageResp,
} from './schemas' 

// ---- 错误分类（分流表实现）----

export type ApiError =
  | { kind: 'network'; message: string } // fetch reject/超时 → 页内横幅，不跳 Settings
  | { kind: 'unauthorized' } // 401 → 全局拦截跳 Settings（豁免名单除外）
  | { kind: 'api'; status: number; error: string } // 其他 4xx → error 字段原样上屏
  | { kind: 'server'; status: number } // 5xx → 页内横幅 + 重试

export function isUnauthorized(e: unknown): e is Extract<ApiError, { kind: 'unauthorized' }> {
  return typeof e === 'object' && e !== null && (e as { kind?: string }).kind === 'unauthorized'
}

export function isNetworkError(e: unknown): e is Extract<ApiError, { kind: 'network' }> {
  return typeof e === 'object' && e !== null && (e as { kind?: string }).kind === 'network'
}

/** 提取可上屏文案（api 类透传服务端固定文案，其余给通用描述——不泄露内部信息）。 */
export function errorMessage(e: unknown): string {
  if (typeof e === 'object' && e !== null && 'kind' in e) {
    const err = e as ApiError
    switch (err.kind) {
      case 'network':
        return '无法连接后端（网络错误或地址不可达）'
      case 'unauthorized':
        return '未授权：admin token 缺失或已失效'
      case 'api':
        return err.error
      case 'server':
        return `服务端错误（HTTP ${err.status}）`
    }
  }
  return String(e)
}

// ---- 环境与端点配置 ----

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** 默认 token 解析：Tauri→凭据库；浏览器 dev→localStorage；node 测试→null。 */
async function defaultResolveToken(): Promise<string | null> {
  if (inTauri()) {
    try {
      return await invoke<string | null>('credential_get')
    } catch {
      return null
    }
  }
  if (typeof localStorage !== 'undefined') {
    return localStorage.getItem('pony-dev-admin-token')
  }
  return null
}

/** token 提供者：由 stores/backend 在启动时装配；测试直接注入。 */
let tokenProvider: () => Promise<string | null> = defaultResolveToken
export function setTokenProvider(p: () => Promise<string | null>): void {
  tokenProvider = p
}

// ---- 401 全局拦截（R7/F8）----

type UnauthorizedListener = () => void
const unauthorizedListeners = new Set<UnauthorizedListener>()
let unauthorizedDebounce: ReturnType<typeof setTimeout> | null = null

/**
 * 订阅 401 事件（App 壳注册一次 → 跳 Settings）。去抖：五页并发拉取同时炸 N
 * 个 401 时只触发一次导航。
 */
export function onUnauthorized(fn: UnauthorizedListener): () => void {
  unauthorizedListeners.add(fn)
  return () => unauthorizedListeners.delete(fn)
}

/**
 * 豁免名单：Settings 的连接测试等调用传 `skipAuthRedirect: true`——401 只返回
 * 给调用方展示，不再触发全局导航（防死循环）；且**绝不清除 keyring 凭据**
 * （服务端轮换期间不能把好凭据洗掉）。
 */
interface RequestOptions {
  skipAuthRedirect?: boolean
}

function notifyUnauthorized(opts?: RequestOptions): void {
  if (opts?.skipAuthRedirect) return
  if (unauthorizedDebounce) return
  unauthorizedDebounce = setTimeout(() => {
    unauthorizedDebounce = null
    for (const fn of unauthorizedListeners) fn()
  }, 50)
}

// ---- 底层请求 ----

function defaultBaseUrl(): string {
  if (typeof localStorage !== 'undefined') {
    return localStorage.getItem('pony-backend-url') ?? ''
  }
  return ''
}
let baseUrlProvider: () => string = defaultBaseUrl
export function setBaseUrlProvider(p: () => string): void {
  baseUrlProvider = p
}

async function rawRequest(
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE',
  path: string,
  body?: unknown,
  opts?: RequestOptions,
): Promise<Response> {
  const base = baseUrlProvider().replace(/\/$/, '')
  const url = `${base}${path}`
  const token = await tokenProvider()
  const headers: Record<string, string> = {}
  if (token) headers.Authorization = `Bearer ${token}`
  if (body !== undefined) headers['Content-Type'] = 'application/json'

  let resp: Response
  if (inTauri()) {
    // R1：WebView 内禁用原生外联（CSP connect-src 'none'），走 plugin-http
    //（Rust 侧发出，scope 白名单锁定管理面地址）
    const { fetch: tauriFetch } = await import('@tauri-apps/plugin-http')
    resp = await tauriFetch(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) })
  } else {
    resp = await fetch(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) })
  }

  if (resp.status === 401) {
    notifyUnauthorized(opts)
    throw { kind: 'unauthorized' } satisfies ApiError
  }
  if (!resp.ok) {
    const text = await resp.text()
    const parsed = ApiErrorBodySchema.safeParse(text ? JSON.parse(text) : {})
    throw parsed.success
      ? ({ kind: resp.status < 500 ? 'api' : 'server', status: resp.status, error: parsed.data.error } satisfies ApiError)
      : ({ kind: resp.status < 500 ? 'api' : 'server', status: resp.status, error: 'bad_request' } satisfies ApiError)
  }
  return resp
}

async function request<S extends z.ZodTypeAny>(
  schema: S,
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE',
  path: string,
  body?: unknown,
  opts?: RequestOptions,
): Promise<z.infer<S>> {
  let resp: Response
  try {
    resp = await rawRequest(method, path, body, opts)
  } catch (e) {
    if (typeof e === 'object' && e !== null && 'kind' in e) throw e
    throw { kind: 'network', message: String(e) } satisfies ApiError
  }
  const json: unknown = await resp.json()
  return schema.parse(json) // 形状漂移在此显式失败（fail-fast）
}

// ---- 端点方法（全部消费端点，形状=API.md）----

export const api = {
  health: (opts?: RequestOptions) => request(HealthRespSchema, 'GET', '/api/health', undefined, opts),

  // tokens
  listTokens: () => request(TokensRespSchema, 'GET', '/api/tokens'),
  createToken: (req: z.infer<typeof CreateTokenReqSchema>) => {
    CreateTokenReqSchema.parse(req) // 出参前校验（含 expires_days 正整数）
    return request(CreateTokenRespSchema, 'POST', '/api/tokens', req)
  },
  revokeToken: (id: number) =>
    request(z.object({ revoked: z.boolean() }), 'DELETE', `/api/tokens/${id}`),

  // routes
  listRoutes: () => request(RoutesRespSchema, 'GET', '/api/routes'),
  createRoute: (req: z.infer<typeof CreateRouteReqSchema>) => {
    CreateRouteReqSchema.parse(req)
    return request(CreateRouteRespSchema, 'POST', '/api/routes', req)
  },
  patchRoute: (name: string, req: z.infer<typeof PatchRouteReqSchema>) => {
    PatchRouteReqSchema.parse(req)
    return request(z.object({ name: z.string(), enabled: z.boolean() }), 'PATCH', `/api/routes/${encodeURIComponent(name)}`, req)
  },
  deleteRoute: (name: string) =>
    request(z.object({ deleted: z.boolean() }), 'DELETE', `/api/routes/${encodeURIComponent(name)}`),
  testRoute: (name: string, opts?: RequestOptions) =>
    request(TestRouteRespSchema, 'POST', `/api/routes/${encodeURIComponent(name)}/test`, undefined, opts),

  // usage / quota
  usage: (params: { hours?: number; route?: string; token_id?: number }) => {
    const q = new URLSearchParams()
    if (params.hours !== undefined) q.set('hours', String(params.hours))
    if (params.route) q.set('route', params.route)
    if (params.token_id !== undefined) q.set('token_id', String(params.token_id))
    const qs = q.toString()
    return request(UsageRespSchema, 'GET', `/api/usage${qs ? `?${qs}` : ''}`)
  },
  quota: () => request(QuotaRespSchema, 'GET', '/api/quota'),

  // alerts
  alerts: (unreadOnly?: boolean, limit?: number) => {
    const q = new URLSearchParams()
    if (unreadOnly) q.set('unread', '1')
    if (limit !== undefined) q.set('limit', String(Math.min(Math.max(limit, 1), 500)))
    const qs = q.toString()
    return request(AlertsRespSchema, 'GET', `/api/alerts${qs ? `?${qs}` : ''}`)
  },
  markAlertRead: (id: number) =>
    request(z.object({ read: z.boolean() }), 'POST', `/api/alerts/${id}/read`),

  // monitor config（M5 §6.1 白名单端点）
  monitorConfig: (opts?: RequestOptions) =>
    request(MonitorConfigRespSchema, 'GET', '/api/monitor/config', undefined, opts),
}
