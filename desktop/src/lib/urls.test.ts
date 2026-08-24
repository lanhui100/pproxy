import { describe, expect, it } from 'vitest'

import { deriveDataPlane } from './urls'

describe('deriveDataPlane', () => {
  it('管理面端口替换为 8899', () => {
    expect(deriveDataPlane('http://<TAILNET_IP>:8900')).toBe('http://<TAILNET_IP>:8899')
  })

  it('无端口的管理面地址追加 8899', () => {
    expect(deriveDataPlane('https://a.b.example')).toBe('https://a.b.example:8899')
    expect(deriveDataPlane('http://my-gateway.lan')).toBe('http://my-gateway.lan:8899')
  })

  it('保留 https scheme 并剥除路径/尾斜杠', () => {
    expect(deriveDataPlane('https://gw.example:8900/')).toBe('https://gw.example:8899')
    expect(deriveDataPlane('http://gw.example:8900/api/base')).toBe('http://gw.example:8899')
  })

  it('公网形态同样按端口规则推导', () => {
    expect(deriveDataPlane('https://access.ponyjob.top')).toBe('https://access.ponyjob.top:8899')
  })

  it('非法输入返回 null', () => {
    expect(deriveDataPlane('')).toBeNull()
    expect(deriveDataPlane('   ')).toBeNull()
    expect(deriveDataPlane('ftp://x.example')).toBeNull()
    expect(deriveDataPlane('http://')).toBeNull()
    expect(deriveDataPlane('http:///path')).toBeNull()
    expect(deriveDataPlane('just-text')).toBeNull()
  })

  it('IPv6 字面量带端口时取最后冒号分段', () => {
    expect(deriveDataPlane('http://[::1]:8900')).toBe('http://[::1]:8899')
  })

  it('IPv6 无端口字面量整段保留（对齐 Rust _ 臂）', () => {
    expect(deriveDataPlane('http://[2001:db8::1]')).toBe('http://[2001:db8::1]:8899')
  })

  it('含冒号但尾段非数字：整段保留为 host（对齐 Rust）', () => {
    expect(deriveDataPlane('http://host:foo')).toBe('http://host:foo:8899')
  })

  it('尾部空端口串按数字语义剥除（对齐 Rust 空串 all(digit)）', () => {
    expect(deriveDataPlane('http://host:/x')).toBe('http://host:8899')
  })
})
