import { describe, expect, it } from 'vitest'
import { generatePresetSnippets } from './presetGenerator'

describe('generatePresetSnippets', () => {
  it('生成 OpenAI 标准环境变量', () => {
    const snippets = generatePresetSnippets({
      dataPlaneBase: 'http://127.0.0.1:8899',
      token: 'tok_live_123',
      service: 'openai',
      upstreamKey: 'sk-my-openai-key',
    })
    const openai = snippets.find((s) => s.id === 'openai')
    expect(openai).toBeDefined()
    expect(openai?.psSnippet).toContain('$env:OPENAI_BASE_URL="http://127.0.0.1:8899/tok_live_123/openai/v1"')
    expect(openai?.psSnippet).toContain('$env:OPENAI_API_KEY="sk-my-openai-key"')
  })

  it('生成 Claude Code 完整环境变量', () => {
    const snippets = generatePresetSnippets({
      dataPlaneBase: 'https://access.example.com',
      token: 'tok_live_123',
      service: 'anthropic',
      upstreamKey: 'sk-ant-test',
    })
    const claude = snippets.find((s) => s.id === 'claude')
    expect(claude).toBeDefined()
    expect(claude?.bashSnippet).toContain('export ANTHROPIC_BASE_URL="https://access.example.com/tok_live_123/anthropic"')
    expect(claude?.bashSnippet).toContain('export ANTHROPIC_API_KEY="sk-ant-test"')
  })

  it('生成 Cursor 带 /v1 的 Base URL', () => {
    const snippets = generatePresetSnippets({
      dataPlaneBase: 'http://127.0.0.1:8899',
      token: 'tok_456',
      service: 'openai',
    })
    const cursor = snippets.find((s) => s.id === 'cursor')
    expect(cursor?.baseUrl).toBe('http://127.0.0.1:8899/tok_456/openai/v1')
  })

  it('Token 缺省时优雅降级为占位符', () => {
    const snippets = generatePresetSnippets({
      dataPlaneBase: 'http://100.1.2.3:8899',
      service: 'gemini',
    })
    const claude = snippets.find((s) => s.id === 'claude')
    expect(claude?.bashSnippet).toContain('http://100.1.2.3:8899/<YOUR_TOKEN>/gemini')
  })
})
