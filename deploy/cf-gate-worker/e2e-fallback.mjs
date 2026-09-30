// P0-1 端到端验收（非零退出）：本地起回显目标 + 固定目标 SOCKS5 中继，
// 经 wrangler dev 跑 gate worker，客户端请求一个直连必败的 host（DNS 无法解析），
// 验证 worker 自动切 SOCKS5 兜底后数据仍可达（收到 {ok:true, via:"socks5"} 且回显）。
// 注：本地端到端验证的是兜底**机制**；真实出海可达性靠部署后 review。
//
// 用法：node deploy/cf-gate-worker/e2e-fallback.mjs
// 前置：已安装 wrangler（WRANGLER_BIN 或 PATH 可寻）、ws 包；端口 8791 空闲。
// 对抗审核 P2：运行前自动清理上次残留的 wrangler/workerd 孤儿进程，可重复执行。
import net from 'node:net'
import { spawn, execSync } from 'node:child_process'
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
  // 对抗审核 P2：先清理上次运行残留的 wrangler/workerd 孤儿进程（占 8791 端口
  // 会导致第二次运行必败 Address already in use）
  try { execSync(`fuser -k ${PORT}/tcp 2>/dev/null; pkill -f 'wrangler.*--port ${PORT}' 2>/dev/null; true`) } catch {}
  await new Promise((r) => setTimeout(r, 300))

  echo = await startEcho()
  socks = await startFixedSocks5('127.0.0.1', echo.port)
  console.log(`echo=127.0.0.1:${echo.port} socks5=127.0.0.1:${socks.port}`)

  // 对抗审核 P2：wrangler 定位（不硬编码机器专属路径）——候选：
  // WRANGLER_BIN env → PATH 里的 wrangler → 常见 npm-global 安装路径
  let wranglerBin = process.env.WRANGLER_BIN || ''
  if (!wranglerBin) {
    const candidates = [
      '/home/dm/.npm-global/bin/wrangler', // 本机实测路径（保留）
      '/usr/local/bin/wrangler',
      '/usr/bin/wrangler',
      process.env.HOME ? `${process.env.HOME}/.npm-global/bin/wrangler` : '',
      process.env.HOME ? `${process.env.HOME}/.local/bin/wrangler` : '',
    ].filter(Boolean)
    wranglerBin = candidates.find((p) => {
      try { fs.accessSync(p); return true } catch { return false }
    }) || ''
  }
  if (!wranglerBin) {
    console.error('E2E FAIL: 找不到 wrangler（设置 WRANGLER_BIN 指向 wrangler 可执行文件）')
    process.exit(1)
  }
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
  // 对抗审核 P2：必须终止整个进程组（wrangler spawn 的 workerd 子进程不是
  // 直接子进程，kill 父进程会让它成为孤儿继续占 8791）
  try { wrangler?.kill('SIGKILL') } catch {}
  try {
    if (wrangler?.pid) execSync(`pkill -9 -P ${wrangler.pid} 2>/dev/null; true`)
  } catch {}
  try { echo?.close() } catch {}
  try { socks?.close() } catch {}
}
