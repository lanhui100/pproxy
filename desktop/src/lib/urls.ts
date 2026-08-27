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
 * 从用户输入中提取规范化域名（支持粘贴完整 URL、带端口、带通配符 *. 等）。
 * 非法输入返回 null。
 */
export function cleanDomainInput(raw: string): string | null {
  let s = raw.trim().toLowerCase()
  if (!s) return null

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
