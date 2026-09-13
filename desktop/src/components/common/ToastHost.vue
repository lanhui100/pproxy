<script setup lang="ts">
import { ref } from 'vue'
import { Check, CircleAlert, CircleCheck, Copy, Info, X } from '@lucide/vue'

import { useToast, type ToastKind } from '@/composables/useToast'

const { toasts, dismiss, dismissAfterCopy } = useToast()

// 语义配置：背景与文本统一采用中性白色透明毛玻璃；语义颜色仅保留在内部图标及错误操作按钮
const SEMANTIC_ICONS: Record<
  ToastKind,
  {
    icon: typeof Info
    iconCls: string
  }
> = {
  success: {
    icon: CircleCheck,
    iconCls: 'text-emerald-500 dark:text-emerald-400',
  },
  info: {
    icon: Info,
    iconCls: 'text-sky-500 dark:text-sky-400',
  },
  error: {
    icon: CircleAlert,
    iconCls: 'text-rose-500 dark:text-rose-400',
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
    class="pointer-events-none fixed inset-0 z-50 flex flex-col items-center justify-center p-4 gap-2"
  >
    <TransitionGroup
      enter-active-class="transition duration-200 cubic-bezier(0.16, 1, 0.3, 1)"
      enter-from-class="opacity-0 scale-90 translate-y-1"
      enter-to-class="opacity-100 scale-100 translate-y-0"
      leave-active-class="transition duration-150 ease-in pointer-events-none"
      leave-from-class="opacity-100 scale-100 translate-y-0"
      leave-to-class="opacity-0 scale-95 translate-y-0.5"
    >
      <div
        v-for="t in toasts"
        :key="t.id"
        class="toast-frosted-card pointer-events-auto flex w-[min(calc(100vw-3rem),19rem)] items-start gap-2.5 rounded-lg px-3 py-2.5 select-none"
      >
        <!-- 语义图标（仅此处使用语义色彩） -->
        <component
          :is="SEMANTIC_ICONS[t.kind].icon"
          class="size-4 shrink-0 mt-0.5"
          :class="SEMANTIC_ICONS[t.kind].iconCls"
        />

        <!-- 文本层级（中性克制配色，不再带有语义背景色/文本色） -->
        <div class="min-w-0 flex-1">
          <p class="text-xs font-medium leading-snug break-words text-neutral-800 dark:text-neutral-100">
            {{ t.message }}
          </p>
          <p
            v-if="t.detail && t.detail !== t.message"
            class="mt-0.5 text-[11px] leading-relaxed break-words line-clamp-3 text-neutral-500 dark:text-neutral-400"
          >
            {{ t.detail }}
          </p>
          <p
            v-else-if="t.kind === 'error'"
            class="mt-0.5 text-[10px] text-neutral-400 dark:text-neutral-500"
          >
            常驻提示 · 请点击复制反馈
          </p>
        </div>

        <!-- 操作区 -->
        <div class="flex shrink-0 items-center gap-1 ml-0.5">
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium text-neutral-600 hover:text-neutral-900 dark:text-neutral-300 dark:hover:text-neutral-100 hover:bg-black/5 dark:hover:bg-white/10 transition-colors cursor-pointer"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3 text-emerald-500" />
            <Copy v-else class="size-3" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded p-0.5 text-neutral-400 hover:text-neutral-700 dark:text-neutral-500 dark:hover:text-neutral-200 hover:bg-black/5 dark:hover:bg-white/10 transition-colors cursor-pointer"
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
/*
 * 白色透明毛玻璃效果，去除边框：
 * - 亮色模式：半透明纯白底 + 强模糊 backdrop-blur
 * - 暗色模式：高通透白/灰毛玻璃底，与暗色背景融合
 */
.toast-frosted-card {
  background-color: rgba(255, 255, 255, 0.78);
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
}

:global(.dark) .toast-frosted-card {
  background-color: rgba(255, 255, 255, 0.12);
}
</style>
