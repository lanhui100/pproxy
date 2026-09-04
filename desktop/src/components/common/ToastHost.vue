<script setup lang="ts">
import { ref } from 'vue'
import { Check, CircleAlert, CircleCheck, Copy, Info, X } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss, dismissAfterCopy } = useToast()

// 纯平扁平、无阴影、毛玻璃色彩与语义文字一体化配置
const SEMANTIC: Record<
  ToastKind,
  {
    icon: typeof Info
    containerCls: string
    iconCls: string
    titleCls: string
    detailCls: string
    hintCls: string
    dismissBtnCls: string
  }
> = {
  success: {
    icon: CircleCheck,
    containerCls:
      'bg-emerald-50/85 border-emerald-500/20 text-emerald-950 dark:bg-emerald-950/75 dark:border-emerald-400/30 dark:text-emerald-50',
    iconCls: 'text-emerald-600 dark:text-emerald-400',
    titleCls: 'text-emerald-950 dark:text-emerald-50',
    detailCls: 'text-emerald-800/85 dark:text-emerald-200/85',
    hintCls: 'text-emerald-700/70 dark:text-emerald-300/70',
    dismissBtnCls:
      'text-emerald-700/50 hover:text-emerald-900 hover:bg-emerald-500/15 dark:text-emerald-300/50 dark:hover:text-emerald-100 dark:hover:bg-emerald-400/15',
  },
  info: {
    icon: Info,
    containerCls:
      'bg-sky-50/85 border-sky-500/20 text-sky-950 dark:bg-sky-950/75 dark:border-sky-400/30 dark:text-sky-50',
    iconCls: 'text-sky-600 dark:text-sky-400',
    titleCls: 'text-sky-950 dark:text-sky-50',
    detailCls: 'text-sky-800/85 dark:text-sky-200/85',
    hintCls: 'text-sky-700/70 dark:text-sky-300/70',
    dismissBtnCls:
      'text-sky-700/50 hover:text-sky-900 hover:bg-sky-500/15 dark:text-sky-300/50 dark:hover:text-sky-100 dark:hover:bg-sky-400/15',
  },
  error: {
    icon: CircleAlert,
    containerCls:
      'bg-rose-50/90 border-rose-500/25 text-rose-950 dark:bg-rose-950/80 dark:border-rose-400/30 dark:text-rose-50',
    iconCls: 'text-rose-600 dark:text-rose-400',
    titleCls: 'text-rose-950 dark:text-rose-50',
    detailCls: 'text-rose-800/85 dark:text-rose-200/85',
    hintCls: 'text-rose-700/70 dark:text-rose-300/70',
    dismissBtnCls:
      'text-rose-700/50 hover:text-rose-900 hover:bg-rose-500/15 dark:text-rose-300/50 dark:hover:text-rose-100 dark:hover:bg-rose-400/15',
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
  <!-- 窗口中央浮层容器，背景点击全穿透 -->
  <div
    aria-live="polite"
    class="pointer-events-none fixed inset-0 z-50 flex flex-col items-center justify-center p-4 gap-2.5"
  >
    <TransitionGroup
      enter-active-class="transition duration-200 ease-out"
      enter-from-class="opacity-0 scale-95"
      enter-to-class="opacity-100 scale-100"
      leave-active-class="transition duration-150 ease-in pointer-events-none"
      leave-from-class="opacity-100 scale-100"
      leave-to-class="opacity-0 scale-95"
    >
      <div
        v-for="t in toasts"
        :key="t.id"
        class="toast-frosted-card pointer-events-auto flex w-[min(calc(100vw-3rem),26rem)] items-start gap-3 rounded-2xl border px-3.5 py-3 transition-all select-none"
        :class="SEMANTIC[t.kind].containerCls"
      >
        <!-- 语义图标 -->
        <component
          :is="SEMANTIC[t.kind].icon"
          class="size-4.5 shrink-0 mt-0.5"
          :class="SEMANTIC[t.kind].iconCls"
        />

        <!-- 文本层级（严格与语义色彩体系一致） -->
        <div class="min-w-0 flex-1">
          <p class="text-xs font-semibold leading-snug break-words" :class="SEMANTIC[t.kind].titleCls">
            {{ t.message }}
          </p>
          <p
            v-if="t.detail && t.detail !== t.message"
            class="mt-1 text-[11px] leading-relaxed break-words line-clamp-3"
            :class="SEMANTIC[t.kind].detailCls"
          >
            {{ t.detail }}
          </p>
          <p
            v-else-if="t.kind === 'error'"
            class="mt-1 text-[10px]"
            :class="SEMANTIC[t.kind].hintCls"
          >
            常驻提示 · 请点击复制反馈
          </p>
        </div>

        <!-- 操作区 -->
        <div class="flex shrink-0 items-center gap-1.5 ml-1">
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded-md border border-rose-500/20 bg-rose-500/15 px-2 py-0.5 text-[11px] font-medium text-rose-700 transition-colors hover:bg-rose-500/25 dark:border-rose-400/30 dark:bg-rose-400/20 dark:text-rose-200 dark:hover:bg-rose-400/30 cursor-pointer"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3" />
            <Copy v-else class="size-3" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded-md p-1 transition-colors cursor-pointer"
            :class="SEMANTIC[t.kind].dismissBtnCls"
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
  backdrop-filter: blur(24px) saturate(180%);
  -webkit-backdrop-filter: blur(24px) saturate(180%);
}
</style>
