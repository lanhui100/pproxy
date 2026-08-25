// 输入规范化测试（M5 验收实测回归：URL 空格导致连接失败）
import { describe, expect, it } from 'vitest'

import { normalizeBaseUrl, normalizeToken } from './normalize'

describe('normalizeBaseUrl', () => {
  it('去首尾空格与结尾斜杠', () => {
    expect(normalizeBaseUrl(' http://100.100.100.10:8900 ')).toBe('http://100.100.100.10:8900')
    expect(normalizeBaseUrl('http://100.100.100.10:8900/')).toBe('http://100.100.100.10:8900')
    expect(normalizeBaseUrl('http://100.100.100.10:8900///')).toBe('http://100.100.100.10:8900')
  })

  it('自动补 http:// 前缀', () => {
    expect(normalizeBaseUrl('100.100.100.10:8900')).toBe('http://100.100.100.10:8900')
    expect(normalizeBaseUrl('devserver.tailnet-example.ts.net:8900')).toBe(
      'http://devserver.tailnet-example.ts.net:8900',
    )
  })

  it('保留 https 且大小写不敏感', () => {
    expect(normalizeBaseUrl('HTTPS://Example.com:8900')).toBe('HTTPS://Example.com:8900')
  })

  it('清除零宽不可见字符（网页复制常见）', () => {
    expect(normalizeBaseUrl('http://100.100.100.10:8900\u200b')).toBe('http://100.100.100.10:8900')
    expect(normalizeBaseUrl('\ufeffhttp://x:1')).toBe('http://x:1')
  })

  it('空输入返回空串', () => {
    expect(normalizeBaseUrl('')).toBe('')
    expect(normalizeBaseUrl('   ')).toBe('')
  })
})

describe('normalizeToken', () => {
  it('去所有空白（粘贴常带换行/尾随空格）', () => {
    expect(normalizeToken('pony_admin_abc\n')).toBe('pony_admin_abc')
    expect(normalizeToken(' pony_admin_abc ')).toBe('pony_admin_abc')
    expect(normalizeToken('pony_admin_ abc def')).toBe('pony_admin_abcdef')
  })

  it('零宽字符同样清除', () => {
    expect(normalizeToken('pony_admin_a\u200bbc')).toBe('pony_admin_abc')
  })
})
