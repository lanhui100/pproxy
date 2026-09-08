// egress-geo — 出站 IP 地理探测结果的进程内缓存（纯逻辑，node 可直接单测）。
//
// 设计要点：
// 1. TTL 内直接复用（fresh），避免每次 bind 都做出站探测；
// 2. 探测失败回退到最近一次成功结果（stale，最长 staleMs），
//    避免 echo 服务抖动把合规出口误判成不合规、把流量全推给兜底出口；
// 3. 超过 staleMs 仍无结果 → status='unknown'，由 shouldBlockEgress fail-closed；
// 4. 并发探测去重：同一 isolate 内多个 bind 同时到达只打一次探测。

export const DEFAULT_TTL_MS = 300_000
export const DEFAULT_STALE_MS = 1_800_000

/**
 * @param {{
 *   probe: () => Promise<{ip?: string, country?: string}>,
 *   ttlMs?: number, staleMs?: number,
 *   now?: () => number, log?: (...args: unknown[]) => void,
 * }} options
 */
export function makeEgressGeoCache({
  probe,
  ttlMs = DEFAULT_TTL_MS,
  staleMs = DEFAULT_STALE_MS,
  now = () => Date.now(),
  log = () => {},
} = {}) {
  if (typeof probe !== 'function') {
    throw new TypeError('makeEgressGeoCache: probe must be a function')
  }
  if (!(ttlMs > 0) || !(staleMs >= ttlMs)) {
    throw new RangeError('makeEgressGeoCache: require 0 < ttlMs <= staleMs')
  }

  let last = null
  let inflight = null

  async function runProbe() {
    try {
      const r = await probe()
      const ip = String((r && r.ip) || '').trim()
      const country = String((r && r.country) || '').trim().toUpperCase()
      if (!country) throw new Error(`probe returned no country (ip=${ip || 'n/a'})`)
      last = { ip, country, at: now() }
      log('[egress-geo] ok', country, ip)
      return last
    } catch (e) {
      log('[egress-geo] probe failed:', String((e && e.message) || e))
      return null
    }
  }

  return {
    /** @returns {Promise<{country: string, ip: string, status: 'fresh'|'stale'|'unknown', ageMs: number}>} */
    async resolve() {
      const t0 = now()
      if (last && t0 - last.at <= ttlMs) {
        return { country: last.country, ip: last.ip, status: 'fresh', ageMs: t0 - last.at }
      }

      if (!inflight) {
        inflight = runProbe().finally(() => {
          inflight = null
        })
      }
      const fresh = await inflight
      if (fresh) {
        return {
          country: fresh.country,
          ip: fresh.ip,
          status: 'fresh',
          ageMs: now() - fresh.at,
        }
      }

      const t1 = now()
      if (last && t1 - last.at <= staleMs) {
        return { country: last.country, ip: last.ip, status: 'stale', ageMs: t1 - last.at }
      }
      return { country: '', ip: '', status: 'unknown', ageMs: -1 }
    },

    /** 仅供诊断/测试：查看最近一次成功结果（不含状态判定）。 */
    peek() {
      return last ? { ...last } : null
    },
  }
}
