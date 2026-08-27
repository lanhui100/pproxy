<script setup lang="ts">
import { computed } from 'vue'
import {
  formatSlotTooltip,
  getLatencyTone,
  padLatencySlots,
  type LatencyPoint,
  type LatencyTone,
} from '@/lib/latencyHistory'

const props = withDefaults(
  defineProps<{
    history?: LatencyPoint[]
    fastThreshold?: number
    warnThreshold?: number
  }>(),
  {
    history: () => [],
    fastThreshold: 800,
    warnThreshold: 2000,
  },
)

const slots = computed(() => padLatencySlots(props.history, 12))

const BAR_COLOR: Record<LatencyTone, string> = {
  ok: 'bg-emerald-500 hover:opacity-80',
  warn: 'bg-amber-500 hover:opacity-80',
  error: 'bg-red-500 hover:opacity-80',
  empty: 'bg-muted/60',
}
</script>

<template>
  <div class="inline-flex items-center gap-1" role="group" aria-label="近 1 小时连通性时序">
    <div
      v-for="(slot, idx) in slots"
      :key="idx"
      class="h-3.5 w-1.5 rounded-xs transition-all duration-150 cursor-pointer"
      :class="BAR_COLOR[getLatencyTone(slot, fastThreshold, warnThreshold)]"
      :title="formatSlotTooltip(slot)"
    />
  </div>
</template>
