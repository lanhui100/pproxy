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

/// 出站地理探测的生效配置（/debug/egress 与门禁共用，避免两处漂移）。
function egressProbeConfig(env) {
  return {
    url: env.EGRESS_GEO_URL || DEFAULT_EGRESS_GEO_URL,
    timeoutMs: Number(env.EGRESS_GEO_PROBE_TIMEOUT_MS) || DEFAULT_EGRESS_PROBE_TIMEOUT_MS,
  }
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url)
    if (url.pathname === '/debug') {
      return new Response(
        JSON.stringify({ set: typeof env.TUNNEL_TOKEN_HASH === 'string' }),
        { headers: { 'content-type': 'application/json' } },
      )
    }
    // 出站地理探测诊断端点（与 /ws 同一 Bearer token 保护）：
    // 直接返回探测结果或真实报错，便于排查 unsupported_egress:UNKNOWN 的成因。
    if (url.pathname === '/debug/egress') {
      const auth = request.headers.get('Authorization') ?? ''
      const presented = auth.startsWith('Bearer ') ? auth.slice(7) : ''
      if (!presented || (await sha256Hex(presented)) !== env.TUNNEL_TOKEN_HASH) {
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
    if (url.pathname !== '/ws') return new Response('not found', { status: 404 })
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') {
      return new Response('websocket required', { status: 400 })
    }
    const auth = request.headers.get('Authorization') ?? ''
    const presented = auth.startsWith('Bearer ') ? auth.slice(7) : ''
    const hash = await sha256Hex(presented)
    if (!presented || hash !== env.TUNNEL_TOKEN_HASH) {
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

      // 出站地理门禁（A 方案）：入站 colo 合规 ≠ 出站 egress IP 合规
      // （connect() 的出站 IP 由 CF 另行分配）。仅对 Cloud Code 系 host 生效，
      // fail-closed；非合规 host 一律放行，避免把泛 Google 流量倾泻到兜底出口。
      if (requiresCompliantEgress(req.host)) {
        const egress = await getEgressCache(env).resolve()
        const egressReason = shouldBlockEgress(egress, req.host, {
          allowedCountries: parseAllowedEgressCountries(env.EGRESS_ALLOWED_COUNTRIES),
        })
        if (egressReason) {
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
      try {
        console.log('[gate] connecting', req.host, port)
        const sock = connect({ hostname: req.host, port })
        const up = sock.writable.getWriter()
        await sock.opened
        established = true
        server.send(JSON.stringify({ ok: true }))
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
        writer = up
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
