#!/usr/bin/env node
// gen-connect-code — 生成 pony-gate:// 连接口令（运维侧分发给桌面端用户）。
//
// 用法：
//   node gen-connect-code.mjs <gate-url[,gate-url2...]> <token>
//   node gen-connect-code.mjs   # 使用默认双 gate 端点，token 从环境变量 TUNNEL_TOKEN 读取
//
// 输出：pony-gate://<base64url(JSON {"v":1,"u":url,"t":token})>
// 口令含明文 token，与 token 同级机密，请通过安全渠道分发。

const DEFAULT_URLS = 'wss://vgate.ponyjob.top/api/ws,wss://gate.ponyjob.top/ws'

const args = process.argv.slice(2)
let urls = args[0] || DEFAULT_URLS
let token = args[1] || process.env.TUNNEL_TOKEN || ''

const list = urls.split(/[,;\n]/).map((s) => s.trim()).filter(Boolean)
if (!list.length || list.some((u) => !u.startsWith('wss://'))) {
  console.error('ERROR: 端点必须全部以 wss:// 开头（连接口令强制加密端点）')
  process.exit(64)
}
if (!token.trim()) {
  console.error('ERROR: 缺少 token（参数 2 或环境变量 TUNNEL_TOKEN）')
  process.exit(64)
}

const payload = JSON.stringify({ v: 1, u: list.join(','), t: token.trim() })
const code = 'pony-gate://' + Buffer.from(payload, 'utf8').toString('base64url')
console.log(code)
