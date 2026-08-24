<script setup lang="ts">
// 可选中档位按钮的单一出口（SPEC §12 UX-11）：告警档位 / 有效期 / Tab 切换等
// 「选中态切换 chips」统一走本组件；点击事件经 attrs 透传，组件内不声明 emit。
withDefaults(defineProps<{ selected: boolean; disabled?: boolean }>(), { disabled: false })
</script>

<template>
  <button
    type="button"
    class="rounded-full border px-3 py-1 text-sm transition-colors duration-150"
    :class="
      selected
        ? 'border-primary bg-primary/10 font-medium text-primary'
        : 'border-border text-muted-foreground hover:bg-muted hover:text-foreground'
    "
    :aria-pressed="selected ? 'true' : 'false'"
    :disabled="disabled"
  >
    <slot />
  </button>
</template>
