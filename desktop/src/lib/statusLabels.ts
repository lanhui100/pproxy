// 枚举 → 中文状态映射（spec §9.2，StatusDot/徽章唯一出口，纯函数）：
// 已知值查表返回 { label, tone }；未知值灰点 + 原文小字（不猜语义），空值回退「未知」。
// tone 随对象一并返回，页面可直接 v-bind 到 <StatusDot>。
import type { QuotaSourceState, TokenStatus } from '@/api/client'

export type Tone = 'ok' | 'warn' | 'error' | 'muted' | 'accent'

export interface StatusView {
  label: string
  tone: Tone
}

const UNKNOWN: StatusView = { label: '未知', tone: 'muted' }

function viewOf(table: Record<string, StatusView>, raw: string): StatusView {
  if (!raw) return UNKNOWN
  return table[raw] ?? { label: raw, tone: 'muted' }
}

// QuotaSourceState 四态（上游额度）
const QUOTA_SOURCE_VIEWS: Record<QuotaSourceState, StatusView> = {
  ok: { label: '运行正常', tone: 'ok' },
  disabled: { label: '已停用', tone: 'muted' },
  error: { label: '异常', tone: 'error' },
  unsupported_plan: { label: '套餐不支持', tone: 'warn' },
}

// TokenStatus 三态（设备密钥）
const TOKEN_STATUS_VIEWS: Record<TokenStatus, StatusView> = {
  active: { label: '启用', tone: 'ok' },
  expired: { label: '已过期', tone: 'warn' },
  revoked: { label: '已撤销', tone: 'muted' },
}

// 上游出口（worker/vercel 为中性标识 → 主题强调色；cf 为 quota source 实际值，R2-UX-2）
const UPSTREAM_VIEWS: Record<string, StatusView> = {
  worker: { label: 'CF Worker', tone: 'accent' },
  vercel: { label: 'Vercel 出口', tone: 'accent' },
  cf: { label: 'CF Worker', tone: 'accent' },
}

// AlertLevel 两态（critical 置顶徽章用红）
const ALERT_LEVEL_VIEWS: Record<'warning' | 'critical', StatusView> = {
  warning: { label: '警告', tone: 'warn' },
  critical: { label: '严重', tone: 'error' },
}

export function quotaSourceLabel(state: string): StatusView {
  return viewOf(QUOTA_SOURCE_VIEWS, state)
}

export function tokenStatusLabel(status: string): StatusView {
  return viewOf(TOKEN_STATUS_VIEWS, status)
}

export function upstreamLabel(upstream: string): StatusView {
  return viewOf(UPSTREAM_VIEWS, upstream)
}

export function alertLevelView(level: string): StatusView {
  return viewOf(ALERT_LEVEL_VIEWS, level)
}
