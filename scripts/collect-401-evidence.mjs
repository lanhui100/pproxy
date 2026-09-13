#!/usr/bin/env node
// 401 取证包：一键采集双出口 401 判定所需的最小证据集（P0/E1-E10）。
//
// 红线：包内只允许 fp8 / set / len / ok / err 分类 / url / 日志脱敏行；
// 禁止明文 token、pony-gate://、TUNNEL_TOKEN_HASH 全值。末尾有违禁 pattern 门禁。
//
// 用法：
//   node scripts/collect-401-evidence.mjs --token-file <path> [--out evidence/]
import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import os from 'node:os'

const args = process.argv.slice(2)
const get = (k) => {
  const i = args.indexOf(k)
  return i >= 0 ? args[i + 1] : null
}
const tokenFile = get('--token-file')
const outDir = get('--out') || 'evidence'
if (!tokenFile || !fs.existsSync(tokenFile)) {
  console.error('usage: node scripts/collect-401-evidence.mjs --token-file <path> [--out evidence/]')
  process.exit(64)
}

const CF_WS = process.env.CF_GATE_WS || 'wss://gate.example.com/ws'
const VERCEL_WS = process.env.VERCEL_GATE_WS || 'wss://vgate.example.com/api/ws'
const CF_DEBUG = process.env.CF_GATE_DEBUG || 'https://gate.example.com/debug'
const VERCEL_DEBUG = process.env.VERCEL_GATE_DEBUG || 'https://vgate.example.com/debug'

const token = fs.readFileSync(tokenFile, 'utf8').trim()
const fp8 = crypto.createHash('sha256').update(token).digest('hex').slice(0, 8)

fs.mkdirSync(outDir, { recursive: true })
const write = (name, obj) => fs.writeFileSync(path.join(outDir, name), typeof obj === 'string' ? obj : JSON.stringify(obj, null, 2))

// 1. 本机指纹（只记 fp8）
write('local-fingerprint.json', { fp8, source: 'token-file', at: new Date().toISOString() })

// 2. 双端 /debug（免鉴权 set/len）
for (const [name, url] of [['debug-cf.json', CF_DEBUG], ['debug-vercel.json', VERCEL_DEBUG]]) {
  try {
    const r = await fetch(url)
    write(name, { url, status: r.status, body: await r.json() })
  } catch (e) {
    write(name, { url, error: String(e) })
  }
}

// 3. 同 token 双端强冒烟（smoke-test.mjs，记 OK/TLS / 401 / denied / timeout）
const { execFile } = await import('node:child_process')
const { promisify } = await import('node:util')
const run = promisify(execFile)
for (const [name, ws] of [['smoke-cf.txt', CF_WS], ['smoke-vercel.txt', VERCEL_WS]]) {
  try {
    const { stdout, stderr } = await run('node', ['deploy/vercel-gate-worker/smoke-test.mjs', ws, 'www.google.com', '443', '--token-file', tokenFile], { timeout: 25000 })
    write(name, `URL: ${ws}\nFP8: ${fp8}\n--- stdout ---\n${stdout}\n--- stderr ---\n${stderr}\n`)
  } catch (e) {
    write(name, `URL: ${ws}\nFP8: ${fp8}\n--- exit ${e.code ?? '?'} ---\n${e.stdout ?? ''}\n${e.stderr ?? ''}\n${e.message ?? ''}\n`)
  }
}

// 4. 取证三问模板（注意：模板原文不得含 pony-gate:// 全串，否则违禁门禁自杀误杀本文件）
write('answers.md', `# 隧道 401 取证三问（fp8=${fp8}）
1. 是否轮换：[ ] 未被通知 / [ ] 已被通知（时间：____，来源：____）
2. 失效前操作：[ ] 重贴授权码 [ ] 导入连接口令 [ ] 同步口令 [ ] 重启应用/系统
   [ ] 外部脚本写凭据 [ ] 升级版本 [ ] 改端点 URL [ ] 无操作（时间线：____）
3. 指纹：桌面自检 fingerprint=____（null 则贴 cred_error 原文）；上次记住的指纹=____（无则空）
   本包 token-file fp8=${fp8}；桌面自检 fp8 必须与此一致，否则先查本地分叉（H2）。
`)

// 5. 违禁 pattern 门禁（命中即阻断发送 протест；只拦带 payload 的口令串，不拦模板提及）
const FORBIDDEN = /Bearer\s+[A-Za-z0-9]|pony-gate:\/\/[A-Za-z0-9+/=_-]{8,}|"t"\s*:\s*"[^"]{8,}|TUNNEL_TOKEN_HASH\s*=\s*[0-9a-f]{16,}/i
const files = fs.readdirSync(outDir)
let blocked = []
for (const f of files) {
  const c = fs.readFileSync(path.join(outDir, f), 'utf8')
  // answers/smoke 允许出现 fp8；禁止的是明文 token（长度远超 8）与口令/全值 hash
  if (FORBIDDEN.test(c)) blocked.push(f)
}
if (blocked.length) {
  console.error(`BLOCKED: 以下文件命中违禁 pattern（疑含明文 token/口令/hash 全值），禁止发送：${blocked.join(', ')}`)
  process.exit(3)
}
console.log(`EVIDENCE_OK out=${outDir} fp8=${fp8} files=${files.join(',')}`)
