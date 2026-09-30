// egress-fallback 单测（P0-1 多出口兜底）：
// node deploy/cf-gate-worker/egress-fallback.test.mjs
// 覆盖：配置解析 / 通道编排与合规门禁 / SOCKS5 端到端（真实本地 SOCKS5 服务器 + 回显目标）/
// 超时 / 握手残字节（leftover）。
import net from 'node:net'
import { Readable, Writable } from 'node:stream'
import {
  parseEndpoint,
  parseFallbackConfig,
  fallbackAllowed,
  channelOrder,
  withTimeout,
  socks5Connect,
  DEFAULT_ATTEMPT_TIMEOUT_MS,
} from './egress-fallback.mjs'
import { shouldBlockEgress, requiresCompliantEgress } from './gate-policy.mjs'

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) {
    passed++
    console.log(`  [pass] ${name}`)
  } else {
    failed++
    console.log(`  [fail] ${name} ${extra}`)
  }
}

console.log('[1] parseEndpoint（host[:port] / [v6]:port）')
check('host:port 解析', JSON.stringify(parseEndpoint('vps.example.com:1080')) === JSON.stringify({ host: 'vps.example.com', port: 1080 }))
check('纯 host 用默认端口', JSON.stringify(parseEndpoint('relay.example.com')) === JSON.stringify({ host: 'relay.example.com', port: 443 }))
check('默认端口可覆盖', parseEndpoint('relay.example.com', 8443).port === 8443)
check('IPv6 方括号', JSON.stringify(parseEndpoint('[2001:db8::1]:1080')) === JSON.stringify({ host: '2001:db8::1', port: 1080 }))
check('IPv6 无端口', parseEndpoint('[2001:db8::1]').host === '2001:db8::1')
check('空串 → null', parseEndpoint('') === null)
check('空白 → null', parseEndpoint('   ') === null)
check('非法端口 → null', parseEndpoint('vps.example.com:0') === null)
check('端口越界 → null', parseEndpoint('vps.example.com:70000') === null)
check('非数字端口 → null', parseEndpoint('vps.example.com:abc') === null)
check('空 host → null', parseEndpoint(':1080') === null)

console.log('[2] parseFallbackConfig（默认全空 = 仅直连）')
const empty = parseFallbackConfig({})
check('默认 socks5 关闭', empty.socks5 === null)
check('默认 proxyip 关闭', empty.proxyIp === null)
check('默认无声明国家码', empty.socks5Country === null && empty.proxyIpCountry === null)
check('默认超时 8000', empty.attemptTimeoutMs === DEFAULT_ATTEMPT_TIMEOUT_MS)
const full = parseFallbackConfig({
  SOCKS5_PROXY: 'vps.example.com:1080',
  SOCKS5_COUNTRY: 'us',
  PROXYIP_HOST: 'relay.example.com',
  PROXYIP_COUNTRY: 'US',
  EGRESS_ATTEMPT_TIMEOUT_MS: '5000',
})
check('socks5 配置生效', full.socks5.host === 'vps.example.com' && full.socks5.port === 1080)
check('国家码大写归一', full.socks5Country === 'US' && full.proxyIpCountry === 'US')
check('proxyip 配置生效', full.proxyIp.host === 'relay.example.com' && full.proxyIp.port === 443)
check('超时覆盖生效', full.attemptTimeoutMs === 5000)
check('env 缺失对象容错', parseFallbackConfig(undefined).socks5 === null)

