<script setup lang="ts">
// 卡片加载骨架：animate-skeleton 灰块（keyframes 见 main.css），形状对齐典型卡片
</script>

<template>
  <div class="animate-skeleton space-y-3 rounded-lg border p-4" aria-hidden="true">
    <div class="h-4 w-24 rounded bg-muted" />
    <div class="h-3 w-full rounded bg-muted" />
    <div class="h-3 w-2/3 rounded bg-muted" />
  </div>
</template>
