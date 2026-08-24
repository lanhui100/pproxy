import { describe, expect, it } from 'vitest'

import { errText } from './errors'

describe('errText', () => {
  it('已知服务端错误串译为中文', () => {
    expect(errText({ kind: 'api', status: 400, error: 'invalid token name' })).toBe('密钥名称不合法')
    expect(errText({ kind: 'api', status: 400, error: 'cannot revoke admin' })).toBe(
      '系统管理员令牌不可撤销',
    )
    expect(errText({ kind: 'api', status: 404, error: 'not_found' })).toContain('不存在')
  })

  it('未知 api 错误回退原文', () => {
    expect(errText({ kind: 'api', status: 400, error: 'some_future_error' })).toBe('some_future_error')
  })

  it('非 api 类沿用分流表文案', () => {
    expect(errText({ kind: 'network', message: '' })).toBe('无法连接后端（网络错误或地址不可达）')
    expect(errText({ kind: 'unauthorized' })).toBe('未授权：admin token 缺失或已失效')
    expect(errText({ kind: 'server', status: 500 })).toBe('服务端错误（HTTP 500）')
  })
})
