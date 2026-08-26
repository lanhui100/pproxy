// API 契约 schema（单一真值源）：形状钉住 docs/ops/API.md，MSW handler 与
// 契约 smoke 脚本共用同一组 zod 定义，双向夹住前后端漂移（M5 spec §2）。
import { z } from 'zod'

// ---- tokens ----

export const TokenStatusSchema = z.enum(['active', 'expired', 'revoked'])
export type TokenStatus = z.infer<typeof TokenStatusSchema>

export const TokenDtoSchema = z.object({
  id: z.number(),
  name: z.string(),
  created_at: z.number(),
  expires_at: z.number().nullable(),
  revoked_at: z.number().nullable(),
  last_used_at: z.number().nullable(),
  status: TokenStatusSchema,
})
export type TokenDto = z.infer<typeof TokenDtoSchema>

export const TokensRespSchema = z.object({ tokens: z.array(TokenDtoSchema) })

export const CreateTokenReqSchema = z.object({
  name: z.string().min(1),
  expires_days: z.number().int().positive().optional(),
})
export const CreateTokenRespSchema = z.object({
  id: z.number(),
  name: z.string(),
  token: z.string(), // 明文仅此一次
  expires_at: z.number().nullable(),
})

// ---- routes ----

export const RouteDtoSchema = z.object({
  name: z.string(),
  target_host: z.string(),
  upstream: z.string().nullable(),
  override_upstream: z.string().nullable(),
  enabled: z.boolean(),
  created_at: z.number(),
  effective_upstream: z.string(),
})
export type RouteDto = z.infer<typeof RouteDtoSchema>

export const RoutesRespSchema = z.object({ routes: z.array(RouteDtoSchema) })

export const CreateRouteReqSchema = z.object({
  name: z.string().min(1),
  target_host: z.string().min(1),
  override_upstream: z.string().optional(),
})
export const CreateRouteRespSchema = z.object({
  name: z.string(),
  upstream: z.string(),
})

/// PATCH 三态（C-P0-2 double_option）：override_upstream 缺席=不改/null=清除/
/// 字符串=设置。zod 表达：字段可选且可空。
export const PatchRouteReqSchema = z.object({
  override_upstream: z.string().nullish(),
  enabled: z.boolean().optional(),
  // C-P2-9：upstream 快照列不可改——出现即前端前置拒绝
  upstream: z.never().optional(),
})

export const TestRouteRespSchema = z.object({
  ok: z.boolean(),
  status: z.number().nullable().optional(),
  latency_ms: z.number().nullable().optional(),
  error: z.string().nullable().optional(),
})

// ---- usage ----

export const UsageRowSchema = z.object({
  route: z.string(),
  token_id: z.number(),
  requests: z.number(),
  bytes_in: z.number(),
  bytes_out: z.number(),
})
export const UsageRespSchema = z.object({
  hours: z.number(),
  since_hour: z.number(),
  rows: z.array(UsageRowSchema),
  total: z.object({
    requests: z.number(),
    bytes_in: z.number(),
    bytes_out: z.number(),
  }),
})
export type UsageResp = z.infer<typeof UsageRespSchema>

// ---- quota / alerts / health ----

export const QuotaSourceStateSchema = z.enum(['ok', 'disabled', 'error', 'unsupported_plan'])
export const QuotaSnapshotSchema = z.object({
  ts: z.number(),
  upstream: z.string(),
  metric: z.string(),
  used: z.number(),
  quota: z.number(),
  pct: z.number(),
})
export const QuotaSourceSchema = z.object({
  name: z.string(),
  state: QuotaSourceStateSchema,
  last_ok: z.number().nullable(),
})
export const QuotaRespSchema = z.object({
  snapshots: z.array(QuotaSnapshotSchema),
  sources: z.array(QuotaSourceSchema),
})
export type QuotaResp = z.infer<typeof QuotaRespSchema>
export type QuotaSourceState = z.infer<typeof QuotaSourceStateSchema>

export const AlertDtoSchema = z.object({
  id: z.number(),
  ts: z.number(),
  level: z.enum(['warning', 'critical']),
  message: z.string(),
  read_at: z.number().nullable(),
})
export const AlertsRespSchema = z.object({ alerts: z.array(AlertDtoSchema) })
export type AlertDto = z.infer<typeof AlertDtoSchema>

export const HealthRespSchema = z.object({
  status: z.string(),
  routes: z.record(z.string(), z.object({ enabled: z.boolean(), upstream: z.string() })),
  tokens_active: z.number(),
  db: z.string(),
})
export type HealthResp = z.infer<typeof HealthRespSchema>

// ---- monitor config（M5 §6.1 白名单端点：仅两字段，无任何凭据）----

export const MonitorConfigRespSchema = z.object({
  threshold_pct: z.number(),
  poll_interval_sec: z.number(),
})
export type MonitorConfigResp = z.infer<typeof MonitorConfigRespSchema>

// ---- 隧道中继下发（桌面端「自动配置」数据源；未配置时两字段均为 null）----

export const TunnelConfigRespSchema = z.object({
  url: z.string().nullable(),
  token: z.string().nullable(),
})
export type TunnelConfigResp = z.infer<typeof TunnelConfigRespSchema>

// ---- 错误体 ----

export const ApiErrorBodySchema = z.object({ error: z.string() })
