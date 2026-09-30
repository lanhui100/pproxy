// egress-fallback — gate worker 多出口兜底（P0-1，借鉴 edgetunnel 反代兜底语义）
// 纯函数 + WHATWG 流式 SOCKS5 客户端；worker.js 传入 cloudflare:sockets 的 connect()，
// node 测试传入包装后的 net 连接，同一套逻辑两端复用。
//
// 通道顺序：direct（现状优先）→ socks5（若配置）→ proxyip（若配置）；
// 每个通道在 EGRESS_ATTEMPT_TIMEOUT_MS 内未建立即失败切下一个，全部失败由调用方报错
// （客户端既有 Vercel failover 逻辑不变，作为最后一道兜底）。
//
// 合规贯通：兜底通道必须先过 gate-policy.mjs 判定——
// 非合规 host 兜底自由（与直连现状一致）；requiresCompliantEgress 的 host（Google
// Cloud Code 系）仅当 {SOCKS5,PROXYIP}_COUNTRY 显式声明且 shouldBlockEgress 放行时
// 才可用，未声明/未放行一律禁用（fail-closed，合规后缀仍只走白名单出口）。

import {
  requiresCompliantEgress,
  shouldBlockEgress,
  parseAllowedEgressCountries,
} from './gate-policy.mjs'

export const DEFAULT_ATTEMPT_TIMEOUT_MS = 8000

/// 解析 "host[:port]" / "[v6]:port" / "host"（默认端口）。非法/空返回 null。
export function parseEndpoint(spec, defaultPort = 443) {
  if (typeof spec !== 'string' || !spec.trim()) return null
  const s = spec.trim()
  let host = s
  let port = defaultPort

  if (s.startsWith('[')) {
    const close = s.indexOf(']')
    if (close === -1) return null
    host = s.slice(1, close)
    const rest = s.slice(close + 1)
    if (rest.startsWith(':')) port = Number(rest.slice(1))
    else if (rest) return null
  } else {
    const idx = s.lastIndexOf(':')
    if (idx !== -1) {
      host = s.slice(0, idx)
      port = Number(s.slice(idx + 1))
    }
  }

  if (!host || !Number.isInteger(port) || port <= 0 || port > 65535) return null
  return { host, port }
}

/// 兜底配置：全部可选，默认留空 = 仅直连（行为与现状完全一致）。
export function parseFallbackConfig(env) {
  const e = env || {}
  return {
    socks5: parseEndpoint(e.SOCKS5_PROXY),
    proxyIp: parseEndpoint(e.PROXYIP_HOST, 443),
    socks5Country: (e.SOCKS5_COUNTRY || '').trim().toUpperCase() || null,
    proxyIpCountry: (e.PROXYIP_COUNTRY || '').trim().toUpperCase() || null,
    allowedCountries: parseAllowedEgressCountries(e.EGRESS_ALLOWED_COUNTRIES),
    attemptTimeoutMs: Number(e.EGRESS_ATTEMPT_TIMEOUT_MS) || DEFAULT_ATTEMPT_TIMEOUT_MS,
  }
}

/// 兜底通道合规判定：合规 host 必须显式声明国家码且过 shouldBlockEgress 才放行。
/// @param {string} host 目标主机
/// @param {string|null} country 声明国家码（大写或 null）
/// @param {{allowedCountries?: string[]}} cfg
export function fallbackAllowed(host, country, cfg) {
  if (!requiresCompliantEgress(host)) return true // 非合规 host：与直连现状一致，不限 geo
  if (!country) return false // 合规 host 未声明国家码 → fail-closed
  return (
    shouldBlockEgress(
      { country, status: 'fresh' },
      host,
      { allowedCountries: cfg.allowedCountries },
    ) === null
  )
}

/// 通道编排：返回按序尝试的通道名数组（直连恒在首位）。
export function channelOrder(host, cfg) {
  const order = ['direct']
  if (cfg.socks5 && fallbackAllowed(host, cfg.socks5Country, cfg)) order.push('socks5')
  if (cfg.proxyIp && fallbackAllowed(host, cfg.proxyIpCountry, cfg)) order.push('proxyip')
  return order
}

/// Promise 超时包装：超时抛错（不吞原始 rejection）。
export function withTimeout(promise, ms, label = 'operation') {
  if (!ms || ms <= 0) return promise
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms)
    promise.then(
      (v) => { clearTimeout(timer); resolve(v) },
      (e) => { clearTimeout(timer); reject(e) },
    )
  })
}

