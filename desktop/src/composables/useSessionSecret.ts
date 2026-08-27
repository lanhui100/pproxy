import { ref } from 'vue'

interface StoredSecret {
  plain: string
  name: string
  expiresAt: number
}

// 内存单例暂存器：页面关闭或应用退出即灰飞烟灭
const cachedSecret = ref<StoredSecret | null>(null)

const TTL_MS = 15 * 60 * 1000 // 15 分钟

export function useSessionSecret() {
  function setSessionSecret(plain: string, name = 'default'): void {
    if (!plain) {
      cachedSecret.value = null
      return
    }
    cachedSecret.value = {
      plain,
      name,
      expiresAt: Date.now() + TTL_MS,
    }
  }

  function getSessionSecret(): string | null {
    if (!cachedSecret.value) return null
    if (Date.now() > cachedSecret.value.expiresAt) {
      cachedSecret.value = null
      return null
    }
    return cachedSecret.value.plain
  }

  function clearSessionSecret(): void {
    cachedSecret.value = null
  }

  return {
    setSessionSecret,
    getSessionSecret,
    clearSessionSecret,
  }
}
