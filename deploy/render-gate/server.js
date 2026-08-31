// render-gate — WS↔TCP 隧道桥（Render 美区出口，协议对齐 deploy/cf-gate-worker/worker.js）
//
// 协议：WS Upgrade（Authorization: Bearer <token>）→ 首帧 Text JSON {"host","port"}
//      → {"ok":true} → Binary 双向透传；拒绝 → {"ok":false,"reason":...} + close。
// 防线：token SHA-256 恒定时间比对、仅 443、私网/保留地址封禁（字面量 + DNS 解析双重校验）、
//      每 IP 鉴权失败锁定、首帧超时、日志零 token。
'use strict'

const http = require('http')
const crypto = require('crypto')
const net = require('net')
const dns = require('dns').promises
const { WebSocketServer } = require('ws')

const PORT = Number(process.env.PORT) || 3000
const TOKEN_HASH = (process.env.TUNNEL_TOKEN_HASH || '').trim().toLowerCase()
const ALLOWED_PORTS = new Set([443])
// 仅自检/调试注入：生产环境绝不设置。EXTRA_PORT 追加放行端口；ALLOW_PRIVATE=1 跳过私网封禁。
if (process.env.RENDER_GATE_EXTRA_PORT) {
  const p = Number(process.env.RENDER_GATE_EXTRA_PORT)
  if (Number.isInteger(p) && p > 0 && p < 65536) ALLOWED_PORTS.add(p)
}
const ALLOW_PRIVATE = process.env.RENDER_GATE_ALLOW_PRIVATE === '1'
const MAX_NAME_LEN = 253
const FIRST_FRAME_TIMEOUT_MS = 10_000
const DNS_TIMEOUT_MS = 5_000
const CONNECT_TIMEOUT_MS = 10_000
// 鉴权失败锁定：5 次失败锁 60s（每 IP），防公网爆破
const AUTH_FAIL_LIMIT = 5
const AUTH_LOCK_MS = 60_000

if (!/^[0-9a-f]{64}$/.test(TOKEN_HASH)) {
  console.error('[render-gate] FATAL: TUNNEL_TOKEN_HASH 缺失或不是 64 位 hex（sha256）')
  process.exit(1)
}

function sha256Hex(s) {
  return crypto.createHash('sha256').update(s, 'utf8').digest('hex')
}

function tokenOk(presented) {
  if (!presented) return false
  const h = sha256Hex(presented)
  const a = Buffer.from(h, 'utf8')
  const b = Buffer.from(TOKEN_HASH, 'utf8')
  return a.length === b.length && crypto.timingSafeEqual(a, b)
}

// ---- 鉴权失败锁定（每 IP；Map 有上限防伪造源 IP 撑爆内存）----
const AUTH_FAILS_MAX_ENTRIES = 10_000
const authFails = new Map() // ip -> { count, lockedUntil }
function clientIp(req) {
  // XFF 链取**最右**值：Render 反代追加的是真实客户端 IP；
  // 取最左会信任客户端伪造的头（锁定绕过/诬陷）。直连（无反代）时 XFF 整体不可信，回退 socket 地址。
  const fwd = req.headers['x-forwarded-for']
  if (typeof fwd === 'string' && fwd.length > 0) {
    const parts = fwd.split(',')
    return parts[parts.length - 1].trim()
  }
  return req.socket.remoteAddress || 'unknown'
}
function isLocked(ip) {
  const rec = authFails.get(ip)
  if (!rec) return false
  if (rec.lockedUntil && rec.lockedUntil > Date.now()) return true
  if (rec.lockedUntil && rec.lockedUntil <= Date.now()) authFails.delete(ip)
  return false
}
function recordAuthFail(ip) {
  if (authFails.size >= AUTH_FAILS_MAX_ENTRIES) authFails.clear() // 超限整体重置（锁定状态牺牲，防内存 DoS）
  const rec = authFails.get(ip) || { count: 0, lockedUntil: 0 }
  rec.count += 1
  if (rec.count >= AUTH_FAIL_LIMIT) {
    rec.lockedUntil = Date.now() + AUTH_LOCK_MS
    rec.count = 0
  }
  authFails.set(ip, rec)
}

