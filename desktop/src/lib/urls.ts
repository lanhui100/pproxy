// 数据面地址推导：语义对齐 crates/cli/src/config.rs 的 derive_from_server ——
// 同 scheme + host、端口替换为 8899；路径/查询一律剥除。非法输入返回 null。
const DATA_PLANE_PORT = 8899

export function deriveDataPlane(adminUrl: string): string | null {
  const trimmed = adminUrl.trim()
  let scheme = ''
  let rest = ''
  if (trimmed.startsWith('https://')) {
    scheme = 'https'
    rest = trimmed.slice('https://'.length)
  } else if (trimmed.startsWith('http://')) {
    scheme = 'http'
    rest = trimmed.slice('http://'.length)
  } else {
    return null
  }
  // 去掉路径部分与尾斜杠
  const hostPort = rest.split('/')[0]
  if (!hostPort) return null
  // 端口解析对齐 Rust rsplit_once 语义：尾段为纯数字（含空串，与 Rust
  // `.all(is_ascii_digit)` 对空串返回 true 一致）即剥除；否则整段保留（R2-ENG-6）
  const lastColon = hostPort.lastIndexOf(':')
  const maybePort = lastColon >= 0 ? hostPort.slice(lastColon + 1) : null
  const stripped = maybePort !== null && /^[0-9]*$/.test(maybePort)
  const host = stripped ? hostPort.slice(0, lastColon) : hostPort
  if (!host) return null
  return `${scheme}://${host}:${DATA_PLANE_PORT}`
}

/**
 * 从用户输入中提取规范化域名（支持粘贴完整 URL、带端口、带通配符 *.、包裹引号等）。
 * 非法输入返回 null。
 */
export function cleanDomainInput(raw: string): string | null {
  let s = raw.trim().toLowerCase()
  if (!s) return null

  // 剥除两端包裹的单引号/双引号/反引号
  s = s.replace(/^["'`]/, '').replace(/["'`]$/, '').trim()

  // 剥除协议 http://, https://, ws:// 等
  s = s.replace(/^[a-z]+:\/\//, '').replace(/^\/\//, '')

  // 剥除路径、查询参数、hash
  s = s.split('/')[0]?.split('?')[0]?.split('#')[0] ?? ''

  // 剥除 IPv6 括号或端口
  if (s.startsWith('[') && s.includes(']')) {
    s = s.slice(1, s.indexOf(']'))
  } else if (s.includes(':')) {
    s = s.split(':')[0] ?? ''
  }

  // 剥除通配符前缀 *. 或 . 或 @
  s = s.replace(/^(\*\.|\.+|@+)/, '')
  // 剥除末尾点
  s = s.replace(/\.+$/, '')

  if (!s || s.length > 253) return null

  // 域名合法字符校验 (字母、数字、点、减号、下划线)
  if (!/^[a-z0-9_-]+(\.[a-z0-9_-]+)*$/.test(s)) {
    return null
  }

  return s
}

export interface ParsedServiceUrl {
  cleanHost: string
  inferredName: string
  extractedKey?: string
  suggestedPreset: 'claude' | 'cursor' | 'openai' | 'gemini' | 'ollama' | 'general'
  subPath: string
}

/**
 * 智能解析服务 URL，提取目标 Host、自动推导服务名称与客户端预设。
 */
export function parseServiceUrlInput(raw: string): ParsedServiceUrl | null {
  let s = raw.trim()
  if (!s) return null

  // 剥除两端包裹引号与 Markdown 链接语法 [text](url)
  s = s.replace(/^\[.*?\]\((.*?)\)$/, '$1').trim()
  s = s.replace(/^["'`]/, '').replace(/["'`]$/, '').trim()

  let extractedKey: string | undefined
  // 提取 query 中的 key= 或 api_key=
  if (s.includes('?')) {
    const queryPart = s.split('?')[1] ?? ''
    const matchKey = queryPart.match(/(?:key|api_key|token)=([a-zA-Z0-9_.-]+)/i)
    if (matchKey?.[1]) {
      extractedKey = matchKey[1]
    }
  }

  // 提取 scheme, hostPort, path
  let body = s.replace(/^[a-z]+:\/\//i, '').replace(/^\/\//, '')
  // 剥除 query 和 hash
  body = body.split('?')[0]?.split('#')[0] ?? ''

  const slashIdx = body.indexOf('/')
  const hostPort = (slashIdx >= 0 ? body.slice(0, slashIdx) : body).trim().toLowerCase()
  const subPath = slashIdx >= 0 ? body.slice(slashIdx) : ''

  if (!hostPort) return null

  // 验证 host 部分
  const hostOnly = hostPort.includes(':') ? (hostPort.split(':')[0] ?? '') : hostPort
  const isIp = /^(\d{1,3}\.){3}\d{1,3}$/.test(hostOnly)
  const isDomain = /^[a-z0-9_-]+(\.[a-z0-9_-]+)*$/.test(hostOnly)

  if (!isIp && !isDomain) return null

  // 推导服务名称与预设
  let inferredName = 'custom'
  let suggestedPreset: ParsedServiceUrl['suggestedPreset'] = 'general'

  if (hostPort.includes('openai.com')) {
    inferredName = 'openai'
    suggestedPreset = 'openai'
  } else if (hostPort.includes('anthropic.com') || hostPort.includes('claude')) {
    inferredName = 'anthropic'
    suggestedPreset = 'claude'
  } else if (hostPort.includes('googleapis.com') || hostPort.includes('gemini')) {
    inferredName = 'gemini'
    suggestedPreset = 'gemini'
  } else if (hostPort.includes('groq.com')) {
    inferredName = 'groq'
    suggestedPreset = 'openai'
  } else if (hostPort.includes('openrouter.ai')) {
    inferredName = 'openrouter'
    suggestedPreset = 'openai'
  } else if (hostPort.includes('mistral.ai')) {
    inferredName = 'mistral'
    suggestedPreset = 'openai'
  } else if (hostPort.includes('x.ai')) {
    inferredName = 'xai'
    suggestedPreset = 'openai'
  } else if (hostPort.includes('11434') || hostPort.includes('ollama')) {
    inferredName = 'ollama'
    suggestedPreset = 'ollama'
  } else {
    // 根据域名主体推导简短名称
    const parts = hostOnly.split('.')
    if (parts.length >= 3) {
      // 如 api.openai.com -> openai, custom-proxy.example.com -> custom-proxy
      if (['api', 'v1', 'gateway', 'proxy', 'ai'].includes(parts[0] ?? '')) {
        inferredName = parts[1] ?? 'service'
      } else {
        inferredName = parts[0] ?? 'service'
      }
    } else if (parts.length === 2) {
      inferredName = parts[0] ?? 'service'
    } else {
      inferredName = hostOnly.replace(/[^a-z0-9]/gi, '') || 'service'
    }
  }

  return {
    cleanHost: hostPort,
    inferredName,
    extractedKey,
    suggestedPreset,
    subPath,
  }
}
