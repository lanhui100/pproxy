<script setup lang="ts">
// 全局 toast 渲染层（壳层挂载一次）：消费 useToast 单例队列。
// 右下角固定层，aria-live="polite"；进出场动画复用 main.css 的 toast keyframes
// （TransitionGroup name="toast" → .toast-enter-active 等）。
import { useToast } from '@/composables/useToast'

const { toasts, dismiss } = useToast()

// 反馈分级着色：success 绿边 / info 灰边 / error 红边
const EDGE_CLASS = {
  success: 'border-l-2 border-l-emerald-500',
  info: 'border-l-2 border-l-zinc-400',
  error: 'border-l-2 border-l-red-500',
} as const
</script>

<template>
  <div
    aria-live="polite"
    class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-80 max-w-[calc(100vw-2rem)] flex-col gap-2"
  >
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="pointer-events-auto flex items-start gap-2 rounded-md border border-border bg-popover py-2 pr-1.5 pl-3 text-sm shadow-sm"
        :class="EDGE_CLASS[t.kind]"
      >
        <p class="min-w-0 flex-1 leading-5 break-words">{{ t.message }}</p>
        <button
          type="button"
          class="shrink-0 rounded p-1 leading-none text-muted-foreground transition-colors hover:text-foreground"
          aria-label="关闭提示"
          @click="dismiss(t.id)"
        >
          ×
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>