// ---- 私网/保留地址判定（IPv4 字面量多形态 + IPv6 + DNS 解析结果）----
function ipv4ToInt(host) {
  // 支持 a.b.c.d / a.b.c / a.b / 单整数 / 0x hex / 0 八进制 各段
  const parts = host.split('.')
  if (parts.length > 4 || parts.length === 0) return null
  const nums = []
  for (const p of parts) {
    if (!p) return null
    let n
    if (/^0x[0-9a-f]+$/i.test(p)) n = parseInt(p, 16)
    else if (/^0[0-7]+$/.test(p) && p.length > 1) n = parseInt(p, 8)
    else if (/^\d+$/.test(p)) n = parseInt(p, 10)
    else return null
    if (Number.isNaN(n) || n < 0) return null
    nums.push(n)
  }
  // 末段允许扩展到 32 位，其余段必须 ≤255
  for (let i = 0; i < nums.length - 1; i++) if (nums[i] > 255) return null
  const maxLast = 2 ** (8 * (5 - nums.length)) - 1
  if (nums[nums.length - 1] > maxLast) return null
  let v = 0
  for (let i = 0; i < nums.length - 1; i++) v += nums[i] * 2 ** (8 * (3 - i))
  v += nums[nums.length - 1]
  return v >>> 0
}

const V4_BLOCKED = [
  [0x00000000, 0xff000000], // 0.0.0.0/8
  [0x0a000000, 0xff000000], // 10/8
  [0x7f000000, 0xff000000], // 127/8
  [0xa9fe0000, 0xffff0000], // 169.254/16
  [0xac100000, 0xfff00000], // 172.16/12
  [0xc0a80000, 0xffff0000], // 192.168/16
  [0x64400000, 0xffc00000], // 100.64/10 CGNAT
  [0xc0000000, 0xffffff00], // 192.0.0/24
  [0xc0000200, 0xffffff00], // 192.0.2/24 doc
  [0xc6120000, 0xfffe0000], // 198.18/15 benchmark
  [0xc6336400, 0xffffff00], // 198.51.100/24 doc
  [0xcb007100, 0xffffff00], // 203.0.113/24 doc
  [0xe0000000, 0xf0000000], // 224/4 multicast
  [0xf0000000, 0xf0000000], // 240/4 reserved
]

function isBlockedV4(int) {
  return V4_BLOCKED.some(([base, mask]) => (int & mask) === base)
}

function isBlockedV6(ip) {
  const h = ip.toLowerCase().replace(/^\[|\]$/g, '')
  if (h === '::' || h === '::1') return true
  if (h.startsWith('fe8') || h.startsWith('fe9') || h.startsWith('fea') || h.startsWith('feb')) return true // link-local fe80::/10
  if (h.startsWith('fc') || h.startsWith('fd')) return true // ULA
  // IPv4-mapped ::ffff:a.b.c.d（点分形式）
  if (h.startsWith('::ffff:')) {
    const tail = h.slice(7)
    const v4 = ipv4ToInt(tail)
    if (v4 !== null) return isBlockedV4(v4)
    // hex 形式 ::ffff:7f00:1 —— Node/OS 同样映射到 IPv4，必须按映射地址判定
    const hexParts = tail.split(':')
    if (hexParts.length === 2 && /^[0-9a-f]{1,4}$/.test(hexParts[0]) && /^[0-9a-f]{1,4}$/.test(hexParts[1])) {
      const v4hex = ((parseInt(hexParts[0], 16) << 16) | parseInt(hexParts[1], 16)) >>> 0
      return isBlockedV4(v4hex)
    }
  }
  // NAT64 64:ff9b::/96 与 6to4 2002::/16 内嵌 IPv4：按内嵌地址判定
  if (h.startsWith('64:ff9b::')) {
    const tail = h.slice('64:ff9b::'.length)
    const v4 = ipv4ToInt(tail)
    if (v4 !== null) return isBlockedV4(v4)
  }
  if (h.startsWith('2002:')) {
    const parts = h.split(':')
    if (parts.length >= 3 && /^[0-9a-f]{1,4}$/.test(parts[1]) && /^[0-9a-f]{1,4}$/.test(parts[2])) {
      const v4hex = ((parseInt(parts[1], 16) << 16) | parseInt(parts[2], 16)) >>> 0
      if (isBlockedV4(v4hex)) return true
    }
  }
  return false
}

