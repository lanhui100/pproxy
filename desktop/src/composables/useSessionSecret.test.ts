import { describe, expect, it } from 'vitest'
import { useSessionSecret } from './useSessionSecret'

describe('useSessionSecret', () => {
  it('存入与取出会话密钥', () => {
    const { setSessionSecret, getSessionSecret, clearSessionSecret } = useSessionSecret()
    clearSessionSecret()
    expect(getSessionSecret()).toBeNull()

    setSessionSecret('tok_secret_123', 'laptop')
    expect(getSessionSecret()).toBe('tok_secret_123')

    clearSessionSecret()
    expect(getSessionSecret()).toBeNull()
  })
})
