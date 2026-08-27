<script setup lang="ts">
import { ref } from 'vue'
import { Check, CircleAlert, CircleCheck, Copy, Info, X } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss, dismissAfterCopy } = useToast()

// 纯净单色、极高通透度语义配置（无边框、无渐变、强毛玻璃）
const SEMANTIC: Record<
  ToastKind,
  {
    icon: typeof Info
    iconCls: string
    glassStyle: Record<string, string>
    badgeStyle: Record<string, string>
  }
> = {
  success: {
    icon: CircleCheck,
    iconCls: 'text-emerald-500',
    glassStyle: {
      backgroundColor: 'rgba(16, 185, 129, 0.15)',
    },
    badgeStyle: {
      backgroundColor: 'rgba(16, 185, 129, 0.20)',
    },
  },
  info: {
    icon: Info,
    iconCls: 'text-sky-500',
    glassStyle: {
      backgroundColor: 'rgba(14, 165, 233, 0.15)',
    },
    badgeStyle: {
      backgroundColor: 'rgba(14, 165, 233, 0.20)',
    },
  },
  error: {
    icon: CircleAlert,
    iconCls: 'text-rose-500',
    glassStyle: {
      backgroundColor: 'rgba(244, 63, 94, 0.18)',
    },
    badgeStyle: {
      backgroundColor: 'rgba(244, 63, 94, 0.22)',
    },
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
        class="toast-frosted-card pointer-events-auto flex w-full items-start gap-3 rounded-2xl p-3.5 shadow-[0_16px_40px_rgba(0,0,0,0.16)] transition-all"
        :style="SEMANTIC[t.kind].glassStyle"
      >
        <!-- 语义小圆标 -->
        <div
          class="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full"
          :style="SEMANTIC[t.kind].badgeStyle"
        >
          <component :is="SEMANTIC[t.kind].icon" class="size-4" :class="SEMANTIC[t.kind].iconCls" />
        </div>

        <!-- 文本层级 -->
        <div class="min-w-0 flex-1 pt-0.5">
          <p class="text-xs font-semibold text-foreground leading-snug break-words">
            {{ t.message }}
          </p>
          <p v-if="t.detail && t.detail !== t.message" class="mt-0.5 text-[11px] text-foreground/80 leading-relaxed break-words line-clamp-2">
            {{ t.detail }}
          </p>
          <p v-else-if="t.kind === 'error'" class="mt-0.5 text-[10px] text-foreground/60">
            常驻提示 · 请点击复制反馈
          </p>
        </div>

        <!-- 操作区 -->
        <div class="flex shrink-0 items-center gap-1">
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded-md bg-rose-500/20 px-2 py-0.5 text-[11px] font-medium text-rose-600 dark:text-rose-300 transition-colors hover:bg-rose-500/30 cursor-pointer"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3" />
            <Copy v-else class="size-3" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded-md p-1 text-foreground/40 transition-colors hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10 cursor-pointer"
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

<style scoped>
.toast-frosted-card {
  border: none !important;
  backdrop-filter: blur(28px) saturate(200%);
  -webkit-backdrop-filter: blur(28px) saturate(200%);
}
</style>
