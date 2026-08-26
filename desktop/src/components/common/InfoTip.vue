<script setup lang="ts">
// InfoTip：行内「说明」图标，悬停显示通俗解释（spec：详细解释用信息图标 + hover tooltip）。
// 用于收纳帮助文字，避免界面堆砌工程术语；纯展示组件，无副作用。
import { TooltipContent, TooltipPortal, TooltipProvider, TooltipRoot, TooltipTrigger } from 'reka-ui'
import { Info } from '@lucide/vue'

withDefaults(defineProps<{ text: string; side?: 'top' | 'bottom' | 'left' | 'right' }>(), { side: 'top' })
</script>

<template>
  <TooltipProvider :delay-duration="150" :skip-delay-duration="300">
    <TooltipRoot>
      <TooltipTrigger as-child>
        <button
          type="button"
          class="inline-flex shrink-0 cursor-help items-center rounded-full p-0.5 align-middle text-muted-foreground/60 transition-colors outline-none hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
          :aria-label="'说明：' + text"
        >
          <Info class="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipPortal>
        <TooltipContent
          :side="side"
          :side-offset="6"
          class="data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 z-50 max-w-64 rounded-lg bg-foreground px-3 py-2 text-xs leading-relaxed text-background shadow-[0_4px_20px_rgba(0,0,0,0.12)] duration-100"
        >
          {{ text }}
        </TooltipContent>
      </TooltipPortal>
    </TooltipRoot>
  </TooltipProvider>
</template>
