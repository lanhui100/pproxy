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

  it('enforces flat minimalist style with no box shadow', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 扁平风格，严禁出现任何 shadow 阴影
    expect(content).not.toMatch(/shadow(-\[|-[a-z0-9]+)/)
    expect(content).not.toContain('box-shadow')
  })

  it('harmonizes frosted glass backdrop and border styles', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 毛玻璃特性：backdrop-blur 或 backdrop-filter
    expect(content).toMatch(/backdrop-blur|backdrop-filter/)
  })

  it('aligns text colors with toast semantic kinds instead of plain foreground', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 文本颜色必须与语义统一（emerald, sky, rose），不再使用一刀切的 text-foreground
    expect(content).not.toMatch(/<p[^>]*class="[^"]*text-foreground[^"]*"/)
    expect(content).toMatch(/emerald/)
    expect(content).toMatch(/sky/)
    expect(content).toMatch(/rose/)
  })

  it('provides minimalist center scale transition instead of bottom translation', () => {
    const content = readFileSync(toastHostPath, 'utf-8')
    // 底部上升的 translate-y-3 应当被替换为现代极简的中心缩放或淡入淡出
    expect(content).not.toContain('translate-y-3')
    expect(content).toMatch(/scale-9[0-9]|scale-100/)
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
