import { describe, expect, it } from 'vitest'

import { cleanDomainInput, deriveDataPlane } from './urls'

describe('deriveDataPlane', () => {
  it('管理面端口替换为 8899', () => {
    expect(deriveDataPlane('http://100.100.100.10:8900')).toBe('http://100.100.100.10:8899')
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

describe('cleanDomainInput', () => {
  it('标准域名正常保留并小写化', () => {
    expect(cleanDomainInput('google.com')).toBe('google.com')
    expect(cleanDomainInput('  GitHub.COM. ')).toBe('github.com')
    expect(cleanDomainInput('one.google.com')).toBe('one.google.com')
  })

  it('剥除 http/https 协议和路径查询参数', () => {
    expect(cleanDomainInput('https://mail.google.com/mail/u/0/#inbox')).toBe('mail.google.com')
    expect(cleanDomainInput('http://api.github.com:443/repos?q=1')).toBe('api.github.com')
    expect(cleanDomainInput('//sub.domain.co.uk/path')).toBe('sub.domain.co.uk')
  })

  it('剥除通配符与前后缀点', () => {
    expect(cleanDomainInput('*.youtube.com')).toBe('youtube.com')
    expect(cleanDomainInput('..openai.com...')).toBe('openai.com')
  })

  it('非法输入返回 null', () => {
    expect(cleanDomainInput('')).toBeNull()
    expect(cleanDomainInput('   ')).toBeNull()
    expect(cleanDomainInput('http://')).toBeNull()
    expect(cleanDomainInput('invalid..domain')).toBeNull()
    expect(cleanDomainInput('http://[::1]:8080')).toBeNull()
  })

  it('支持清洗包裹引号', () => {
    expect(cleanDomainInput('"https://api.openai.com"')).toBe('api.openai.com')
    expect(cleanDomainInput("'github.com'")).toBe('github.com')
    expect(cleanDomainInput('`chatgpt.com`')).toBe('chatgpt.com')
  })
})

describe('parseServiceUrlInput', () => {
  it('解析标准 OpenAI URL 并推导 openai 预设', async () => {
    const { parseServiceUrlInput } = await import('./urls')
    const r = parseServiceUrlInput('https://api.openai.com/v1/chat/completions')
    expect(r).not.toBeNull()
    expect(r?.cleanHost).toBe('api.openai.com')
    expect(r?.inferredName).toBe('openai')
    expect(r?.suggestedPreset).toBe('openai')
  })

  it('解析 Anthropic URL 并推导 claude 预设', async () => {
    const { parseServiceUrlInput } = await import('./urls')
    const r = parseServiceUrlInput('https://api.anthropic.com/v1/messages')
    expect(r).not.toBeNull()
    expect(r?.cleanHost).toBe('api.anthropic.com')
    expect(r?.inferredName).toBe('anthropic')
    expect(r?.suggestedPreset).toBe('claude')
  })

  it('解析 Google Gemini URL 并提取 query 中的 API key', async () => {
    const { parseServiceUrlInput } = await import('./urls')
    const r = parseServiceUrlInput('https://generativelanguage.googleapis.com/v1beta/models?key=AIzaSySecret123')
    expect(r).not.toBeNull()
    expect(r?.cleanHost).toBe('generativelanguage.googleapis.com')
    expect(r?.inferredName).toBe('gemini')
    expect(r?.extractedKey).toBe('AIzaSySecret123')
    expect(r?.suggestedPreset).toBe('gemini')
  })

  it('解析带非标端口的 URL 并完整保留端口', async () => {
    const { parseServiceUrlInput } = await import('./urls')
    const r = parseServiceUrlInput('https://custom-proxy.example.com:8443/v1')
    expect(r).not.toBeNull()
    expect(r?.cleanHost).toBe('custom-proxy.example.com:8443')
    expect(r?.inferredName).toBe('custom-proxy')
  })

  it('解析本地 Ollama 实例', async () => {
    const { parseServiceUrlInput } = await import('./urls')
    const r = parseServiceUrlInput('http://127.0.0.1:11434/v1')
    expect(r).not.toBeNull()
    expect(r?.cleanHost).toBe('127.0.0.1:11434')
    expect(r?.inferredName).toBe('ollama')
  })
})
