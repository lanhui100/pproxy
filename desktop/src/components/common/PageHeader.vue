<script setup lang="ts">
// 页头：标题+副题（左）、操作区 slot（右）；一屏一事的主任务锚点
defineProps<{ title: string; subtitle?: string }>()
</script>

<template>
  <div class="mb-6 flex items-start justify-between gap-4">
    <div class="min-w-0">
      <h1 class="text-lg font-semibold tracking-tight">{{ title }}</h1>
      <p v-if="subtitle" class="mt-0.5 text-sm text-muted-foreground">{{ subtitle }}</p>
    </div>
    <div v-if="$slots.actions" class="flex shrink-0 items-center gap-2">
      <slot name="actions" />
    </div>
  </div>
</template>
