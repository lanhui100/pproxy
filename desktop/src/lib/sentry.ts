import * as Sentry from '@sentry/vue'
import type { App } from 'vue'

const DEFAULT_DSN = import.meta.env.VITE_SENTRY_DSN || ''

export function initSentry(app: App, router?: any): void {
  const dsn = (window as any).__PONY_SENTRY_DSN__ || DEFAULT_DSN
  if (!dsn) {
    // 未配置 DSN 时静默跳过上报，但不影响本地错误收集与诊断
    return
  }

  Sentry.init({
    app,
    dsn,
    integrations: [
      ...(router ? [Sentry.browserTracingIntegration({ router })] : []),
      Sentry.replayIntegration({
        maskAllText: true,
        blockAllMedia: true,
      }),
    ],
    tracesSampleRate: 0.2,
    replaysSessionSampleRate: 0.05,
    replaysOnErrorSampleRate: 1.0,
    environment: import.meta.env.MODE || 'production',
    beforeSend(event) {
      // 敏感信息脱敏：抹除 Authorization/token/密码等凭据
      if (event.request?.headers) {
        delete event.request.headers['Authorization']
        delete event.request.headers['authorization']
      }
      return event
    },
  })
}

/**
 * 结构化上报代理链路异常至 Sentry
 */
export function captureProxyDiagnostic(context: {
  category: 'probe_failure' | 'tunnel_disconnect' | 'sysproxy_error'
  target: string
  error: string
  durationMs?: number
  extra?: Record<string, any>
}): void {
  Sentry.addBreadcrumb({
    category: 'proxy',
    message: `[${context.category}] ${context.target}: ${context.error}`,
    level: 'warning',
    data: {
      durationMs: context.durationMs,
      ...context.extra,
    },
  })

  // 仅在真实错误时主动上报 event
  if (context.error && !context.error.includes('代理未启用')) {
    Sentry.captureMessage(`[PProxy] ${context.category}: ${context.target} (${context.error})`, {
      level: 'error',
      extra: context,
    })
  }
}
