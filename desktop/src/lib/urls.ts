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
