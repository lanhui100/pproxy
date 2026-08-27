// statusLabels 纯函数测试：全枚举映射 + 未知值/空值回退（spec §9.2）
import { describe, expect, it } from 'vitest'

import { alertLevelView, quotaSourceLabel, tokenStatusLabel, upstreamLabel } from './statusLabels'

describe('quotaSourceLabel', () => {
  it('四态映射', () => {
    expect(quotaSourceLabel('ok')).toEqual({ label: '额度正常', tone: 'ok' })
    expect(quotaSourceLabel('disabled')).toEqual({ label: '未配监控', tone: 'muted' })
    expect(quotaSourceLabel('error')).toEqual({ label: '监控不可用', tone: 'muted' })
    expect(quotaSourceLabel('unsupported_plan')).toEqual({ label: '免费版无监控', tone: 'muted' })
  })
})

describe('tokenStatusLabel', () => {
  it('三态映射', () => {
    expect(tokenStatusLabel('active')).toEqual({ label: '启用', tone: 'ok' })
    expect(tokenStatusLabel('expired')).toEqual({ label: '已过期', tone: 'warn' })
    expect(tokenStatusLabel('revoked')).toEqual({ label: '已撤销', tone: 'muted' })
  })
})

describe('upstreamLabel', () => {
  it('worker/vercel 映射（专名保留）', () => {
    expect(upstreamLabel('worker')).toEqual({ label: 'CF Worker', tone: 'accent' })
    expect(upstreamLabel('vercel')).toEqual({ label: 'Vercel 出口', tone: 'accent' })
  })
})

describe('alertLevelView', () => {
  it('两态映射', () => {
    expect(alertLevelView('warning')).toEqual({ label: '警告', tone: 'warn' })
    expect(alertLevelView('critical')).toEqual({ label: '严重', tone: 'error' })
  })
})

describe('未知值回退（不猜语义）', () => {
  it('未知字符串 → 原文 + 灰点', () => {
    expect(quotaSourceLabel('weird_state')).toEqual({ label: 'weird_state', tone: 'muted' })
    expect(tokenStatusLabel('flying')).toEqual({ label: 'flying', tone: 'muted' })
    expect(upstreamLabel('fly_io')).toEqual({ label: 'fly_io', tone: 'muted' })
    expect(alertLevelView('info')).toEqual({ label: 'info', tone: 'muted' })
  })

  it('空值 → 「未知」+ 灰点', () => {
    const empty = { label: '未知', tone: 'muted' }
    expect(quotaSourceLabel('')).toEqual(empty)
    expect(tokenStatusLabel('')).toEqual(empty)
    expect(upstreamLabel('')).toEqual(empty)
  })
})
