// gate worker — M6 spec §6：WS↔TCP 隧道桥（TLS 端到端透传，Worker 只见密文）
// relay 模式采用 CF 官方文档规范：sock.readable.pipeTo(WritableStream) +
// ws message → writer.write（此前手搓 for-await/双闭包模式触发本地 workerd 崩溃）。
import { connect } from 'cloudflare:sockets'
import {
  shouldBlockColo,
  parseBlockedColos,
  parseAllowedColos,
  strictGoogleEnabled,
  requiresCompliantEgress,
  shouldBlockEgress,
  parseAllowedEgressCountries,
} from './gate-policy.mjs'
import {
  parseFallbackConfig,
  channelOrder,
  socks5Connect,
  withTimeout,
} from './egress-fallback.mjs'
import { WS_PATH, DEBUG_PATH, DEBUG_EGRESS_PATH, routeFor, nginxWelcomePage } from './camouflage.mjs'
import { makeEgressGeoCache } from './egress-geo.mjs'
import { probeEgressGeo, DEFAULT_EGRESS_GEO_URL, DEFAULT_EGRESS_PROBE_TIMEOUT_MS } from './egress-probe.mjs'

const ALLOWED_PORTS = new Set([443]) // 80 明文透传默认禁用（M6 spec R8/F12）
const MAX_NAME_LEN = 253

// 出站地理门禁缓存：isolate 级复用，避免每个 bind 都做出站探测。
// env 只在 fetch 内可见，故惰性创建。
let egressCache = null

function getEgressCache(env) {
  if (!egressCache) {
    egressCache = makeEgressGeoCache({
      probe: () => probeEgressGeo(egressProbeConfig(env)),
      ttlMs: Number(env.EGRESS_GEO_TTL_MS) || undefined,
      staleMs: Number(env.EGRESS_GEO_STALE_MS) || undefined,
      log: (...args) => console.log(...args),
    })
  }
  return egressCache
}

async function sha256Hex(s) {
  const b = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s))
  return [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('')
}

