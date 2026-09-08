// egress-geo 单测：node deploy/cf-gate-worker/egress-geo.test.mjs
//
// 覆盖：TTL 复用、stale 回退、unknown fail-closed 路径、并发去重、参数校验。
import { makeEgressGeoCache } from './egress-geo.mjs'

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) {
    passed++
    console.log(`  [pass] ${name}`)
  } else {
    failed++
    console.log(`  [fail] ${name} ${extra}`)
  }
}

function fakeClock(start = 1_000_000) {
  let t = start
  return { now: () => t, advance: (ms) => (t += ms) }
}

console.log('[1] 首次探测（fresh）')
{
  const clock = fakeClock()
  let calls = 0
  const cache = makeEgressGeoCache({
    probe: async () => {
      calls++
      return { ip: '104.28.158.1', country: 'us' }
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  const r = await cache.resolve()
  check('status=fresh', r.status === 'fresh', JSON.stringify(r))
  check('国家码大写归一化', r.country === 'US')
  check('探测调用 1 次', calls === 1)
  check('peek 可见最近结果', cache.peek() && cache.peek().country === 'US')
}

console.log('[2] TTL 内复用，不重复探测')
{
  const clock = fakeClock()
  let calls = 0
  const cache = makeEgressGeoCache({
    probe: async () => {
      calls++
      return { ip: '1.1.1.1', country: 'JP' }
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  await cache.resolve()
  clock.advance(999)
  const r = await cache.resolve()
  check('TTL 内仍 fresh', r.status === 'fresh')
  check('未重复探测', calls === 1, `calls=${calls}`)
  clock.advance(2)
  const r2 = await cache.resolve()
  check('TTL 过期后重新探测', r2.status === 'fresh' && calls === 2, `calls=${calls}`)
}

console.log('[3] 探测失败 → stale 回退（不误判为不合规）')
{
  const clock = fakeClock()
  let calls = 0
  let ok = true
  const cache = makeEgressGeoCache({
    probe: async () => {
      calls++
      if (!ok) throw new Error('echo service down')
      return { ip: '18.233.6.83', country: 'US' }
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  await cache.resolve()
  ok = false
  clock.advance(1001)
  const r = await cache.resolve()
  check('探测失败回退 stale', r.status === 'stale', JSON.stringify(r))
  check('沿用最近成功结果', r.country === 'US' && r.ip === '18.233.6.83')
  check('每次过期都重试探测', calls === 2, `calls=${calls}`)
}

console.log('[4] 超过 staleMs 且探测失败 → unknown（调用方 fail-closed）')
{
  const clock = fakeClock()
  const cache = makeEgressGeoCache({
    probe: async () => {
      throw new Error('down')
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  const r0 = await cache.resolve()
  check('从未成功过 → unknown', r0.status === 'unknown' && r0.country === '', JSON.stringify(r0))
  check('unknown 带 ageMs=-1', r0.ageMs === -1)

  let ok = true
  const cache2 = makeEgressGeoCache({
    probe: async () => {
      if (!ok) throw new Error('down')
      return { ip: '9.9.9.9', country: 'SG' }
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  await cache2.resolve()
  ok = false
  clock.advance(6000)
  const r1 = await cache2.resolve()
  check('stale 超期 → unknown', r1.status === 'unknown', JSON.stringify(r1))
}

console.log('[5] 并发去重：同一时刻只探测一次')
{
  const clock = fakeClock()
  let calls = 0
  let release
  const gate = new Promise((res) => (release = res))
  const cache = makeEgressGeoCache({
    probe: async () => {
      calls++
      await gate
      return { ip: '5.5.5.5', country: 'DE' }
    },
    ttlMs: 1000,
    staleMs: 5000,
    now: clock.now,
  })
  const all = Promise.all([cache.resolve(), cache.resolve(), cache.resolve()])
  release()
  const rs = await all
  check('三次并发只探测一次', calls === 1, `calls=${calls}`)
  check('三次结果一致', rs.every((r) => r.country === 'DE' && r.status === 'fresh'))
}

console.log('[6] 参数校验与边界')
{
  let threw = false
  try {
    makeEgressGeoCache({})
  } catch {
    threw = true
  }
  check('缺 probe 抛 TypeError', threw)

  threw = false
  try {
    makeEgressGeoCache({ probe: async () => ({}), ttlMs: 100, staleMs: 50 })
  } catch {
    threw = true
  }
  check('staleMs < ttlMs 抛 RangeError', threw)

  const clock = fakeClock()
  const cache = makeEgressGeoCache({
    probe: async () => ({ ip: '2.2.2.2' }),
    ttlMs: 1000,
    staleMs: 2000,
    now: clock.now,
  })
  const r = await cache.resolve()
  check('探测无国家码 → unknown', r.status === 'unknown', JSON.stringify(r))
  check('peek 仍为空', cache.peek() === null)
}

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)
