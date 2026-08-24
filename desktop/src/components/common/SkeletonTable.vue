<script setup lang="ts">
// 表格加载骨架：头部 + rows 行灰块，宽度交错避免机械感
withDefaults(defineProps<{ rows?: number }>(), { rows: 5 })

// 行首块宽度循环取用，制造自然节奏
const LEAD_WIDTHS = ['w-1/5', 'w-1/4', 'w-1/6', 'w-1/4'] as const
</script>

<template>
  <div class="overflow-hidden rounded-lg border" aria-hidden="true">
    <div class="animate-skeleton">
      <div class="border-b bg-muted/50 px-4 py-2.5">
        <div class="h-3 w-16 rounded bg-muted" />
      </div>
      <div
        v-for="i in rows"
        :key="i"
        class="flex items-center gap-4 border-b px-4 py-3 last:border-b-0"
      >
        <div class="h-3 rounded bg-muted" :class="LEAD_WIDTHS[i % LEAD_WIDTHS.length]" />
        <div class="h-3 flex-1 rounded bg-muted" />
        <div class="h-3 w-14 rounded bg-muted" />
      </div>
    </div>
  </div>
</template>
