<script setup lang="ts">
// 全局 toast 渲染层（壳层挂载一次）：消费 useToast 单例队列。
// 靠近底部居中悬浮；语义色高透明毛玻璃底（绿=成功/蓝=提示/红=出错）；
// error 不自动消失，带「复制」一键导出错误详情（粘贴给 AI 排障），复制后延时消失。
import { ref } from 'vue'
import { Check, CircleAlert, CircleCheck, Copy, Info } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss, dismissAfterCopy } = useToast()

// 语义图标 + 语义玻璃底（soft token 高透明 + backdrop-blur）
const SEMANTIC: Record<ToastKind, { icon: typeof Info; iconCls: string; glassCls: string }> = {
  success: { icon: CircleCheck, iconCls: 'text-ok', glassCls: 'bg-ok-soft/70' },
  info: { icon: Info, iconCls: 'text-info', glassCls: 'bg-info-soft/70' },
  error: { icon: CircleAlert, iconCls: 'text-bad', glassCls: 'bg-bad-soft/70' },
}

// 每条 toast 独立的「已复制」态（按钮反馈，随后条目自动消失）
const copiedIds = ref<Set<number>>(new Set())

async function copyError(id: number, text: string): Promise<void> {
  const payload = `Pony Proxy 错误信息（请帮我排查原因）：\n${text}`
  try {
    await navigator.clipboard.writeText(payload)
  } catch {
    // webview 剪贴板受限时的兜底：execCommand
    const ta = document.createElement('textarea')
    ta.value = payload
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    try {
      document.execCommand('copy')
    } catch {
      /* 尽力而为：失败则保持 toast 常驻，用户可手动关闭 */
      document.body.removeChild(ta)
      return
    }
    document.body.removeChild(ta)
  }
  const next = new Set(copiedIds.value)
  next.add(id)
  copiedIds.value = next
  dismissAfterCopy(id)
}
</script>

<template>
  <div
    aria-live="polite"
    class="pointer-events-none fixed bottom-6 left-1/2 z-50 flex w-[min(calc(100vw-3rem),26rem)] -translate-x-1/2 flex-col items-center gap-2"
  >
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="pointer-events-auto flex w-full items-start gap-2.5 rounded-xl py-2.5 pr-1.5 pl-3.5 shadow-[0_8px_32px_rgba(0,0,0,0.12)] backdrop-blur-xl backdrop-saturate-150"
        :class="SEMANTIC[t.kind].glassCls"
      >
        <component :is="SEMANTIC[t.kind].icon" class="mt-0.5 size-4 shrink-0" :class="SEMANTIC[t.kind].iconCls" aria-hidden="true" />
        <div class="min-w-0 flex-1">
          <p class="text-sm leading-5 break-words">{{ t.message }}</p>
          <!-- error 常驻提示：说明为何不消失 -->
          <p v-if="t.kind === 'error'" class="mt-0.5 text-xs text-muted-foreground">不会自动关闭</p>
        </div>
        <div class="flex shrink-0 items-center gap-0.5">
          <!-- 一键复制错误详情（粘贴给 AI 排障）；复制后延时消失 -->
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs font-medium text-bad transition-colors hover:bg-bad/10"
            :aria-label="'复制错误信息'"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3.5" />
            <Copy v-else class="size-3.5" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded-md p-1 leading-none text-muted-foreground/60 transition-colors hover:text-foreground"
            aria-label="关闭提示"
            @click="dismiss(t.id)"
          >
            ×
          </button>
        </div>
      </div>
    </TransitionGroup>
  </div>
</template>
