import { describe, expect, it } from 'vitest'

import { joinUsageTokenName } from './usageJoin'

const tokens = [
  { id: 1, name: 'my-laptop', status: 'active' },
  { id: 2, name: 'old-phone', status: 'revoked' },
  { id: 3, name: 'pad', status: 'active' },
]

describe('joinUsageTokenName', () => {
  it('命中令牌返回名字；多 token 各自映射且同 id 去重', () => {
    const rows = [{ token_id: 1 }, { token_id: 3 }, { token_id: 1 }]
    const m = joinUsageTokenName(rows, tokens)
    expect(m.get(1)).toBe('my-laptop')
    expect(m.get(3)).toBe('pad')
    expect(m.size).toBe(2)
  })

  it('缺失 id 回退 #id', () => {
    const m = joinUsageTokenName([{ token_id: 9 }], tokens)
    expect(m.get(9)).toBe('#9')
    expect(m.has(1)).toBe(false) // 未出现在 rows 中的令牌不入表
  })

  it('revoked 令牌标注（已撤销）', () => {
    const m = joinUsageTokenName([{ token_id: 2 }], tokens)
    expect(m.get(2)).toBe('old-phone（已撤销）')
  })

  it('空 rows 返回空 Map；tokens 缺失时全部回退 #id', () => {
    expect(joinUsageTokenName([], tokens).size).toBe(0)
    const m = joinUsageTokenName([{ token_id: 3 }, { token_id: 4 }], [])
    expect([...m.entries()]).toEqual([
      [3, '#3'],
      [4, '#4'],
    ])
  })
})
