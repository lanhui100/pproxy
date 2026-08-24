// format 纯函数测试：重点覆盖非法输入回退与单位换算边界（spec §6.2）
import { describe, expect, it } from 'vitest'

import { alertLevelLabel, fmtBytes, fmtCount, fmtDate, fmtDateTime, fmtRelative } from './format'

describe('fmtBytes', () => {
  it('0 → 0 B，B 区间取整', () => {
    expect(fmtBytes(0)).toBe('0 B')
    expect(fmtBytes(512)).toBe('512 B')
    expect(fmtBytes(1023)).toBe('1023 B')
  })

  it('KB/MB/GB 换算：<10 保 2 位小数，否则 1 位', () => {
    expect(fmtBytes(1024)).toBe('1.00 KB')
    expect(fmtBytes(1536)).toBe('1.50 KB')
    expect(fmtBytes(5 * 1024 * 1024)).toBe('5.00 MB')
    expect(fmtBytes(12.5 * 1024 * 1024)).toBe('12.5 MB')
    expect(fmtBytes(3.5 * 1024 ** 3)).toBe('3.50 GB')
  })

  it('超过 GB 上限不再进位', () => {
    expect(fmtBytes(1024 ** 4)).toBe('1024.0 GB')
  })

  it('负数/NaN/Infinity → —', () => {
    expect(fmtBytes(-1)).toBe('—')
    expect(fmtBytes(Number.NaN)).toBe('—')
    expect(fmtBytes(Number.POSITIVE_INFINITY)).toBe('—')
  })
})

describe('fmtCount', () => {
  it('<10000 千分位', () => {
    expect(fmtCount(0)).toBe('0')
    expect(fmtCount(999)).toBe('999')
    expect(fmtCount(9999)).toBe('9,999')
  })

  it('≥10000 → x.x 万', () => {
    expect(fmtCount(10000)).toBe('1.0 万')
    expect(fmtCount(12345)).toBe('1.2 万')
    expect(fmtCount(234567)).toBe('23.5 万')
  })

  it('负数/NaN → —', () => {
    expect(fmtCount(-5)).toBe('—')
    expect(fmtCount(Number.NaN)).toBe('—')
  })
})

describe('fmtDate / fmtDateTime', () => {
  // 用本地时间分量构造，测试与展示均不依赖时区
  it('YYYY-MM-DD 与 YYYY-MM-DD HH:mm（补零）', () => {
    const day = new Date(2024, 0, 5).getTime()
    expect(fmtDate(day)).toBe('2024-01-05')
    const withTime = new Date(2024, 4, 20, 9, 5).getTime()
    expect(fmtDateTime(withTime)).toBe('2024-05-20 09:05')
  })

  it('无效时间戳 → —', () => {
    expect(fmtDate(Number.NaN)).toBe('—')
    expect(fmtDateTime(Number.NaN)).toBe('—')
  })
})

describe('fmtRelative', () => {
  const now = new Date(2024, 5, 10, 12, 0, 0).getTime()

  it('刚刚 / N分钟前 / N小时前', () => {
    expect(fmtRelative(now - 30_000, now)).toBe('刚刚')
    expect(fmtRelative(now - 5 * 60_000, now)).toBe('5分钟前')
    expect(fmtRelative(now - 3 * 3_600_000, now)).toBe('3小时前')
  })

  it('24–48 小时 → 昨天；超 2 天回退绝对日期', () => {
    expect(fmtRelative(now - 30 * 3_600_000, now)).toBe('昨天')
    expect(fmtRelative(now - 72 * 3_600_000, now)).toBe(fmtDate(now - 72 * 3_600_000))
  })

  it('未来时间戳与 NaN 回退 fmtDate', () => {
    const future = now + 3_600_000
    expect(fmtRelative(future, now)).toBe(fmtDate(future))
    expect(fmtRelative(Number.NaN, now)).toBe('—')
  })
})

describe('alertLevelLabel', () => {
  it('两态中文映射', () => {
    expect(alertLevelLabel('warning')).toBe('警告')
    expect(alertLevelLabel('critical')).toBe('严重')
  })
})
