import { describe, expect, it } from 'vitest'

import { resolveExpiresDays } from './expiry'

describe('resolveExpiresDays', () => {
  it('never → undefined（请求体不带 expires_days 键）', () => {
    expect(resolveExpiresDays('never', 0)).toBeUndefined()
  })

  it('d30 → 30', () => {
    expect(resolveExpiresDays('d30', 0)).toBe(30)
  })

  it('d90 → 90', () => {
    expect(resolveExpiresDays('d90', 0)).toBe(90)
  })

  it('custom 正整数原样透传', () => {
    expect(resolveExpiresDays('custom', 1)).toBe(1)
    expect(resolveExpiresDays('custom', 180)).toBe(180)
  })

  it('custom 非正整数抛 Error("有效期不合法")', () => {
    expect(() => resolveExpiresDays('custom', 0)).toThrow('有效期不合法')
    expect(() => resolveExpiresDays('custom', -3)).toThrow('有效期不合法')
    expect(() => resolveExpiresDays('custom', 1.5)).toThrow('有效期不合法')
    expect(() => resolveExpiresDays('custom', Number.NaN)).toThrow('有效期不合法')
  })

  // ENG-1 回归钉：档位必须真实落到请求体形状，缺键 = 服务端永久密钥
  it('d30 → 请求体含 {expires_days:30}；never → 请求体不含该键', () => {
    const d30 = resolveExpiresDays('d30', 0)
    const bodyD30 = d30 !== undefined ? { expires_days: d30 } : {}
    expect(bodyD30).toEqual({ expires_days: 30 })

    const never = resolveExpiresDays('never', 0)
    const bodyNever = never !== undefined ? { expires_days: never } : {}
    expect(bodyNever).toEqual({})
    expect('expires_days' in bodyNever).toBe(false)
  })
})
