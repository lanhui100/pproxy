#!/usr/bin/env node
// 经 gate 隧道量"Google 看到的来源 IP/国家"——用来证明出口是否合规。
//
// 用法:
//   node gate_exitip.mjs [ws-url] [次数]
//   ws-url 缺省 wss://gate.ponyjob.top/ws
// token: 环境变量 PPROXY_TUNNEL_TOKEN，或 ~/.pony/config.toml 的 tunnel_token
//
// 依赖 ws：优先从本仓库 scripts/node_modules 解析，其次 deploy/cf-gate-worker/node_modules。

import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import tls from 'node:tls'
import { Duplex } from 'node:stream'

const here = path.dirname(fileURLToPath(import.meta.url))
const repo = path.resolve(here, '../../..')

function loadWs() {
  for (const base of [repo, path.join(repo, 'deploy/cf-gate-worker'), here]) {
    try {
      return createRequire(path.join(base, 'noop.js'))('ws')
    } catch {}
  }
  throw new Error('未找到 ws 模块：请在仓库根或 deploy/cf-gate-worker 下安装（pnpm i / npm i ws）')
}

function loadToken() {
  if (process.env.PPROXY_TUNNEL_TOKEN) return process.env.PPROXY_TUNNEL_TOKEN.trim()
  const cfg = path.join(os.homedir(), '.pony', 'config.toml')
  const text = fs.readFileSync(cfg, 'utf8')
  const m = text.match(/^\s*tunnel_token\s*=\s*"?([^"\n]+)"?/m)
  if (!m) throw new Error(`未在 ${cfg} 找到 tunnel_token，且未设置 PPROXY_TUNNEL_TOKEN`)
  return m[1].trim()
}

const WebSocket = loadWs()
const url = process.argv[2] || 'wss://gate.ponyjob.top/ws'
const rounds = Number(process.argv[3] || 3)
const token = loadToken()

function once() {
  return new Promise((resolve) => {
    const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } })
    let established = false
    const timer = setTimeout(() => {
      try { ws.close() } catch {}
      resolve('TIMEOUT')
    }, 25000)

    ws.on('open', () => ws.send(JSON.stringify({ host: 'ipinfo.io', port: 443 })))
    ws.on('error', (e) => { clearTimeout(timer); resolve(`WS ERR ${e.message}`) })
    ws.on('message', (data, isBinary) => {
      if (established || isBinary) return
      let msg
      try { msg = JSON.parse(data.toString()) } catch { return }
      if (msg.ok !== true) {
        clearTimeout(timer)
        resolve(`BIND DENIED ${JSON.stringify(msg)}`)
        try { ws.close() } catch {}
        return
      }
      established = true
      const duplex = new Duplex({
        read() {},
        write(chunk, _enc, cb) { try { ws.send(chunk) } catch {} cb() },
      })
      ws.on('message', (d, bin) => { if (bin) duplex.push(Buffer.isBuffer(d) ? d : Buffer.from(d)) })
      const sock = tls.connect(
        { socket: duplex, servername: 'ipinfo.io', rejectUnauthorized: false },
        () => sock.write('GET /json HTTP/1.1\r\nHost: ipinfo.io\r\nUser-Agent: pproxy-diag/1.0\r\nConnection: close\r\n\r\n'),
      )
      let buf = ''
      sock.on('data', (c) => {
        buf += c.toString()
        if (!buf.includes('\r\n\r\n') || !buf.includes('}')) return
        setTimeout(() => {
          clearTimeout(timer)
          const body = buf.split('\r\n\r\n').slice(1).join('')
          try {
            const o = JSON.parse(body)
            resolve(`${o.ip}  ${o.country}/${o.city || o.region || ''}  ${o.org || ''}`)
          } catch { resolve(`PARSE ${body.slice(0, 80)}`) }
          try { ws.close() } catch {}
        }, 300)
      })
      sock.on('error', (e) => { clearTimeout(timer); resolve(`TLS ERR ${e.message}`) })
    })
  })
}

console.log(`gate=${url}  rounds=${rounds}`)
for (let i = 1; i <= rounds; i++) console.log(`  #${i}: ${await once()}`)