console.log('[3] 合规门禁（fallbackAllowed / channelOrder）')
const g = { allowedCountries: ['US', 'JP', 'SG'] }
check('非合规 host 兜底自由（无国家码）', fallbackAllowed('github.com', null, g) === true)
check('非合规 host 兜底自由（含国家码）', fallbackAllowed('openai.com', 'US', g) === true)
check('合规 host 未声明国家码 → fail-closed', fallbackAllowed('daily-cloudcode-pa.googleapis.com', null, g) === false)
check('合规 host 声明合规国家 → 放行', fallbackAllowed('daily-cloudcode-pa.googleapis.com', 'US', g) === true)
check('合规 host 声明非合规国家 → 拒绝', fallbackAllowed('daily-cloudcode-pa.googleapis.com', 'HK', g) === false)
check('requiresCompliantEgress 判定一致', requiresCompliantEgress('daily-cloudcode-pa.googleapis.com') === true)
check('非合规 host 不触发 geo 门禁', requiresCompliantEgress('github.com') === false)
check('shouldBlockEgress 声明国家码语义', shouldBlockEgress({ country: 'US', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com', { allowedCountries: ['US'] }) === null)
check('shouldBlockEgress 拒绝非白名单', shouldBlockEgress({ country: 'HK', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com', { allowedCountries: ['US'] }) === 'unsupported_egress:HK')

const cfgBoth = parseFallbackConfig({
  SOCKS5_PROXY: 'vps.example.com:1080',
  PROXYIP_HOST: 'relay.example.com',
})
check('通道顺序：直连 → socks5 → proxyip', channelOrder('github.com', cfgBoth).join(',') === 'direct,socks5,proxyip')
check('仅 socks5 配置', channelOrder('github.com', parseFallbackConfig({ SOCKS5_PROXY: 'vps.example.com:1080' })).join(',') === 'direct,socks5')
check('仅 proxyip 配置', channelOrder('github.com', parseFallbackConfig({ PROXYIP_HOST: 'relay.example.com' })).join(',') === 'direct,proxyip')
check('无配置仅直连', channelOrder('github.com', parseFallbackConfig({})).join(',') === 'direct')
check('合规 host 无国家码 → 仅直连', channelOrder('daily-cloudcode-pa.googleapis.com', cfgBoth).join(',') === 'direct')
check('合规 host 声明合规 → 含兜底', channelOrder('daily-cloudcode-pa.googleapis.com', parseFallbackConfig({ SOCKS5_PROXY: 'vps.example.com:1080', SOCKS5_COUNTRY: 'US' })).join(',') === 'direct,socks5')

console.log('[4] withTimeout')
check('正常 promise 通过', (await withTimeout(Promise.resolve(42), 50)) === 42)
check('rejection 透传', (await withTimeout(Promise.reject(new Error('boom')), 50).then(() => false, (e) => e.message === 'boom')) === true)
{
  const never = new Promise(() => {})
  const t0 = Date.now()
  const timedOut = await withTimeout(never, 60, 'test-op').then(() => false, (e) => String(e).includes('timed out after 60ms'))
  check('超时抛错', timedOut === true, `got: ${timedOut}`)
  check('超时耗时≈60ms', Date.now() - t0 >= 55, `elapsed=${Date.now() - t0}`)
}

console.log('[5] SOCKS5 端到端（真实本地 SOCKS5 服务器 + 回显目标）')
function startEchoServer() {
  return new Promise((resolve) => {
    const srv = net.createServer((c) => c.pipe(c))
    srv.listen(0, '127.0.0.1', () => resolve({ port: srv.address().port, close: () => srv.close() }))
  })
}

/// 极简 SOCKS5 服务器：greeting no-auth → CONNECT(domain) → 转发到目标。
/// @param {(targetHost: string, targetPort: number) => net.Socket} dial 测试注入目标连接
function startSocks5Server(dial, replyRep = 0x00) {
  return new Promise((resolve) => {
    const srv = net.createServer((conn) => {
      let stage = 'greet'
      conn.on('data', (buf) => {
        if (stage === 'greet') {
          if (buf[0] !== 0x05) { conn.destroy(); return }
          conn.write(Buffer.from([0x05, 0x00]))
          stage = 'connect'
          return
        }
        if (stage === 'connect') {
          if (buf[0] !== 0x05 || buf[1] !== 0x01) { conn.destroy(); return }
          const atyp = buf[3]
          if (atyp !== 0x03) { conn.destroy(); return }
          const len = buf[4]
          const host = buf.subarray(5, 5 + len).toString('utf8')
          const port = buf.readUInt16BE(5 + len)
          stage = 'done'
          const target = dial(host, port)
          target.on('connect', () => {
            conn.write(Buffer.from([0x05, replyRep, 0x00, 0x01, 127, 0, 0, 1, 0, 0]))
            target.pipe(conn)
            conn.pipe(target)
          })
          target.on('error', () => { try { conn.destroy() } catch {} })
          return
        }
      })
    })
    srv.listen(0, '127.0.0.1', () => resolve({ port: srv.address().port, close: () => srv.close() }))
  })
}

function nodeSocketLike(s) {
  return {
    readable: Readable.toWeb(s),
    writable: Writable.toWeb(s),
  }
}

{
  const echo = await startEchoServer()
  const socks = await startSocks5Server((host, port) => net.connect(port, host))
  const raw = net.connect(socks.port, '127.0.0.1')
  const sock = nodeSocketLike(raw)
  const writer = sock.writable.getWriter()
  try {
    const { leftover } = await socks5Connect(sock, writer, '127.0.0.1', echo.port, 3000)
    check('握手成功且无残字节', leftover === null, leftover ? `leftover len=${leftover.length}` : '')
    const reader = sock.readable.getReader()
    await writer.write(new TextEncoder().encode('hello-fallback'))
    const { value } = await withTimeout(reader.read(), 3000, 'echo read')
    const text = new TextDecoder().decode(value)
    check('经 SOCKS5 中继回显', text === 'hello-fallback', `got: ${text}`)
    reader.releaseLock()
  } finally {
    raw.destroy()
    socks.close()
    echo.close()
  }
}

{
  // REP 非 0（连接拒绝）→ 握手必须抛错（服务器直接回 REP=5，不连目标）
  const srv = await new Promise((resolve) => {
    const server = net.createServer((conn) => {
      let stage = 'greet'
      conn.on('data', (buf) => {
        if (stage === 'greet') {
          conn.write(Buffer.from([0x05, 0x00]))
          stage = 'connect'
          return
        }
        if (stage === 'connect') {
          conn.write(Buffer.from([0x05, 0x05, 0x00, 0x01, 127, 0, 0, 1, 0, 0]))
          stage = 'done'
        }
      })
    })
    server.listen(0, '127.0.0.1', () => resolve({ port: server.address().port, close: () => server.close() }))
  })
  const raw = net.connect(srv.port, '127.0.0.1')
  const sock = nodeSocketLike(raw)
  const writer = sock.writable.getWriter()
  try {
    const err = await socks5Connect(sock, writer, 'target.example.com', 443, 3000).then(
      () => null,
      (e) => String(e),
    )
    check('REP!=0 抛错', err !== null && err.includes('REP=5'), `err=${err}`)
  } finally {
    raw.destroy()
    srv.close()
  }
}

{
  // leftover：服务器在 CONNECT 回复后立即多发 3 字节，必须完整取回
  const echo = await startEchoServer()
  const srv = await new Promise((resolve) => {
    const server = net.createServer((conn) => {
      let stage = 'greet'
      conn.on('data', (buf) => {
        if (stage === 'greet') {
          conn.write(Buffer.from([0x05, 0x00]))
          stage = 'connect'
          return
        }
        if (stage === 'connect') {
          const len = buf[4]
          const host = buf.subarray(5, 5 + len).toString('utf8')
          const port = buf.readUInt16BE(5 + len)
          const target = net.connect(port, host)
          target.on('connect', () => {
            conn.write(Buffer.from([0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0, 1, 2, 3]))
            target.pipe(conn)
            conn.pipe(target)
          })
          stage = 'done'
        }
      })
    })
    server.listen(0, '127.0.0.1', () => resolve({ port: server.address().port, close: () => server.close() }))
  })
  const raw = net.connect(srv.port, '127.0.0.1')
  const sock = nodeSocketLike(raw)
  const writer = sock.writable.getWriter()
  try {
    const { leftover } = await socks5Connect(sock, writer, '127.0.0.1', echo.port, 3000)
    check('残字节完整取回', leftover && leftover.length === 3 && leftover[2] === 3, `leftover=${leftover}`)
  } finally {
    raw.destroy()
    srv.close()
    echo.close()
  }
}

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)
