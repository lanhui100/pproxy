<script setup lang="ts">
import { ref } from 'vue'
import { Check, CircleAlert, CircleCheck, Copy, Info, X } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss, dismissAfterCopy } = useToast()

// 语义配色与微光毛玻璃材质
const SEMANTIC: Record<
  ToastKind,
  {
    icon: typeof Info
    iconCls: string
    iconBg: string
    borderCls: string
    bgGradient: string
  }
> = {
  success: {
    icon: CircleCheck,
    iconCls: 'text-emerald-500 dark:text-emerald-400',
    iconBg: 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400',
    borderCls: 'border-emerald-500/20 dark:border-emerald-400/25',
    bgGradient: 'bg-gradient-to-r from-emerald-500/10 via-background/85 to-background/90 dark:from-emerald-950/30 dark:via-zinc-950/85 dark:to-zinc-950/90',
  },
  info: {
    icon: Info,
    iconCls: 'text-sky-500 dark:text-sky-400',
    iconBg: 'bg-sky-500/15 text-sky-600 dark:text-sky-400',
    borderCls: 'border-sky-500/20 dark:border-sky-400/25',
    bgGradient: 'bg-gradient-to-r from-sky-500/10 via-background/85 to-background/90 dark:from-sky-950/30 dark:via-zinc-950/85 dark:to-zinc-950/90',
  },
  error: {
    icon: CircleAlert,
    iconCls: 'text-rose-500 dark:text-rose-400',
    iconBg: 'bg-rose-500/15 text-rose-600 dark:text-rose-400',
    borderCls: 'border-rose-500/30 dark:border-rose-400/35',
    bgGradient: 'bg-gradient-to-r from-rose-500/15 via-background/90 to-background/90 dark:from-rose-950/40 dark:via-zinc-950/90 dark:to-zinc-950/90',
  },
}

const copiedIds = ref<Set<number>>(new Set())

async function copyError(id: number, text: string): Promise<void> {
  const payload = `Pony Proxy 错误详情：\n${text}`
  try {
    await navigator.clipboard.writeText(payload)
  } catch {
    const ta = document.createElement('textarea')
    ta.value = payload
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    try {
      document.execCommand('copy')
    } catch {
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
    class="pointer-events-none fixed bottom-6 left-1/2 z-50 flex w-[min(calc(100vw-2rem),24rem)] -translate-x-1/2 flex-col items-center gap-2"
  >
    <TransitionGroup
      enter-active-class="transition duration-300 ease-out"
      enter-from-class="opacity-0 translate-y-3 scale-95"
      enter-to-class="opacity-100 translate-y-0 scale-100"
      leave-active-class="transition duration-200 ease-in"
      leave-from-class="opacity-100 translate-y-0 scale-100"
      leave-to-class="opacity-0 translate-y-2 scale-95"
    >
      <div
        v-for="t in toasts"
        :key="t.id"
        class="pointer-events-auto flex w-full items-start gap-3 rounded-2xl border p-3 shadow-[0_16px_40px_rgba(0,0,0,0.14)] backdrop-blur-2xl backdrop-saturate-200 transition-all"
        :class="[SEMANTIC[t.kind].borderCls, SEMANTIC[t.kind].bgGradient]"
      >
        <!-- 语义小圆标 -->
        <div class="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full" :class="SEMANTIC[t.kind].iconBg">
          <component :is="SEMANTIC[t.kind].icon" class="size-4" :class="SEMANTIC[t.kind].iconCls" />
        </div>

        <!-- 文本层级 -->
        <div class="min-w-0 flex-1 pt-0.5">
          <p class="text-xs font-semibold text-foreground leading-snug break-words">
            {{ t.message }}
          </p>
          <p v-if="t.detail && t.detail !== t.message" class="mt-0.5 text-[11px] text-muted-foreground leading-relaxed break-words line-clamp-2">
            {{ t.detail }}
          </p>
          <p v-else-if="t.kind === 'error'" class="mt-0.5 text-[10px] text-muted-foreground/70">
            常驻提示 · 请点击复制反馈
          </p>
        </div>

        <!-- 操作区 -->
        <div class="flex shrink-0 items-center gap-1">
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded-md bg-rose-500/10 px-2 py-0.5 text-[11px] font-medium text-rose-500 transition-colors hover:bg-rose-500/20 cursor-pointer"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3" />
            <Copy v-else class="size-3" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded-md p-1 text-muted-foreground/50 transition-colors hover:text-foreground hover:bg-muted/40 cursor-pointer"
            title="关闭提示"
            @click="dismiss(t.id)"
          >
            <X class="size-3.5" />
          </button>
        </div>
      </div>
    </TransitionGroup>
  </div>
</template>
