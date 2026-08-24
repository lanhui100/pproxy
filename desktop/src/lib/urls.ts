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
  // 端口为纯数字则剥掉，host 留存；无端口或非数字段视作 host 本体
  const host = hostPort.includes(':')
    ? (() => {
        const idx = hostPort.lastIndexOf(':')
        const maybePort = hostPort.slice(idx + 1)
        return maybePort.length > 0 && /^[0-9]+$/.test(maybePort) ? hostPort.slice(0, idx) : null
      })()
    : hostPort
  if (!host) return null
  return `${scheme}://${host}:${DATA_PLANE_PORT}`
}
