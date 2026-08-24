#!/usr/bin/env node
// spike-tunnel.mjs — M6 P0 S1/S2：经 gate worker 隧道拉大文件，测吞吐/CPU/并发
//
// 用法：
//   单轮（S2 并发口径）：node spike-tunnel.mjs <ws-url> <token> [file] [--concurrency N]
//   持续（S1 口径）    ：node spike-tunnel.mjs <ws-url> <token> [file] --loop 600 [--concurrency 1]
// 指标：ciphertext 总量（隧道吞吐）为主指标；plainMB 为 TLS 解密后明文量。
import WebSocket from 'ws'
import tls from 'node:tls'
import { Duplex } from 'node:stream'

const args = process.argv.slice(2)
const wsUrl = args[0]
const token = args[1]
let filePath = args[2] ?? '/files/100Mb.dat'
if (!filePath.startsWith('/')) filePath = '/' + filePath
function numOpt(flag, dflt) {
  const i = args.indexOf(flag)
  return i > -1 ? Number(args[i + 1]) : dflt
}
const concurrency = numOpt('--concurrency', 1)
const loopSec = numOpt('--loop', 0) // >0：重复下载直至满时长（S1 持续负载口径）

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
  return { sock, pushCipher: (d) => { pushed += d.length; sock.push(Buffer.from(d)) }, pushed: () => pushed }
}

/** 单次下载：建隧道→TLS→GET→拉到连接关闭。直接变异外部 st（聚合无丢失）。 */
function oneDownload(id, st, deadlineMs) {
  return new Promise((resolve) => {
    if (Date.now() >= deadlineMs || Date.now() - st.lastRoundEnd < 300 && st.rounds > 0) {
      return resolve()
    }
    const ws = new WebSocket(wsUrl, { headers: { Authorization: `Bearer ${token}` } })
    const remaining = deadlineMs - Date.now()
    const kill = setTimeout(() => fail(new Error(`deadline hit (${Math.max(remaining, 0)}ms left)`)), Math.max(remaining, 1000))
    let bridgeRef = null

    function fail(err) {
      clearTimeout(kill)
      st.error = String(err.message ?? err)
      st.ok = false
      try { ws.close() } catch {}
      resolve()
    }

    ws.on('error', (e) => fail(e))
    ws.on('message', (data, isBinary) => {
      if (isBinary) {
        bridgeRef?.pushCipher(data)
        st.cipherIn = bridgeRef.pushed()
        return
      }
      const msg = JSON.parse(data.toString())
      if (!msg.ok) return fail(new Error(`tunnel denied: ${msg.reason ?? ''}`))

      st.phase = 'tls'
      const br = bridge(ws)
      bridgeRef = br
      const tlsSock = tls.connect({ socket: br.sock, servername: TARGET_HOST }, () => {
        st.phase = 'http'
        tlsSock.write(
          `GET ${filePath} HTTP/1.1\r\nHost: ${TARGET_HOST}\r\nUser-Agent: spike/1.0\r\nConnection: close\r\n\r\n`,
        )
      })
      tlsSock.on('error', (e) => fail(e))
      let headerDone = false
      tlsSock.on('data', (chunk) => {
        if (!headerDone) {
          const idx = chunk.indexOf('\r\n\r\n')
          if (idx > -1) {
            headerDone = true
            const status = Number(chunk.slice(9, 12))
            if (status !== 200) return fail(new Error(`HTTP ${status} over tunnel`))
            st.plainMB += (chunk.length - idx - 4) / 1048576
          }
          return
        }
        st.plainMB += chunk.length / 1048576
      })
      tlsSock.on('close', () => {
        st.lastRoundEnd = Date.now()
        st.rounds++
        clearTimeout(kill)
        resolve()
      })
    })

    ws.on('open', () => {
      ws.send(JSON.stringify({ host: TARGET_HOST, port: 443 }))
    })
  })
}

/** 聚合 worker：持续模式循环下载至 deadline；单轮模式跑一次。 */
async function worker(id) {
  const st = { id, ok: true, cipherIn: 0, plainMB: 0, ms: 0, rounds: 0, error: '' }
  const deadline = Date.now() + (loopSec > 0 ? loopSec : 60) * 1000
  if (loopSec <= 0) {
    const r = await oneDownload(id, st, Date.now() + 60_000)
    return { ...st, ...r }
  }
  while (Date.now() < deadline && !st.error) {
    await oneDownload(id, st, deadline)
  }
  st.ms = Date.now() - (deadline - (loopSec > 0 ? loopSec : 60) * 1000)
  return st
}

console.log(
  `spike: url=${wsUrl} target=${TARGET_HOST}:443 file=${filePath} concurrency=${concurrency}` +
    (loopSec > 0 ? ` loopSec=${loopSec}（S1 持续负载口径）` : '（单轮，S2 口径）'),
)
const results = await Promise.all(Array.from({ length: concurrency }, (_, i) => worker(i)))
for (const r of results) console.log(JSON.stringify(r))
const totalIn = results.reduce((a, r) => a + r.cipherIn, 0)
const totalPlain = results.reduce((a, r) => a + r.plainMB, 0).toFixed(1)
const wallMs = loopSec > 0 ? loopSec * 1000 : Math.max(...results.map((r) => r.ms), 1)
console.log(
  `SUMMARY ok=${results.filter((r) => r.ok && !r.error).length}/${concurrency} cipherInMB=${(totalIn / 1048576).toFixed(1)} plainMB=${totalPlain} avgMbps=${((totalIn * 8) / wallMs).toFixed(1)} errors=${results.map((r) => r.error).filter(Boolean).join(';') || 'none'}`,
)
process.exit(results.every((r) => !r.error) ? 0 : 1)
