// P0-1 端到端验收（非零退出）：本地起回显目标 + 固定目标 SOCKS5 中继，
// 经 wrangler dev 跑 gate worker，客户端请求一个直连必败的 host（DNS 无法解析），
// 验证 worker 自动切 SOCKS5 兜底仍可出海（收到 {ok:true, via:"socks5"} 且数据回显）。
//
// 用法：node deploy/cf-gate-worker/e2e-fallback.mjs
// 前置：已安装 wrangler（npx 可用）、ws 包；端口 8791 空闲。
import net from 'node:net'
import { spawn } from 'node:child_process'
import crypto from 'node:crypto'
import fs from 'node:fs'
import { once } from 'node:events'
import WebSocket from 'ws'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const workerDir = path.dirname(fileURLToPath(import.meta.url))

const PORT = 8791
const TOKEN = 'e2e-fallback-token'
const HASH = crypto.createHash('sha256').update(TOKEN).digest('hex')

function startEcho() {
  return new Promise((resolve) => {
    const srv = net.createServer((c) => c.pipe(c))
    srv.listen(0, '127.0.0.1', () => resolve({ port: srv.address().port, close: () => srv.close() }))
  })
}

/// 固定目标 SOCKS5 中继：任何 CONNECT 一律转发到 (fixedHost, fixedPort)——
/// 模拟"用户 VPS 反代"在直连被墙时仍可达真实目标；CONNECT 请求里的 host 被忽略。
function startFixedSocks5(fixedHost, fixedPort) {
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
          const target = net.connect(fixedPort, fixedHost)
          target.on('connect', () => {
            conn.write(Buffer.from([0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0]))
            target.pipe(conn)
            conn.pipe(target)
          })
          target.on('error', () => { try { conn.destroy() } catch {} })
          stage = 'done'
        }
      })
    })
    srv.listen(0, '127.0.0.1', () => resolve({ port: srv.address().port, close: () => srv.close() }))
  })
}

function waitForReady(proc, log) {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error('wrangler dev 启动超时')), 60000)
    proc.stdout.on('data', (d) => {
      log(d.toString())
      if (d.toString().includes('Ready on')) { clearTimeout(t); resolve() }
    })
    proc.stderr.on('data', (d) => log(d.toString()))
    proc.on('exit', (code) => { clearTimeout(t); reject(new Error(`wrangler dev 提前退出 code=${code}`)) })
  })
}

let wrangler = null
let echo = null
let socks = null
try {
  echo = await startEcho()
  socks = await startFixedSocks5('127.0.0.1', echo.port)
  console.log(`echo=127.0.0.1:${echo.port} socks5=127.0.0.1:${socks.port}`)

  const wranglerBin = process.env.WRANGLER_BIN || '/home/dm/.npm-global/bin/wrangler'
  const wranglerReal = fs.realpathSync(wranglerBin)
  const isNodeScript = wranglerReal.endsWith('.js')
  const spawnArgs = ['dev', '--local', '--port', String(PORT),
    '--var', `TUNNEL_TOKEN_HASH:${HASH}`,
    '--var', `SOCKS5_PROXY:127.0.0.1:${socks.port}`,
    '--var', 'EGRESS_ATTEMPT_TIMEOUT_MS:2000']
  wrangler = isNodeScript
    ? spawn(process.execPath, [wranglerReal, ...spawnArgs], { cwd: workerDir, stdio: ['ignore', 'pipe', 'pipe'] })
    : spawn(wranglerBin, spawnArgs, { cwd: workerDir, stdio: ['ignore', 'pipe', 'pipe'] })

  await waitForReady(wrangler, () => {})

  const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws`, { headers: { Authorization: `Bearer ${TOKEN}` } })
  await once(ws, 'open')
  const firstFrame = new Promise((res, rej) => {
    const t = setTimeout(() => rej(new Error('等待 ok 帧超时')), 10000)
    ws.once('message', (d) => { clearTimeout(t); res(JSON.parse(d.toString())) })
    ws.once('error', rej)
  })
  ws.send(JSON.stringify({ host: 'direct-blocked.invalid', port: 443 }))
  const reply = await firstFrame

  if (reply.ok !== true || reply.via !== 'socks5') {
    console.error('FAIL: 期望 {ok:true, via:"socks5"}，实际', JSON.stringify(reply))
    process.exit(1)
  }
  console.log('PASS: fallback via=', reply.via)

  // 数据面回显验证：经 SOCKS5 中继往返（客户端协议 = 建立后全走二进制帧）
  ws.send(Buffer.from('ping-through-fallback'))
  const echoed = await new Promise((res, rej) => {
    const t = setTimeout(() => rej(new Error('等待回显超时')), 10000)
    ws.once('message', (d) => { clearTimeout(t); res(d.toString()) })
    ws.once('error', rej)
  })
  if (echoed !== 'ping-through-fallback') {
    console.error('FAIL: 数据未回显', echoed)
    process.exit(1)
  }
  console.log('PASS: 数据经 SOCKS5 兜底回显')
  ws.close()
  console.log('E2E PASS')
  process.exit(0)
} catch (e) {
  console.error('E2E FAIL', e)
  process.exit(1)
} finally {
  try { wrangler?.kill() } catch {}
  try { echo?.close() } catch {}
  try { socks?.close() } catch {}
}
