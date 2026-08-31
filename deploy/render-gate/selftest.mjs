// render-gate 自检：起两个 server.js 子进程——
//   A（默认防线）：鉴权 / ACL / 私网变体 / 失败锁定
//   B（测试注入 RENDER_GATE_ALLOW_PRIVATE=1 + EXTRA_PORT）：透传往返 / Ping-Pong / 并发
// 运行：npm install && npm run selftest
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import net from 'node:net'
import WebSocket from 'ws'

const TOKEN = 'selftest-token-123'
const TOKEN_HASH = createHash('sha256').update(TOKEN, 'utf8').digest('hex')
const PORT_A = 38591
const PORT_B = 38592
const BASE_A = `http://127.0.0.1:${PORT_A}`

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) { passed++; console.log(`  [pass] ${name}`) }
  else { failed++; console.log(`  [fail] ${name} ${extra}`) }
}

// loopback 回显 TCP 目标（仅 B 实例可连，A 实例 ACL 必须拒绝）；计数跟踪用于半关断言
let echoConns = 0
const echoServer = net.createServer((sock) => {
  echoConns++
  sock.on('close', () => { echoConns-- })
  sock.pipe(sock)
})
await new Promise((r) => echoServer.listen(0, '127.0.0.1', r))
const ECHO_PORT = echoServer.address().port

function startServer(port, extraEnv = {}) {
  const child = spawn(process.execPath, ['server.js'], {
    env: { ...process.env, PORT: String(port), TUNNEL_TOKEN_HASH: TOKEN_HASH, ...extraEnv },
    stdio: 'ignore',
  })
  return child
}

const childA = startServer(PORT_A)
const childB = startServer(PORT_B, {
  RENDER_GATE_ALLOW_PRIVATE: '1',
  RENDER_GATE_EXTRA_PORT: String(ECHO_PORT),
})
await new Promise((r) => setTimeout(r, 900))

