<script setup lang="ts">
// 状态彩点+中文词（spec §2-2）：语义色仅绿/黄/红，灰=中性，主题色=强调
// size（UX-11）：默认 'sm' 保持既有观感；'lg' 用于总览大号状态点（点 size-3 + 文字 xl 加粗）
import type { Tone } from '@/lib/statusLabels'

withDefaults(defineProps<{ tone: Tone; label: string; size?: 'sm' | 'md' | 'lg' }>(), { size: 'sm' })

const DOT_CLASS: Record<Tone, string> = {
  ok: 'bg-emerald-500',
  warn: 'bg-amber-500',
  error: 'bg-red-500',
  muted: 'bg-zinc-400',
  accent: 'bg-primary',
}
</script>

<template>
  <span class="inline-flex items-center gap-1.5">
    <span class="shrink-0 rounded-full" :class="[DOT_CLASS[tone], size === 'lg' ? 'size-3' : 'size-1']" aria-hidden="true" />
    <span :class="size === 'lg' ? 'text-xl font-semibold' : undefined">{{ label }}</span>
  </span>
</template>
