// gate（Node/Vercel）— WS↔TCP 隧道桥，与 deploy/cf-gate-worker 同协议（M6 spec §6）。
//
// 存在意义：CF Worker 平台禁止 connect() 到 Cloudflare IP（Worker→CF 回环防护），
// 且 OpenAI 按 AS13335 整段拉黑 CF 出口（ADR-002）——CF gate 对 openai.com 等
// Cloudflare 托管目标必然失败。本服务以真实服务器（AWS/VPS）IP 直连，作为
// 桌面端多端点 failover 的备用出口。
//
// 2026-08-30 修复（首帧吞没 bug）：
//   ws v8 的 message 回调中，文本帧的 data 同样是 Buffer——`isBinary || Buffer.isBuffer(data)`
//   恒为真，JSON 首帧被当二进制吞掉，连接永不建立（smoke-test.mjs 可复现）。
//   判定必须只用 isBinary。
//
// 两种运行形态：
// - standalone：`node server.js`（VPS systemd 常驻，见 systemd/pony-gate-node.service），
//   端点 /ws；
// - Vercel Function：Fluid compute 常驻，官方模式默认导出 http.Server，
//   端点 /api/ws（vercel.json maxDuration=120 即连接寿命上限，以实码为准）。
import http from 'node:http'
import net from 'node:net'
import crypto from 'node:crypto'
import { WebSocketServer } from 'ws'

const ALLOWED_PORTS = new Set([443]) // 80 明文透传默认禁用（对齐 cf-gate-worker，M6 spec R8/F12）
const MAX_NAME_LEN = 253
// 兼容两种挂载路径：standalone /ws 与 Vercel Function /api/ws
const GATE_PATHS = new Set(['/ws', '/api/ws'])

function sha256Hex(s) {
  return crypto.createHash('sha256').update(s).digest('hex')
}

/// hash 归一化：trim + 小写（与 cf-gate-worker 同口径，fail-closed）。
function normHash(h) {
  return typeof h === 'string' ? h.trim().toLowerCase() : ''
}

function isHashWellFormed(h) {
  return /^[0-9a-f]{64}$/.test(normHash(h))
}

function validHost(h) {
  if (!h || typeof h !== 'string' || h.length > MAX_NAME_LEN) return false
  const lower = h.toLowerCase().replace(/\.$/, '')
  if (lower === 'localhost' || lower.endsWith('.localhost')) return false
  if (/^(10\.|127\.|169\.254\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(lower)) return false
  if (/^0177\.|^0x7f\.|^\[?::1\]?$/.test(lower)) return false
  return true
}

export function createGateServer() {
  const server = http.createServer((req, res) => {
    const parsed = new URL(req.url, `http://${req.headers.host || 'localhost'}`)
    if (parsed.pathname === '/debug' || parsed.searchParams.has('debug')) {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      // 与 CF /debug 对齐：trim().length（env 尾换行时两端 len 一致才有排障价值）
      res.end(JSON.stringify({ set: typeof process.env.TUNNEL_TOKEN_HASH === 'string', len: (process.env.TUNNEL_TOKEN_HASH || '').trim().length }))
      return
    }
    if (parsed.pathname === '/' || parsed.pathname === '/api/ws' || parsed.pathname === '/ws') {
      res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' })
      res.end('Pony Gate (Node/Vercel) is running.')
      return
    }
    res.writeHead(404)
    res.end('not found')
  })

  const wss = new WebSocketServer({ noServer: true })

  server.on('upgrade', (req, socket, head) => {
    const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`)
    if (!GATE_PATHS.has(url.pathname)) {
      socket.destroy()
      return
    }
    const auth = req.headers['authorization'] ?? ''
    const presented = auth.startsWith('Bearer ') ? auth.slice(7).trim() : ''
    const expectedHash = normHash(process.env.TUNNEL_TOKEN_HASH)
    // fail-closed（同 deploy/vercel/api/proxy.js 安全基线）：env 缺失一律拒绝
    // 归一化：env 侧 trim+小写（消灭 dashboard/CLI 写入的尾换行与大小写漂移），presented 侧 trim。
    if (!expectedHash || !presented || sha256Hex(presented) !== expectedHash) {
      if (!isHashWellFormed(process.env.TUNNEL_TOKEN_HASH)) {
        console.log('[gate] TUNNEL_TOKEN_HASH malformed: len=', String(process.env.TUNNEL_TOKEN_HASH ?? '').trim().length)
      }
      socket.write('HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n')
      socket.destroy()
      return
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      wss.emit('connection', ws, req)
    })
  })

  wss.on('connection', (ws) => {
    let established = false
    let tcpSocket = null
    // 空闲超时防护（对齐 Fluid compute 计费优化）：
    // 若连接建立后或数据交互后超过 30 秒无任何上下行活动，主动释放连接，
    // 防止客户端失联或长挂导致持续消耗 Fluid Provisioned Memory。
    const IDLE_TIMEOUT_MS = 30_000
    let idleTimer = setTimeout(() => {
      ws.close(1000, 'idle timeout')
    }, IDLE_TIMEOUT_MS)

    function resetIdleTimer() {
      if (idleTimer) clearTimeout(idleTimer)
      idleTimer = setTimeout(() => {
        ws.close(1000, 'idle timeout')
      }, IDLE_TIMEOUT_MS)
    }

    ws.on('message', (data, isBinary) => {
      resetIdleTimer()
      // ws v8：文本帧与二进制帧的 data 都是 Buffer，只能靠 isBinary 区分
      if (isBinary) {
        if (tcpSocket && !tcpSocket.destroyed) {
          tcpSocket.write(data)
        }
        return
      }
      if (established) {
        if (tcpSocket && !tcpSocket.destroyed) {
          tcpSocket.write(data)
        }
        return
      }
      let req
      try {
        req = JSON.parse(data.toString())
      } catch {
        ws.close(1008, 'bad first frame')
        return
      }
      const port = Number(req.port)
      if (!validHost(req.host) || !ALLOWED_PORTS.has(port)) {
        ws.send(JSON.stringify({ ok: false, reason: 'acl denied' }))
        ws.close(1008, 'acl denied')
        return
      }

      // 标准 Node.js TCP 连接：对 OpenAI / Anthropic / Grok / Google 等所有目标均无限制
      const sock = net.createConnection({ host: req.host, port }, () => {
        established = true
        tcpSocket = sock
        ws.send(JSON.stringify({ ok: true }))
      })

      sock.on('data', (chunk) => {
        resetIdleTimer()
        if (ws.readyState === ws.OPEN) {
          ws.send(chunk)
        }
      })

      sock.on('error', (err) => {
        if (!established) {
          ws.send(JSON.stringify({ ok: false, reason: err.message }))
        }
        ws.close(1011, 'tcp error')
      })

      sock.on('close', () => {
        ws.close(1000, 'upstream closed')
      })
    })

    ws.on('close', () => {
      if (idleTimer) {
        clearTimeout(idleTimer)
        idleTimer = null
      }
      if (tcpSocket) {
        tcpSocket.destroy()
      }
    })
  })

  return server
}

// Vercel Functions 官方 WebSocket 模式：默认导出 http.Server 实例
// （Fluid compute 下模块常驻，单实例承载多连接）；standalone 模式由 server.js 复用同一实例。
export default createGateServer()
