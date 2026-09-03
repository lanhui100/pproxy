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
  // 剥除中英文全半角引号、书名号与反引号
  s = s.replace(/^[“"‘'『「`]/, '').replace(/[”"’'』」`]$/, '').trim()

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

  // 验证 host 部分（剥离端口与末尾点）
  const hostOnly = (hostPort.includes(':') ? (hostPort.split(':')[0] ?? '') : hostPort).replace(/\.+$/, '')
  const isIp = /^(\d{1,3}\.){3}\d{1,3}$/.test(hostOnly)
  const isDomain = /^[a-z0-9_-]+(\.[a-z0-9_-]+)*$/.test(hostOnly)

  if (!isIp && !isDomain) return null

  // 推导服务名称与预设（严格对齐 Rust PROVIDER_ROUTES 映射）
  let inferredName = 'custom'
  let suggestedPreset: ParsedServiceUrl['suggestedPreset'] = 'general'

  if (hostOnly.includes('openai.com')) {
    inferredName = 'openai'
    suggestedPreset = 'openai'
  } else if (hostOnly.includes('anthropic.com') || hostOnly.includes('claude')) {
    inferredName = 'anthropic'
    suggestedPreset = 'claude'
  } else if (hostOnly.includes('googleapis.com') || hostOnly.includes('gemini')) {
    inferredName = 'gemini'
    suggestedPreset = 'gemini'
  } else if (hostOnly.includes('groq.com')) {
    inferredName = 'groq'
    suggestedPreset = 'openai'
  } else if (hostOnly.includes('openrouter.ai')) {
    inferredName = 'openrouter'
    suggestedPreset = 'openai'
  } else if (hostOnly.includes('mistral.ai')) {
    inferredName = 'mistral'
    suggestedPreset = 'openai'
  } else if (hostOnly.includes('x.ai')) {
    inferredName = 'xai'
    suggestedPreset = 'openai'
  } else if (hostOnly.includes('twitter.com')) {
    inferredName = 'x'
    suggestedPreset = 'general'
  } else if (hostOnly.includes('huggingface.co')) {
    inferredName = 'hf'
    suggestedPreset = 'general'
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

  // 路由合法性对齐服务端 `^[a-z][a-z0-9_-]{0,63}$`：两端清理 - 与 _，首字符非字母垫付 x，防御 pony_ 前缀，截断 63
  inferredName = inferredName.replace(/^[-_]+/, '').replace(/[-_]+$/, '')
  if (!/^[a-z]/.test(inferredName)) {
    inferredName = `x${inferredName}`
  }
  if (inferredName.startsWith('pony_')) {
    inferredName = `x${inferredName}`
  }
  inferredName = inferredName.slice(0, 63)

  return {
    cleanHost: hostPort,
    inferredName,
    extractedKey,
    suggestedPreset,
    subPath,
  }
}

/**
 * 安全且可靠地在系统默认浏览器中打开外部 URL（兼容 Tauri 与 Web 运行环境）
 */
export async function openExternalUrl(url: string): Promise<void> {
  const isTauriEnv = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
  if (isTauriEnv) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('open_external_url', { url })
      return
    } catch (e) {
      console.error('Failed to open external url via tauri command:', e)
    }
  }
  if (typeof window !== 'undefined') {
    window.open(url, '_blank', 'noopener,noreferrer')
  }
}

/**
 * API 反代地址生成结果（对齐 Rust `proxy_access_url_generate` 命令返回结构）。
 * 语义：反代地址 = {数据面基址}/{token}/{route}，本地 127.0.0.1:8899、公网 access.ponyjob.top。
 */
export interface AccessUrlResult {
  /** 本地数据面接入地址，形如 http://127.0.0.1:8899/{token}/{route} */
  local_url: string
  /** 公网数据面接入地址，形如 https://access.ponyjob.top/{token}/{route} */
  public_url: string
  /** 推导出的服务路由名（如 anthropic / openai / gemini） */
  route: string
  /** 是否已使用本机保存的加速授权码填入令牌段；false 时为 <token> 占位 */
  has_token: boolean
}

/** 浏览器 dev 环境的兜底推导：与 Rust normalize_provider_base_url + infer_route 对齐（打包产物不含此路径）。 */
export function buildAccessUrlDev(baseUrl: string): AccessUrlResult {
  const parsed = parseServiceUrlInput(baseUrl)
  if (!parsed) throw new Error('无法识别的模型提供商地址，请粘贴形如 https://api.anthropic.com 的 base_url')
  const token = (typeof localStorage !== 'undefined' ? localStorage.getItem('pony-dev-tunnel-token') : null)?.trim()
  const seg = token || '<token>'
  return {
    local_url: `http://127.0.0.1:8899/${seg}/${parsed.inferredName}`,
    public_url: `https://access.ponyjob.top/${seg}/${parsed.inferredName}`,
    route: parsed.inferredName,
    has_token: Boolean(token),
  }
}
