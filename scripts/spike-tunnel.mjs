#!/usr/bin/env node
// spike-tunnel.mjs — M6 P0 S1/S2：经 gate worker 隧道拉大文件，测吞吐/CPU/并发
// 用法：node scripts/spike-tunnel.mjs <ws-url> <token> [file-path] [--concurrency N]
// 指标：ciphertext 总量（隧道吞吐）与明文总量（TLS 解密后），平台负载评估以前者为准。
import WebSocket from 'ws'
import tls from 'node:tls'
import { Duplex } from 'node:stream'

const args = process.argv.slice(2)
const wsUrl = args[0]
const token = args[1]
const filePath = args[2] ?? '/files/100Mb.dat'
const concIdx = args.indexOf('--concurrency')
const concurrency = concIdx > -1 ? Number(args[concIdx + 1]) : 1

if (!wsUrl || !token) {
  console.error('usage: spike-tunnel.mjs <ws-url> <token> [file-path]')
  process.exit(2)
}
const TARGET_HOST = 'proof.ovh.net' // OVH 网络，非 Cloudflare 托管（sockets 策略允许直连）

/** 把 WS 会话桥接成 Node duplex socket，供 tls.connect 复用（端到端 TLS，无 MITM）。 */
function bridge(ws) {
  let pushed = 0
  const sock = new Duplex({
    read() {},
    write(chunk, _enc, cb) {
      if (ws.readyState === WebSocket.OPEN) ws.send(chunk)
      cb()
    },
  })
  return {
    sock,
    /** 入向密文入口：由 ws message 处理器调用 */
    pushCipher(data) {
      pushed += data.length
      sock.push(Buffer.from(data))
    },
    pushed: () => pushed,
  }
}

function one(id, downloadMs) {
  return new Promise((resolve) => {
    const t0 = Date.now()
    const result = { id, ok: false, phase: 'connect', cipherIn: 0, plainMB: 0, ms: 0, error: '' }
    const ws = new WebSocket(wsUrl, { headers: { Authorization: `Bearer ${token}` } })
    const kill = setTimeout(() => finish(new Error(`timeout ${downloadMs}ms`)), downloadMs)
    let bridgeRef = null

    function finish(err) {
      clearTimeout(kill)
      result.ms = Date.now() - t0
      result.cipherIn = bridgeRef ? bridgeRef.pushed() : 0
      if (err) result.error = String(err.message ?? err)
      else result.ok = true
      try { ws.close() } catch {}
      resolve(result)
    }

    ws.on('error', (e) => finish(e))
    ws.on('message', (data, isBinary) => {
      if (isBinary) {
        bridgeRef?.pushCipher(data)
        return
      }
      const msg = JSON.parse(data.toString())
      if (!msg.ok) return finish(new Error(`tunnel denied: ${msg.reason ?? ''}`))

      // 隧道建立 → 在透传流上做 TLS 握手 + HTTP GET（证书校验自然发生=完整性验证）
      result.phase = 'tls'
      const br = bridge(ws)
      bridgeRef = br
      const tlsSock = tls.connect({ socket: br.sock, servername: TARGET_HOST }, () => {
        result.phase = 'http'
        tlsSock.write(
          `GET ${filePath} HTTP/1.1\r\nHost: ${TARGET_HOST}\r\nUser-Agent: spike/1.0\r\nConnection: close\r\n\r\n`,
        )
      })
      tlsSock.on('error', (e) => finish(e))
      let headerDone = false
      tlsSock.on('data', (chunk) => {
        if (!headerDone) {
          const idx = chunk.indexOf('\r\n\r\n')
          if (idx > -1) {
            headerDone = true
            const status = Number(chunk.slice(9, 12))
            if (status !== 200) return finish(new Error(`HTTP ${status} over tunnel`))
            result.plainMB += (chunk.length - idx - 4) / 1048576
          }
          return
        }
        result.plainMB += chunk.length / 1048576
      })
      tlsSock.on('close', () => finish(null))
    })

    ws.on('open', () => {
      ws.send(JSON.stringify({ host: TARGET_HOST, port: 443 }))
    })
  })
}

console.log(`spike: url=${wsUrl} target=${TARGET_HOST}:443 file=${filePath} concurrency=${concurrency}`)
const results = await Promise.all(Array.from({ length: concurrency }, (_, i) => one(i, 60_000)))
for (const r of results) console.log(JSON.stringify(r))
const totalIn = results.reduce((a, r) => a + r.cipherIn, 0)
const maxMs = Math.max(...results.map((r) => r.ms), 1)
console.log(
  `SUMMARY ok=${results.filter((r) => r.ok).length}/${concurrency} cipherInMB=${(totalIn / 1048576).toFixed(1)} avgMbps=${((totalIn * 8) / maxMs).toFixed(1)} elapsedMs=${maxMs}`,
)
process.exit(results.every((r) => r.ok) ? 0 : 1)
