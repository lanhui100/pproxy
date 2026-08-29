import http from 'http'
import net from 'net'
import crypto from 'crypto'
import { WebSocketServer } from 'ws'

const ALLOWED_PORTS = new Set([443])
const MAX_NAME_LEN = 253

function sha256Hex(s) {
  return crypto.createHash('sha256').update(s).digest('hex')
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
    if (req.url === '/debug') {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ set: typeof process.env.TUNNEL_TOKEN_HASH === 'string' }))
      return
    }
    if (req.url === '/') {
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
    if (url.pathname !== '/ws') {
      socket.destroy()
      return
    }
    const auth = req.headers['authorization'] ?? ''
    const presented = auth.startsWith('Bearer ') ? auth.slice(7) : ''
    const expectedHash = process.env.TUNNEL_TOKEN_HASH
    if (!presented || (expectedHash && sha256Hex(presented) !== expectedHash)) {
      socket.write('HTTP/1.1 401 Unauthorized\r\n\r\n')
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

    ws.on('message', (data, isBinary) => {
      if (isBinary || Buffer.isBuffer(data)) {
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
      if (tcpSocket) {
        tcpSocket.destroy()
      }
    })
  })

  return server
}

export default createGateServer()