function isBlockedIp(ip) {
  if (ip.includes(':')) return isBlockedV6(ip)
  const v4 = ipv4ToInt(ip)
  if (v4 !== null) return isBlockedV4(v4)
  return false
}

function validHostName(h) {
  if (!h || typeof h !== 'string' || h.length > MAX_NAME_LEN) return false
  const lower = h.toLowerCase().replace(/\.$/, '')
  if (lower === 'localhost' || lower.endsWith('.localhost')) return false
  if (lower === 'metadata' || lower.endsWith('.internal')) return false
  return /^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$/.test(lower)
}

// DNS 解析校验：域名解析到私网/保留地址同样拒绝（Render 容器有内网，worker.js 单字面量黑名单不够用）
async function resolvesToPublic(host) {
  const literalV4 = ipv4ToInt(host)
  if (literalV4 !== null) return !isBlockedV4(literalV4)
  if (host.includes(':')) return !isBlockedV6(host.replace(/^\[|\]$/g, ''))
  try {
    const results = await Promise.race([
      dns.lookup(host, { all: true }),
      new Promise((_, rej) => setTimeout(() => rej(new Error('dns timeout')), DNS_TIMEOUT_MS)),
    ])
    if (!results.length) return false
    return results.every((r) => !isBlockedIp(r.address))
  } catch {
    return false
  }
}

// ---- HTTP 入口：健康检查 + WS Upgrade ----
const server = http.createServer((req, res) => {
  if (req.url === '/healthz') {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ ok: true }))
    return
  }
  // 配置状态探针（对齐 worker.js /debug 先例）：只回 bool，不含任何机密
  if (req.url === '/debug') {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ set: true }))
    return
  }
  res.writeHead(404)
  res.end('not found')
})

// 免费层保活：Render 按入站 HTTP 判定活跃，对自身公网 /healthz 周期 GET 可防 15min 闲置休眠。
// 默认关闭，设 KEEPALIVE_URL=https://<app>.onrender.com 开启；建议再叠一层外部监控（UptimeRobot 等）。
const KEEPALIVE_URL = (process.env.KEEPALIVE_URL || '').trim().replace(/\/+$/, '')
if (KEEPALIVE_URL) {
  const KEEPALIVE_INTERVAL_MS = 10 * 60 * 1000
  const ping = () => {
    fetch(`${KEEPALIVE_URL}/healthz`)
      .then((r) => console.log(`[render-gate] keepalive ${r.status}`))
      .catch((e) => console.log(`[render-gate] keepalive failed: ${e.message || e}`))
  }
  setTimeout(ping, 30_000) // 启动后先自证一次
  setInterval(ping, KEEPALIVE_INTERVAL_MS).unref()
}

const wss = new WebSocketServer({ noServer: true })

server.on('upgrade', (req, socket, head) => {
  const url = new URL(req.url, 'http://localhost')
  if (url.pathname !== '/ws') {
    socket.destroy()
    return
  }
  const ip = clientIp(req)
  if (isLocked(ip)) {
    socket.write('HTTP/1.1 429 Too Many Requests\r\n\r\n')
    socket.destroy()
    return
  }
  const auth = req.headers.authorization || ''
  const presented = auth.startsWith('Bearer ') ? auth.slice(7) : ''
  if (!tokenOk(presented)) {
    recordAuthFail(ip)
    socket.write('HTTP/1.1 401 Unauthorized\r\n\r\n')
    socket.destroy()
    return
  }
  wss.handleUpgrade(req, socket, head, (ws) => handleTunnel(ws, req))
})