function validHost(h) {
  if (!h || typeof h !== 'string' || h.length > MAX_NAME_LEN) return false
  const lower = h.toLowerCase().replace(/\.$/, '')
  if (lower === 'localhost' || lower.endsWith('.localhost')) return false
  if (/^(10\.|127\.|169\.254\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(lower)) return false
  if (/^0177\.|^0x7f\.|^\[?::1\]?$/.test(lower)) return false
  return true
}

function jsonResp(obj, status = 200) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

/// hash 归一化：trim + 小写。服务端只接受 64 位小写 hex 的 sha256；
/// 人肉粘贴/旧工具链带入的尾换行、大写一律归一，格式非法则比对必败（fail-closed）并由调用方记日志。
function normHash(h) {
  return typeof h === 'string' ? h.trim().toLowerCase() : ''
}

function isHashWellFormed(h) {
  return /^[0-9a-f]{64}$/.test(normHash(h))
}

/// 出站地理探测的生效配置（/debug/egress 与门禁共用，避免两处漂移）。
function egressProbeConfig(env) {
  return {
    url: env.EGRESS_GEO_URL || DEFAULT_EGRESS_GEO_URL,
    timeoutMs: Number(env.EGRESS_GEO_PROBE_TIMEOUT_MS) || DEFAULT_EGRESS_PROBE_TIMEOUT_MS,
  }
}

/// 建立单个出口通道（P0-1 多出口兜底）。
/// @param {string} channel 'direct' | 'socks5' | 'proxyip'
/// @param {ReturnType<parseFallbackConfig>} cfg
/// @param {{host: string, port: number}} target 客户端声明的目标
/// @param {number} timeoutMs 0 = 不设超时（仅直连、无兜底时的现状行为）
/// @returns {Promise<{sock: object, writer: object, leftover: Uint8Array|null}>}
async function openChannel(channel, cfg, target, timeoutMs) {
  let sock = null
  try {
    if (channel === 'direct') {
      sock = connect({ hostname: target.host, port: target.port })
      const writer = sock.writable.getWriter()
      await withTimeout(sock.opened, timeoutMs, 'direct connect')
      return { sock, writer, leftover: null }
    }
    if (channel === 'socks5') {
      // SOCKS5 链式：连用户自有 VPS（默认 1080 语义由配置 host:port 决定，参照
      // crates/core/src/relay.rs::socks5_connect）。握手复用同一 writer，成功后直接中继。
      sock = connect({ hostname: cfg.socks5.host, port: cfg.socks5.port })
      const writer = sock.writable.getWriter()
      await withTimeout(sock.opened, timeoutMs, 'socks5 tcp connect')
      const { leftover } = await socks5Connect(sock, writer, target.host, target.port, timeoutMs)
      return { sock, writer, leftover }
    }
    // proxyip：SNI 反代中继。客户端 TLS ClientHello 携带目标 host 的 SNI，
    // 反代据此把 TCP 流转发到真实目标（edgetunnel 反代语义），worker 只直连中继地址。
    sock = connect({ hostname: cfg.proxyIp.host, port: cfg.proxyIp.port })
    const writer = sock.writable.getWriter()
    await withTimeout(sock.opened, timeoutMs, 'proxyip connect')
    return { sock, writer, leftover: null }
  } catch (e) {
    // 超时/失败时关闭悬空 socket，避免黑洞通道泄漏
    try { sock?.close?.() } catch {}
    throw e
  }
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url)
    const route = routeFor(url.pathname)
    if (route === 'debug') {
      const cfg = parseFallbackConfig(env)
      return new Response(
        JSON.stringify({
          set: typeof env.TUNNEL_TOKEN_HASH === 'string',
          // 只暴露长度不暴露 hash 本体：len!=64 即 env 写脏（换行/截断），与 Vercel /debug 口径对齐
          len: typeof env.TUNNEL_TOKEN_HASH === 'string' ? env.TUNNEL_TOKEN_HASH.trim().length : 0,
          // 多出口兜底配置态（P0-1，不含端点/国家码本体，仅暴露"是否配置"）
          fallback: {
            socks5: !!cfg.socks5,
            proxyip: !!cfg.proxyIp,
            socks5CountryDeclared: !!cfg.socks5Country,
            proxyipCountryDeclared: !!cfg.proxyIpCountry,
            attemptTimeoutMs: cfg.attemptTimeoutMs,
          },
        }),
        { headers: { 'content-type': 'application/json' } },
      )
    }
    // 出站地理探测诊断端点（与 /ws 同一 Bearer token 保护）：
    // 直接返回探测结果或真实报错，便于排查 unsupported_egress:UNKNOWN 的成因。
    if (route === 'debugEgress') {
      const auth = request.headers.get('Authorization') ?? ''
      const presented = auth.startsWith('Bearer ') ? auth.slice(7).trim() : ''
      if (!presented || (await sha256Hex(presented)) !== normHash(env.TUNNEL_TOKEN_HASH)) {
        if (!isHashWellFormed(env.TUNNEL_TOKEN_HASH)) {
          console.log('[gate] TUNNEL_TOKEN_HASH malformed: len=', String(env.TUNNEL_TOKEN_HASH ?? '').trim().length)
        }
        return new Response('unauthorized', { status: 401 })
      }
      const cfg = egressProbeConfig(env)
      const allowed = parseAllowedEgressCountries(env.EGRESS_ALLOWED_COUNTRIES)
      try {
        const geo = await probeEgressGeo(cfg)
        return jsonResp({
          ok: true,
          ip: geo.ip,
          country: geo.country,
          compliant: allowed.includes(geo.country),
          allowedCountries: allowed,
          config: cfg,
        })
      } catch (e) {
        return jsonResp({ ok: false, error: String((e && e.message) || e), config: cfg })
      }
    }
    // 非隧道路径一律返回伪装页（B011 反指纹：不再裸 404"not found"，
    // 浏览器直开域名看到普通站点；/debug、/debug/egress 已在上文处理）。
    if (route !== 'ws') return nginxWelcomePage()
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') {
      return new Response('websocket required', { status: 400 })
    }
    const auth = request.headers.get('Authorization') ?? ''
    const presented = auth.startsWith('Bearer ') ? auth.slice(7).trim() : ''
    const hash = await sha256Hex(presented)
    // 归一化比对：env 侧 trim+小写（消灭 wrangler secret/手工粘贴带入的尾换行与大小写漂移）；
    // presented 侧 trim（token 字母表无首尾空格）。格式非法时服务端日志告警，不向客户端泄细节。
    if (!presented || hash !== normHash(env.TUNNEL_TOKEN_HASH)) {
      if (!isHashWellFormed(env.TUNNEL_TOKEN_HASH)) {
        console.log('[gate] TUNNEL_TOKEN_HASH malformed: len=', String(env.TUNNEL_TOKEN_HASH ?? '').trim().length)
      }
      return new Response('unauthorized', { status: 401 })
    }

    const pair = new WebSocketPair()
    // 2026-08 审计修复：原实现 acceptWebSocket(server)/返回 server 在生产均抛 500。
    // 正确组合：server.accept() 后 Response 必须携带 client 端（pair[0]），已 E2E 验证。
    const server = pair[1]
    server.accept()

    let established = false
    let writer = null

    server.addEventListener('message', async (event) => {
      if (event.data instanceof ArrayBuffer) {
        writer?.write(new Uint8Array(event.data))
        return
      }
      if (established) return
      let req
      try {
        req = JSON.parse(event.data)
      } catch {
        server.close(1008, 'bad first frame')
        return
      }
      const port = Number(req.port)
      if (!validHost(req.host) || !ALLOWED_PORTS.has(port)) {
        server.send(JSON.stringify({ ok: false, reason: 'acl denied' }))
        server.close(1008, 'acl denied')
        return
      }
      // colo 门禁：Google 系 host ∧ 非合规区域/黑名单 → 秒拒，
      // 客户端（桌面端 engine_tunnel）denied fallover 自动落到 Vercel iad1 美区兜底。
      // 非 Google host 任何 colo 放行，防全量流量倾泻到兜底出口。
      // 严格白名单默认开启（fail-closed）：仅当显式设置 STRICT_GOOGLE_WHITELIST=false/0 时关闭，
      // 保证"仅放行 Google 官方合规区域"不是可选项而是生产默认（P1-2 修复）。
      const coloReason = shouldBlockColo(request.cf?.colo, req.host, {
        blockedColos: parseBlockedColos(env.BLOCKED_COLOS),
        allowedColos: parseAllowedColos(env.ALLOWED_COLOS),
        strictWhitelist: strictGoogleEnabled(env.STRICT_GOOGLE_WHITELIST),
      })
      if (coloReason) {
        console.log('[gate] colo blocked', coloReason, req.host)
        server.send(JSON.stringify({ ok: false, reason: coloReason }))
        server.close(1008, coloReason)
        return
      }

      // 多出口兜底（P0-1）：直连失败/被 CF 收紧时自动切用户配置的兜底通道。
      // 通道编排在 egress-fallback.mjs：direct（现状优先）→ socks5 → proxyip，
      // 全部按 channelOrder 排好序且已过滤不合规通道（合规 host 仅保留声明合规的兜底）。
      const cfg = parseFallbackConfig(env)
      let channels = channelOrder(req.host, cfg)

      // 出站地理门禁（A 方案）：入站 colo 合规 ≠ 出站 egress IP 合规
      // （connect() 的出站 IP 由 CF 另行分配）。仅对 Cloud Code 系 host 生效，
      // fail-closed；非合规 host 一律放行，避免把泛 Google 流量倾泻到兜底出口。
      // 有声明合规的兜底通道时：直连被 geo 门禁拒 → 直接切兜底（不出 Vercel failover）；
      // 无兜底时维持现状（拒绝 → 客户端 failover 到 Vercel 合规出口）。
      if (requiresCompliantEgress(req.host)) {
        const egress = await getEgressCache(env).resolve()
        const egressReason = shouldBlockEgress(egress, req.host, {
          allowedCountries: parseAllowedEgressCountries(env.EGRESS_ALLOWED_COUNTRIES),
        })
        if (egressReason) {
          const compliantFallbacks = channels.filter((c) => c !== 'direct')
          if (compliantFallbacks.length) {
            console.log('[gate] egress blocked, switch to compliant fallback', egressReason, req.host)
            channels = compliantFallbacks
          } else {
            console.log(
              '[gate] egress blocked',
              egressReason,
              req.host,
              egress.status,
              egress.ip || '',
            )
            server.send(JSON.stringify({ ok: false, reason: egressReason }))
            server.close(1008, egressReason)
            return
          }
        }
      }

      try {
        // 顺序尝试各通道；只有直连时维持现状（无超时，避免行为漂移），
        // 存在兜底通道时给每个通道建立超时（防黑洞挂死永远切不到兜底）。
        let via = null
        let sock = null
        let leftover = null
        let lastErr = null
        const attemptTimeoutMs = channels.length > 1 ? cfg.attemptTimeoutMs : 0
        for (const channel of channels) {
          try {
            const opened = await openChannel(channel, cfg, req, port, attemptTimeoutMs)
            sock = opened.sock
            writer = opened.writer
            leftover = opened.leftover
            via = channel
            break
          } catch (e) {
            lastErr = e
            console.log('[gate] channel failed', channel, String(e))
          }
        }
        if (!sock) {
          console.log('[gate] connect error (all channels)', String(lastErr))
          server.send(JSON.stringify({ ok: false, reason: String(lastErr) }))
          try { server.close(1011, 'connect failed') } catch {}
          return
        }
        console.log('[gate] connected via', via, req.host, port)
        established = true
        server.send(JSON.stringify({ ok: true, via }))
        // leftover（SOCKS5 握手后已读到的上游早到字节）必须先于 pipeTo flush 给客户端，
        // 与 crates/core/src/relay.rs relay_with_leftover 语义对齐，防止粘包丢字节。
        if (leftover && leftover.length) {
          try { server.send(leftover) } catch {}
        }
        // TCP→WS：官方规范 pipe 模式
        // 上游正常 EOF（pipeTo resolve）与异常（reject）都必须关闭 WS——
        // 此前只挂了 .catch，正常关闭时 WS 悬挂成"半死隧道"，客户端侧表现为 EOF。
        const closeUpstream = () => {
          try { server.close(1000, 'upstream closed') } catch {}
        }
        sock.readable
          .pipeTo(
            new WritableStream({
              write(chunk) {
                try {
                  server.send(chunk)
                } catch {}
              },
            }),
          )
          .then(closeUpstream, closeUpstream)
      } catch (e) {
        console.log('[gate] connect error', String(e))
        server.send(JSON.stringify({ ok: false, reason: String(e) }))
        try { server.close(1011, 'connect failed') } catch {}
      }
    })

    server.addEventListener('close', () => {
      writer?.close().catch(() => {})
    })

    return new Response(null, { status: 101, webSocket: pair[0] })
  },
}
