<script setup lang="ts">
// 状态彩点+中文词（spec §2-2）：语义色仅绿/黄/红，灰=中性，主题色=强调
import type { Tone } from '@/lib/statusLabels'

defineProps<{ tone: Tone; label: string }>()

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
    <span class="size-1 shrink-0 rounded-full" :class="DOT_CLASS[tone]" aria-hidden="true" />
    <span>{{ label }}</span>
  </span>
</template>
