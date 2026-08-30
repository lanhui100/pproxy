// gate 冒烟测试：验证 WS↔TCP 网关端到端可用（CF worker 与 Node/Vercel 通用）。
//
// 用法：
//   node smoke-test.mjs <ws-url> <host> [port] [--token <plaintext>] [--token-file <path>]
//
// 流程：WS Upgrade（Bearer）→ 首帧 {"host","port"} → 等 {"ok":true} →
// 在隧道之上做真实 TLS 握手（验证双向透传），成功即 exit 0。
// 被拒（{"ok":false}）时打印 reason 后 exit 2（CF 平台限制等场景）。
import WebSocket from 'ws'
import tls from 'node:tls'
import fs from 'node:fs'
import { Duplex } from 'node:stream'

const args = process.argv.slice(2)
const url = args[0]
const host = args[1]
const port = Number(args[2] || 443)

let tokenFlag = null
for (let i = 3; i < args.length; i++) {
  if (args[i] === '--token') tokenFlag = args[++i]
  if (args[i] === '--token-file') tokenFlag = fs.readFileSync(args[++i], 'utf8').trim()
}
if (!url || !host || !tokenFlag) {
  console.error('usage: node smoke-test.mjs <ws-url> <host> [port] (--token <t> | --token-file <f>)')
  process.exit(64)
}

const fail = (msg) => { console.error(`FAIL: ${msg}`); process.exit(1) }
const timer = setTimeout(() => fail(`timeout after 15s (${host}:${port})`), 15000)

const ws = new WebSocket(url, {
  headers: { Authorization: `Bearer ${tokenFlag}` },
  handshakeTimeout: 10000,
})

ws.on('unexpected-response', (_req, res) => {
  clearTimeout(timer)
  console.error(`FAIL: HTTP ${res.statusCode} at upgrade`)
  process.exit(1)
})
ws.on('error', (e) => { clearTimeout(timer); fail(String(e)) })

let tlsSock = null
let bridge = null

ws.on('open', () => {
  ws.send(JSON.stringify({ host, port }))
})

ws.on('message', (data, isBinary) => {
  if (!isBinary) {
    let v
    try { v = JSON.parse(data.toString()) } catch { return }
    if (v.ok === false) {
      clearTimeout(timer)
      console.error(`DENIED (${host}:${port}): ${v.reason ?? '?'}`)
      process.exit(2)
    }
    if (v.ok === true) startTls()
    return
  }
  // 隧道数据 → 喂给 TLS 层
  bridge?.push(data)
})

ws.on('close', () => { clearTimeout(timer); if (!tlsSock) fail('ws closed before tls established'); })

function startTls() {
  bridge = new Duplex({
    write(chunk, _enc, cb) { ws.send(chunk); cb() },
    read() {},
  })
  tlsSock = tls.connect({ socket: bridge, servername: host }, () => {
    clearTimeout(timer)
    const cipher = tlsSock.getCipher()?.name ?? '?'
    console.log(`OK: TLS established via ${url} to ${host}:${port} (cipher ${cipher})`)
    tlsSock.end()
    ws.close()
    process.exit(0)
  })
  tlsSock.once('error', (e) => { clearTimeout(timer); fail(`tls: ${e.message}`) })
}
