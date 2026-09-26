import { describe, expect, it, beforeEach, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { useToast } from '@/composables/useToast'

describe('Toast Component & UX Specification', () => {
  const toastHostPath = resolve(__dirname, '../components/common/ToastHost.vue')

  it('verifies ToastHost is positioned at viewport center and not bottom-anchored', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 不能再是底部悬浮
    expect(content).not.toContain('bottom-6')
    // 必须是窗口正中央（水平与垂直双重居中）
    expect(content).toMatch(/fixed (inset-0|top-1\/2)/)
    expect(content).toMatch(/(items-center justify-center|-translate-x-1\/2 -translate-y-1\/2)/)
    // 确保事件穿透控制正常
    expect(content).toContain('pointer-events-none')
    expect(content).toContain('pointer-events-auto')
  })

  it('enforces gray high-contrast background with subtle border and min-height for 3 lines', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 质感灰色背景，与浅色桌面形成清晰对比度，拒绝白色透明发飘
    expect(content).toMatch(/bg-neutral-(800|900)/)
    // 保证高度至少支持 3 行文字（min-h-[5.25rem] 或更高），避免细长条
    expect(content).toMatch(/min-h-\[(5|5\.25|5\.5|6)rem\]/)
    // 宽度合理，避免极度拉伸
    expect(content).toMatch(/w-\[min\(calc\(100vw-3rem\),(19|20|21|22)rem\)\]/)
  })

  it('adopts frosted glass and rounded card container', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 毛玻璃特性：backdrop-blur 或 backdrop-filter
    expect(content).toMatch(/backdrop-blur|backdrop-filter/)
    // 圆角卡片
    expect(content).toMatch(/rounded-(lg|xl)\s/)
  })

  it('restricts semantic colors strictly to internal icons, keeping card and text neutral', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 卡片本身与文本不再带有语义色彩（emerald/sky/rose 背景和文字色）
    expect(content).not.toMatch(/bg-(emerald|sky|rose)/)
    // 图标仍保留语义色
    expect(content).toMatch(/text-emerald-/)
    expect(content).toMatch(/text-sky-/)
    expect(content).toMatch(/text-rose-/)
  })

  it('provides subtle micro-motion center transition instead of bottom translation', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 底部大幅位移已被移除
    expect(content).not.toContain('translate-y-3')
    // 轻微弹出动效：极小位移与缩放 (scale-90/95 + 微微 translate-y-1)
    expect(content).toMatch(/scale-9[0-9]|scale-100/)
    expect(content).toMatch(/duration-(150|200)/)
  })

  describe('useToast composable functionality', () => {
    beforeEach(() => {
      vi.useFakeTimers()
      const toast = useToast()
      const currentList = [...toast.toasts.value]
      for (const t of currentList) {
        toast.dismiss(t.id)
      }
    })

    it('manages auto-dismiss for success and manual for error', () => {
      const toast = useToast()
      const sId = toast.success('操作成功')
      const eId = toast.error('操作失败', '详细堆栈')

      expect(toast.toasts.value).toHaveLength(2)
      expect(toast.toasts.value.find((t) => t.id === sId)?.kind).toBe('success')
      expect(toast.toasts.value.find((t) => t.id === eId)?.kind).toBe('error')

      // 3 秒后 success 自动消失，error 保留
      vi.advanceTimersByTime(3000)
      expect(toast.toasts.value.find((t) => t.id === sId)).toBeUndefined()
      expect(toast.toasts.value.find((t) => t.id === eId)).toBeDefined()

      // 手动 dismiss error
      toast.dismiss(eId)
      expect(toast.toasts.value).toHaveLength(0)
    })
  })
})
