// egress-probe — CF Worker 出站 IP 地理探测（仅 workerd 运行时可用，node 无法导入）。
//
// 为什么用 connect()+startTls() 而不是 fetch()：
// 要测的正是转发 Google 流量走的那条出站链路。fetch() 子请求可能被 CF 骨干改道到
// 另一个 colo，而 connect() 从 Worker 所在 colo 出站——后者才是 Google 看到的那条路径。
//
// 注意：若运行时不支持 startTls()，本探测会抛错 → 缓存返回 unknown →
// shouldBlockEgress 对 Cloud Code 系 host fail-closed，即退化为"固定 failover 到
// 真实机房出口"。这是安全侧降级，不是静默放行。

import { connect } from 'cloudflare:sockets'

export const DEFAULT_EGRESS_GEO_URL = 'https://ipinfo.io/json'
export const DEFAULT_EGRESS_PROBE_TIMEOUT_MS = 1500

/** 解析探测 URL：仅允许 https:（明文探测无意义且会泄露目标）。 */
export function parseEgressProbeUrl(url) {
  const u = new URL(url)
  if (u.protocol !== 'https:') {
    throw new Error(`egress probe requires https: ${url}`)
  }
  return {
    hostname: u.hostname,
    port: Number(u.port || 443),
    path: `${u.pathname}${u.search || ''}`,
  }
}

function readOnce(reader, ms) {
  let timer = null
  return Promise.race([
    reader.read(),
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`egress probe read timeout (${ms}ms)`)), ms)
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer)
  })
}

/**
 * 探测本 Worker 的出站 IP 及其国家码。
 * @param {{url?: string, timeoutMs?: number}} [options]
 * @returns {Promise<{ip: string, country: string}>}
 */
export async function probeEgressGeo({
  url = DEFAULT_EGRESS_GEO_URL,
  timeoutMs = DEFAULT_EGRESS_PROBE_TIMEOUT_MS,
} = {}) {
  const { hostname, port, path } = parseEgressProbeUrl(url)
  const deadline = Date.now() + timeoutMs

  const socket = connect({ hostname, port }, { secureTransport: 'starttls' })
  if (typeof socket.startTls !== 'function') {
    throw new Error('egress probe: startTls unavailable in this runtime')
  }
  const secure = socket.startTls()
  const writer = secure.writable.getWriter()
  const reader = secure.readable.getReader()
  const decoder = new TextDecoder()

  try {
    await secure.opened
    await writer.write(
      new TextEncoder().encode(
        `GET ${path} HTTP/1.1\r\nHost: ${hostname}\r\n` +
          'User-Agent: pony-gate-egress-probe\r\nAccept: application/json\r\nConnection: close\r\n\r\n',
      ),
    )

    let buf = ''
    while (Date.now() < deadline) {
      const { value, done } = await readOnce(reader, Math.max(1, deadline - Date.now()))
      if (done) break
      buf += decoder.decode(value, { stream: true })
      const idx = buf.indexOf('\r\n\r\n')
      if (idx < 0) continue
      try {
        const body = JSON.parse(buf.slice(idx + 4))
        return { ip: String(body.ip || ''), country: String(body.country || '').toUpperCase() }
      } catch {
        // body 尚未收全，继续读
      }
    }
    throw new Error('egress probe: incomplete response')
  } finally {
    try {
      await writer.close()
    } catch {}
    try {
      await reader.cancel()
    } catch {}
    try {
      await secure.close()
    } catch {}
  }
}
