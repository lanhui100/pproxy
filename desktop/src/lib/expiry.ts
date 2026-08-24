// 有效期档位 → expires_days 解析（ENG-1 唯一出口）。
// 服务端语义：请求体缺 expires_days 键 = None = 永不过期；
// 因此除 never 外任何档位都必须显式产出天数，杜绝「选了 30/90 天却创建永久密钥」。
export type ExpiryMode = 'never' | 'd30' | 'd90' | 'custom'

export function resolveExpiresDays(mode: ExpiryMode, customDays: number): number | undefined {
  switch (mode) {
    case 'never':
      return undefined
    case 'd30':
      return 30
    case 'd90':
      return 90
    case 'custom':
      if (!Number.isInteger(customDays) || customDays <= 0) {
        throw new Error('有效期不合法')
      }
      return customDays
  }
}