function handleTunnel(ws, req) {
  const ip = clientIp(req)
  let established = false
  let upstream = null
  let closed = false

  const firstFrameTimer = setTimeout(() => {
    if (!established) {
      safeSend(ws, JSON.stringify({ ok: false, reason: 'first-frame timeout' }))
      teardown(1008, 'first-frame timeout')
    }
  }, FIRST_FRAME_TIMEOUT_MS)

  function safeSend(w, data) {
    try { w.send(data) } catch {}
  }

  function teardown(code, reason) {
    if (closed) return
    closed = true
    clearTimeout(firstFrameTimer)
    try { ws.close(code, reason) } catch {}
    if (upstream) {
      try { upstream.destroy() } catch {}
      upstream = null
    }
  }

  ws.on('message', (data, isBinary) => {
    if (closed) return
    if (established) {
      if (isBinary && upstream && !upstream.destroyed) {
        // 背压：TCP 写缓冲满时暂停 WS 读取
        if (!upstream.write(data)) {
          ws.pause()
          upstream.once('drain', () => { try { ws.resume() } catch {} })
        }
      }
      return
    }
    if (isBinary) { teardown(1008, 'binary before bind'); return }

    let reqJson
    try { reqJson = JSON.parse(data.toString('utf8')) } catch {
      safeSend(ws, JSON.stringify({ ok: false, reason: 'bad first frame' }))
      teardown(1008, 'bad first frame')
      return
    }
    const host = String(reqJson.host || '').toLowerCase().replace(/\.$/, '')
    const port = Number(reqJson.port)
    if (!ALLOWED_PORTS.has(port) || !validHostName(host)) {
      safeSend(ws, JSON.stringify({ ok: false, reason: 'acl denied' }))
      teardown(1008, 'acl denied')
      return
    }

    const pubCheck = ALLOW_PRIVATE ? Promise.resolve(true) : resolvesToPublic(host)
    pubCheck.then((pub) => {
      if (closed) return
      if (!pub) {
        safeSend(ws, JSON.stringify({ ok: false, reason: 'acl denied' }))
        teardown(1008, 'acl denied')
        return
      }
      console.log('[render-gate] connecting', host, port, 'client', ip)
      const sock = net.connect({ host, port })
      sock.setNoDelay(true)
      const dialTimer = setTimeout(() => {
        sock.destroy(new Error('connect timeout'))
      }, CONNECT_TIMEOUT_MS)

      sock.once('connect', () => {
        clearTimeout(dialTimer)
        established = true
        upstream = sock
        clearTimeout(firstFrameTimer)
        safeSend(ws, JSON.stringify({ ok: true }))
        sock.on('data', (chunk) => {
          // TCP → WS：ws.send 无返回背压信号，以 bufferedAmount 阈值暂停/恢复 TCP 读
          if (ws.readyState !== ws.OPEN) return
          ws.send(chunk, { binary: true })
          if (ws.bufferedAmount > 4 * 1024 * 1024) {
            sock.pause()
            const iv = setInterval(() => {
              if (ws.readyState !== ws.OPEN || ws.bufferedAmount < 1024 * 1024) {
                clearInterval(iv)
                if (!sock.destroyed) sock.resume()
              }
            }, 50)
          }
        })
        sock.on('close', () => teardown(1000, 'upstream closed'))
        sock.on('error', () => teardown(1011, 'upstream error'))
      })
      sock.once('error', (e) => {
        clearTimeout(dialTimer)
        if (!established) {
          safeSend(ws, JSON.stringify({ ok: false, reason: String(e.message || e) }))
          teardown(1011, 'connect failed')
        }
      })
    })
  })

  ws.on('close', () => teardown(1000, 'client closed'))
  ws.on('error', () => teardown(1011, 'client error'))
  ws.on('pong', () => {})
}

server.listen(PORT, '0.0.0.0', () => {
  console.log(`[render-gate] listening on 0.0.0.0:${PORT}, token hash configured: ${TOKEN_HASH.slice(0, 8)}…`)
})
