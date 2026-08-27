<script setup lang="ts">
// 状态彩点+中文词（spec §2-2）：语义色仅绿/黄/红，灰=中性，主题色=强调
import type { Tone } from '@/lib/statusLabels'

withDefaults(defineProps<{ tone: Tone; label?: string; size?: 'sm' | 'md' | 'lg' }>(), { size: 'sm', label: '' })

const DOT_CLASS: Record<Tone, string> = {
  ok: 'bg-emerald-500',
  warn: 'bg-amber-500',
  error: 'bg-red-500',
  muted: 'bg-zinc-400',
  accent: 'bg-primary',
}

const DOT_SIZE: Record<'sm' | 'md' | 'lg', string> = {
  sm: 'size-1.5',
  md: 'size-2.5',
  lg: 'size-3',
}
</script>

<template>
  <span class="inline-flex items-center gap-1.5">
    <span class="shrink-0 rounded-full" :class="[DOT_CLASS[tone], DOT_SIZE[size]]" aria-hidden="true" />
    <span v-if="label" :class="size === 'lg' ? 'text-xl font-semibold' : undefined">{{ label }}</span>
  </span>
</template>
