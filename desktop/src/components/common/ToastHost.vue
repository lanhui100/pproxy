<script setup lang="ts">
// 全局 toast 渲染层（壳层挂载一次）：消费 useToast 单例队列。
// 居中顶部悬浮、毛玻璃高透明底、语义图标区分成功/提示/出错；
// 进出场动画复用 main.css 的 toast keyframes（TransitionGroup name="toast"）。
import { CircleAlert, CircleCheck, Info } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss } = useToast()

// 语义图标 + 语义着色：绿=成功 / 蓝=提示 / 红=出错
const SEMANTIC: Record<ToastKind, { icon: typeof Info; cls: string }> = {
  success: { icon: CircleCheck, cls: 'text-ok' },
  info: { icon: Info, cls: 'text-info' },
  error: { icon: CircleAlert, cls: 'text-bad' },
}
</script>

<template>
  <div
    aria-live="polite"
    class="pointer-events-none fixed top-5 left-1/2 z-50 flex w-[min(calc(100vw-3rem),24rem)] -translate-x-1/2 flex-col items-center gap-2"
  >
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="pointer-events-auto flex w-full items-start gap-2.5 rounded-xl bg-popover/70 py-2.5 pr-1.5 pl-3.5 shadow-[0_8px_32px_rgba(0,0,0,0.10)] backdrop-blur-xl backdrop-saturate-150 dark:bg-popover/75"
      >
        <component :is="SEMANTIC[t.kind].icon" class="mt-0.5 size-4 shrink-0" :class="SEMANTIC[t.kind].cls" aria-hidden="true" />
        <p class="min-w-0 flex-1 text-sm leading-5 break-words">{{ t.message }}</p>
        <button
          type="button"
          class="shrink-0 rounded-md p-1 leading-none text-muted-foreground/60 transition-colors hover:text-foreground"
          aria-label="关闭提示"
          @click="dismiss(t.id)"
        >
          ×
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>
