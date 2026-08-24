// gate worker — M6 spec §6：WS↔TCP 隧道桥（TLS 端到端透传，Worker 只见密文）
// relay 模式采用 CF 官方文档规范：sock.readable.pipeTo(WritableStream) +
// ws message → writer.write（此前手搓 for-await/双闭包模式触发本地 workerd 崩溃）。
import { connect } from 'cloudflare:sockets'

const ALLOWED_PORTS = new Set([443]) // 80 明文透传默认禁用（M6 spec R8/F12）
const MAX_NAME_LEN = 253

async function sha256Hex(s) {
  const b = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s))
  return [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('')
}

function validHost(h) {
  if (!h || typeof h !== 'string' || h.length > MAX_NAME_LEN) return false
  const lower = h.toLowerCase().replace(/\.$/, '')
  if (lower === 'localhost' || lower.endsWith('.localhost')) return false
  if (/^(10\.|127\.|169\.254\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(lower)) return false
  if (/^0177\.|^0x7f\.|^\[?::1\]?$/.test(lower)) return false
  return true
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url)
    if (url.pathname === '/debug') {
      return new Response(
        JSON.stringify({ set: typeof env.TUNNEL_TOKEN_HASH === 'string' }),
        { headers: { 'content-type': 'application/json' } },
      )
    }
    if (url.pathname !== '/ws') return new Response('not found', { status: 404 })
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') {
      return new Response('websocket required', { status: 400 })
    }
    const auth = request.headers.get('Authorization') ?? ''
    const presented = auth.startsWith('Bearer ') ? auth.slice(7) : ''
    const hash = await sha256Hex(presented)
    if (!presented || hash !== env.TUNNEL_TOKEN_HASH) {
      return new Response('unauthorized', { status: 401 })
    }

    const pair = new WebSocketPair()
    // 注意：本 workerd 版本禁止 accept() 后再返回 Response——返回即自动接手
    const server = pair[1]
    ctx.acceptWebSocket(server)

    let established = false
    let writer = null

    server.addEventListener('message', async (event) => {
      if (event.data instanceof ArrayBuffer) {
        writer?.write(new Uint8Array(event.data))
        return
      }
      if (established) return
      let req
      try {
        req = JSON.parse(event.data)
      } catch {
        server.close(1008, 'bad first frame')
        return
      }
      const port = Number(req.port)
      if (!validHost(req.host) || !ALLOWED_PORTS.has(port)) {
        server.send(JSON.stringify({ ok: false, reason: 'acl denied' }))
        server.close(1008, 'acl denied')
        return
      }
      try {
        console.log('[gate] connecting', req.host, port)
        const sock = connect({ hostname: req.host, port })
        const up = sock.writable.getWriter()
        await sock.opened
        established = true
        server.send(JSON.stringify({ ok: true }))
        // TCP→WS：官方规范 pipe 模式
        sock.readable
          .pipeTo(
            new WritableStream({
              write(chunk) {
                try {
                  server.send(chunk)
                } catch {}
              },
            }),
          )
          .catch(() => {
            try { server.close(1000, 'upstream closed') } catch {}
          })
        writer = up
      } catch (e) {
        console.log('[gate] connect error', String(e))
        server.send(JSON.stringify({ ok: false, reason: String(e) }))
        try { server.close(1011, 'connect failed') } catch {}
      }
    })

    server.addEventListener('close', () => {
      writer?.close().catch(() => {})
    })

    return new Response(null, { status: 101, webSocket: server })
  },
}