/// 缓冲读：从 WHATWG ReadableStream 精确读 n 字节，多余字节留在 buffer 供 leftover()
/// 取回（对齐 crates/core/src/relay.rs 的 relay_with_leftover 语义，防止握手粘包丢字节）。
class BufferedReader {
  constructor(reader, timeoutMs) {
    this.reader = reader
    this.timeoutMs = timeoutMs
    this.chunks = []
    this.offset = 0
    this.buffered = 0
    this.eof = false
  }

  async read(n) {
    while (this.buffered < n) {
      if (this.eof) throw new Error('socks5 EOF during handshake')
      const { value, done } = await withTimeout(
        this.reader.read(),
        this.timeoutMs,
        'socks5 handshake read',
      )
      if (done) {
        this.eof = true
        throw new Error('socks5 EOF during handshake')
      }
      this.chunks.push(value)
      this.buffered += value.length
    }
    const out = new Uint8Array(n)
    let filled = 0
    while (filled < n) {
      const c = this.chunks[0]
      const take = Math.min(c.length - this.offset, n - filled)
      out.set(c.subarray(this.offset, this.offset + take), filled)
      filled += take
      this.offset += take
      this.buffered -= take
      if (this.offset === c.length) {
        this.chunks.shift()
        this.offset = 0
      }
    }
    return out
  }

  /// 握手后仍留在 buffer 的字节（理论上为 0，防御性取回）。
  leftover() {
    if (this.buffered <= 0) return null
    const out = new Uint8Array(this.buffered)
    let off = 0
    for (const c of this.chunks) {
      const take = c.length - this.offset
      out.set(c.subarray(this.offset), off)
      off += take
    }
    return out
  }

  finish() {
    const l = this.leftover()
    this.reader.releaseLock()
    return l
  }
}

/// SOCKS5 客户端握手（RFC 1928，无认证 + 域名 CONNECT）。
/// 参照 crates/core/src/relay.rs::socks5_connect 语义，但**完整消费 CONNECT 回复**
/// （含 ATYP 变长 BND.ADDR），避免回复残字节被当作隧道数据转发给客户端。
/// @param {{readable: ReadableStream, writable: WritableStream}} sock
/// @param {WritableStreamDefaultWriter} writer 调用方已获取的 writer（握手与中继共用）
/// @param {string} targetHost
/// @param {number} targetPort
/// @param {number} timeoutMs
/// @returns {{leftover: Uint8Array|null}} 握手后多余字节（正常情况下 null）
export async function socks5Connect(sock, writer, targetHost, targetPort, timeoutMs) {
  const br = new BufferedReader(sock.readable.getReader(), timeoutMs)
  let leftover = null
  try {
    // greeting: VER=5, NMETHODS=1, METHODS=[no-auth]
    await withTimeout(writer.write(new Uint8Array([0x05, 0x01, 0x00])), timeoutMs, 'socks5 greeting')
    const sel = await br.read(2)
    if (sel[0] !== 0x05 || sel[1] !== 0x00) {
      throw new Error(`socks5 greeting rejected: method=${sel[1]}`)
    }

    // CONNECT (domain): VER=5 CMD=1 RSV=0 ATYP=3 len host port
    const hostBytes = new TextEncoder().encode(targetHost)
    if (hostBytes.length > 255) throw new Error('socks5 target host too long')
    const req = new Uint8Array(4 + 1 + hostBytes.length + 2)
    req[0] = 0x05
    req[1] = 0x01
    req[2] = 0x00
    req[3] = 0x03
    req[4] = hostBytes.length
    req.set(hostBytes, 5)
    req[5 + hostBytes.length] = (targetPort >> 8) & 0xff
    req[6 + hostBytes.length] = targetPort & 0xff
    await withTimeout(writer.write(req), timeoutMs, 'socks5 connect')

    // 回复：VER REP RSV ATYP [BND.ADDR BND.PORT]——完整消费，防残字节进隧道
    const rep = await br.read(4)
    if (rep[0] !== 0x05) throw new Error(`socks5 bad version: ${rep[0]}`)
    if (rep[1] !== 0x00) throw new Error(`socks5 connect failed: REP=${rep[1]}`)
    const atyp = rep[3]
    let addrLen = 0
    if (atyp === 0x01) addrLen = 4 // IPv4
    else if (atyp === 0x04) addrLen = 16 // IPv6
    else if (atyp === 0x03) addrLen = (await br.read(1))[0] // domain
    else throw new Error(`socks5 bad ATYP: ${atyp}`)
    await br.read(addrLen + 2)
  } finally {
    // 无论成败都释放读锁（失败时调用方负责关闭 socket）
    leftover = br.finish()
  }
  return { leftover }
}
