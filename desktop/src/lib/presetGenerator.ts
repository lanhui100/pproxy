export interface PresetOptions {
  dataPlaneBase: string
  token?: string
  service: string
  upstreamKey?: string
}

export interface PresetSnippet {
  id: 'openai' | 'claude' | 'cursor' | 'nextchat' | 'python' | 'curl'
  name: string
  title: string
  baseUrl: string
  psSnippet?: string
  bashSnippet?: string
  codeSnippet?: string
  notes?: string
}

export function generatePresetSnippets(opts: PresetOptions): PresetSnippet[] {
  const base = (opts.dataPlaneBase || 'http://127.0.0.1:8899').replace(/\/$/, '')
  const tok = opts.token?.trim() || '<YOUR_TOKEN>'
  const svc = opts.service.trim() || '<SERVICE>'
  const key = opts.upstreamKey?.trim() || '<YOUR_API_KEY>'

  const accessUrl = `${base}/${tok}/${svc}`
  const v1Url = `${accessUrl}/v1`

  return [
    {
      id: 'openai',
      name: 'OpenAI 环境变量',
      title: '标准 OpenAI 环境变量',
      baseUrl: v1Url,
      psSnippet: `$env:OPENAI_BASE_URL="${v1Url}"\n$env:OPENAI_API_KEY="${key}"`,
      bashSnippet: `export OPENAI_BASE_URL="${v1Url}"\nexport OPENAI_API_KEY="${key}"`,
      notes: '绝大多数支持 OpenAI 接口规范的 CLI、开源项目与脚本均读取此环境变量。',
    },
    {
      id: 'claude',
      name: 'Claude Code',
      title: 'Anthropic / Claude Code 终端',
      baseUrl: accessUrl,
      psSnippet: `$env:ANTHROPIC_BASE_URL="${accessUrl}"\n$env:ANTHROPIC_API_KEY="${key}"`,
      bashSnippet: `export ANTHROPIC_BASE_URL="${accessUrl}"\nexport ANTHROPIC_API_KEY="${key}"`,
      notes: '配置后在终端运行 claude 即可直接连接。',
    },
    {
      id: 'cursor',
      name: 'Cursor / VS Code',
      title: 'Cursor / VSCode AI 插件',
      baseUrl: v1Url,
      notes: '已自动携带 /v1。粘贴进 Cursor Settings → Models → Override OpenAI Base URL，API Key 填入你的上游密钥。',
    },
    {
      id: 'nextchat',
      name: 'NextChat / Chatbox',
      title: 'ChatGPT 客户端 / 网页应用',
      baseUrl: accessUrl,
      notes: '在客户端自定义接口地址填入上方 Base URL；API Key 填入你的上游 API Key（若上游无要求可填任意字符串）。',
    },
    {
      id: 'python',
      name: 'Python SDK',
      title: 'OpenAI 官方 Python 库',
      baseUrl: v1Url,
      codeSnippet: `from openai import OpenAI\n\nclient = OpenAI(\n    base_url="${v1Url}",\n    api_key="${key}",\n)`,
    },
    {
      id: 'curl',
      name: 'cURL / 终端',
      title: '命令行请求示例',
      baseUrl: v1Url,
      bashSnippet: `curl ${v1Url}/chat/completions \\\n  -H "Authorization: Bearer ${key}" \\\n  -H "Content-Type: application/json" \\\n  -d '{"model": "gpt-4o", "messages": [{"role": "user", "content": "hi"}]}'`,
    },
  ]
}
