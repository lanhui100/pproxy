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
        class="toast-frosted-card pointer-events-auto flex w-[min(calc(100vw-3rem),21rem)] min-h-[5.25rem] items-start gap-3 rounded-xl p-3.5 select-none bg-neutral-900/90 text-neutral-100 dark:bg-neutral-800/95 backdrop-blur-xl shadow-lg border border-white/10"
      >
        <!-- 语义图标（仅此处使用语义色彩） -->
        <component
          :is="SEMANTIC_ICONS[t.kind].icon"
          class="size-5 shrink-0 mt-0.5"
          :class="SEMANTIC_ICONS[t.kind].iconCls"
        />

        <!-- 文本层级（深灰色/中性灰色质感底色，至少容纳三行文字，避免细长条） -->
        <div class="min-w-0 flex-1 flex flex-col justify-center min-h-[3.25rem] py-0.5">
          <p class="text-xs font-semibold leading-relaxed break-words text-neutral-100">
            {{ t.message }}
          </p>
          <p
            v-if="t.detail && t.detail !== t.message"
            class="mt-1 text-[11px] leading-relaxed break-words line-clamp-3 text-neutral-300"
          >
            {{ t.detail }}
          </p>
          <p
            v-else-if="t.kind === 'error'"
            class="mt-1 text-[11px] text-neutral-400"
          >
            常驻提示 · 请点击复制反馈
          </p>
          <p
            v-else
            class="mt-0.5 text-[11px] text-neutral-400 opacity-80"
          >
            系统操作提示
          </p>
        </div>

        <!-- 操作区 -->
        <div class="flex shrink-0 items-center gap-1 mt-0.5">
          <button
            v-if="t.kind === 'error'"
            type="button"
            class="flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-neutral-200 hover:text-white bg-white/10 hover:bg-white/20 transition-colors cursor-pointer"
            @click="copyError(t.id, t.detail ?? t.message)"
          >
            <Check v-if="copiedIds.has(t.id)" class="size-3.5 text-emerald-400" />
            <Copy v-else class="size-3.5" />
            {{ copiedIds.has(t.id) ? '已复制' : '复制' }}
          </button>
          <button
            type="button"
            class="rounded-md p-1 text-neutral-400 hover:text-neutral-100 hover:bg-white/10 transition-colors cursor-pointer"
            title="关闭提示"
            @click="dismiss(t.id)"
          >
            <X class="size-4" />
          </button>
        </div>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
/*
 * 灰色毛玻璃卡片质感效果：
 * 同时在 class 与 scoped style 提供双重兜底保证 WebView2 兼容
 */
.toast-frosted-card {
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  backdrop-filter: blur(20px) saturate(180%);
}
</style>
