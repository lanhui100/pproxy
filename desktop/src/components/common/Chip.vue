<script setup lang="ts">
// 可选中档位按钮的单一出口（SPEC §12 UX-11）：告警档位 / 有效期 / Tab 切换等
// 「选中态切换 chips」统一走本组件；点击事件经 attrs 透传，组件内不声明 emit。
// 无边框语言：未选=浅灰填充，选中=墨色实底。
withDefaults(defineProps<{ selected: boolean; disabled?: boolean }>(), { disabled: false })
</script>

<template>
  <button
    type="button"
    class="rounded-full px-3 py-1 text-sm transition-colors duration-150"
    :class="
      selected
        ? 'bg-primary font-medium text-primary-foreground'
        : 'bg-muted/70 text-muted-foreground hover:bg-muted hover:text-foreground'
    "
    :aria-pressed="selected ? 'true' : 'false'"
    :disabled="disabled"
  >
    <slot />
  </button>
</template>
