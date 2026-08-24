<script setup lang="ts">
// 空态：说明现状 + slot actions 给唯一下一步动作（spec §2-3）
defineProps<{ title: string; description?: string }>()
</script>

<template>
  <div class="rounded-lg border border-dashed px-6 py-10 text-center">
    <p class="text-sm font-medium">{{ title }}</p>
    <p v-if="description" class="mx-auto mt-1 max-w-md text-sm text-muted-foreground">{{ description }}</p>
    <div v-if="$slots.actions" class="mt-4 flex items-center justify-center gap-2">
      <slot name="actions" />
    </div>
  </div>
</template>