function connectWs(port, token) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`, {
      headers: token ? { authorization: `Bearer ${token}` } : {},
    })
    ws.once('open', () => resolve(ws))
    ws.once('error', reject)
    ws.once('unexpected-response', (_req, res) => reject(new Error(`http ${res.statusCode}`)))
  })
}

function bindTarget(ws, host, port, timeoutMs = 4000) {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve({ ok: false, reason: 'test timeout' }), timeoutMs)
    ws.once('message', (data, isBinary) => {
      clearTimeout(timer)
      if (isBinary) return resolve({ ok: false, reason: 'unexpected binary' })
      try { resolve(JSON.parse(data.toString())) } catch { resolve({ ok: false, reason: 'bad json' }) }
    })
    ws.send(JSON.stringify({ host, port }))
  })
}

try {
  console.log('[1] HTTP 面')
  const hz = await fetch(`${BASE_A}/healthz`)
  check('healthz 200', hz.status === 200)
  const dbg = await fetch(`${BASE_A}/debug`)
  check('debug {set:true}', dbg.status === 200 && (await dbg.json()).set === true)

  console.log('[2] 鉴权')
  await connectWs(PORT_A, 'wrong-token').then(
    (ws) => { check('错误 token 被拒', false); ws.close() },
    (e) => check('错误 token 被拒（401）', String(e.message).includes('401'), e.message),
  )
  await connectWs(PORT_A, null).then(
    (ws) => { check('无 token 被拒', false); ws.close() },
    (e) => check('无 token 被拒（401）', String(e.message).includes('401'), e.message),
  )

  console.log('[3] ACL（实例 A，默认防线）')
  const aclCases = [
    ['非 443 端口', 'example.com', 8443],
    ['IPv4 回环字面量', '127.0.0.1', ECHO_PORT],
    ['IPv4 十进制变体', '2130706433', ECHO_PORT],
    ['IPv4 hex 变体', '0x7f000001', ECHO_PORT],
    ['IPv4 八进制变体', '017700000001', ECHO_PORT],
    ['link-local metadata', '169.254.169.254', 443],
    ['localhost', 'localhost', ECHO_PORT],
    ['内网 192.168', '192.168.1.1', 443],
    ['CGNAT 100.64', '100.64.0.1', 443],
    ['IPv6 回环', '[::1]', 443],
    ['IPv4-mapped IPv6 点分', '::ffff:127.0.0.1', 443],
    ['IPv4-mapped IPv6 hex 形式', '::ffff:7f00:1', 443],
    ['NAT64 内嵌回环', '64:ff9b::127.0.0.1', 443],
    ['6to4 内嵌回环', '2002:7f00:0001::', 443],
    ['ULA fc00', 'fd00::1', 443],
    ['IPv6 link-local', 'fe80::1', 443],
  ]
  for (const [name, host, port] of aclCases) {
    const ws = await connectWs(PORT_A, TOKEN)
    const r = await bindTarget(ws, host, port)
    check(`${name} 拒绝`, r.ok === false, JSON.stringify(r))
    ws.close()
  }
  // DNS 解析到私网的域名也应被拒（解析结果校验）
  {
    const ws = await connectWs(PORT_A, TOKEN)
    const r = await bindTarget(ws, 'ip6-localhost', ECHO_PORT).catch(() => ({ ok: false }))
    check('解析到回环的域名拒绝（best-effort）', r.ok === false, JSON.stringify(r))
    ws.close()
  }

  console.log('[4] 透传（实例 B，测试注入放行 loopback）')
  {
    const ws = await connectWs(PORT_B, TOKEN)
    const r = await bindTarget(ws, '127.0.0.1', ECHO_PORT)
    check('bind ok:true', r.ok === true, JSON.stringify(r))
    const payload = Buffer.from('ping-payload-42')
    ws.send(payload)
    const echo = await new Promise((resolve) => {
      ws.once('message', (data, isBinary) => resolve({ data, isBinary }))
      setTimeout(() => resolve(null), 3000)
    })
    check('二进制回显往返', !!echo && echo.isBinary && echo.data.toString() === 'ping-payload-42')
    // 半关：关闭 WS 后 render-gate 必须释放上游 TCP（echo 侧连接计数回落）
    const before = echoConns
    ws.close()
    await new Promise((r2) => setTimeout(r2, 500))
    check('WS 关闭后上游 TCP 被释放', echoConns === before - 1, `before=${before} now=${echoConns}`)
  }

  console.log('[5] Ping/Pong（ws 库自动应答）')
  {
    const ws = await connectWs(PORT_B, TOKEN)
    const pong = await new Promise((resolve) => {
      ws.once('pong', () => resolve(true))
      ws.ping()
      setTimeout(() => resolve(false), 2000)
    })
    check('ping 自动 pong', pong)
    ws.close()
  }

  console.log('[6] 并发 10 连接（实例 B 各自独立透传）')
  {
    const conns = await Promise.all(Array.from({ length: 10 }, () => connectWs(PORT_B, TOKEN)))
    const binds = await Promise.all(conns.map((ws) => bindTarget(ws, '127.0.0.1', ECHO_PORT)))
    check('10 并发全部 bind ok', binds.every((r) => r.ok === true), JSON.stringify(binds.filter((r) => !r.ok)))
    const echoes = await Promise.all(conns.map((ws, i) => new Promise((resolve) => {
      ws.once('message', (data) => resolve(data.toString()))
      ws.send(Buffer.from(`msg-${i}`))
      setTimeout(() => resolve(null), 3000)
    })))
    check('10 并发回显互不串扰', echoes.every((e, i) => e === `msg-${i}`), JSON.stringify(echoes))
    conns.forEach((ws) => ws.close())
  }

  console.log('[7] 鉴权失败锁定（实例 A，5 次失败 → 429）')
  {
    for (let i = 0; i < 5; i++) await connectWs(PORT_A, 'bad').catch(() => {})
    const locked = await connectWs(PORT_A, 'bad').then(
      () => 'not-locked',
      (e) => (String(e.message).includes('429') ? 'locked' : e.message),
    )
    check('第 6 次失败返回 429', locked === 'locked', locked)
  }
} finally {
  childA.kill()
  childB.kill()
  echoServer.close()
}

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)
