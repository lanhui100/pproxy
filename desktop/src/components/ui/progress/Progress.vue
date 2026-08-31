<script setup lang="ts">
import type { ProgressRootProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import {
  ProgressIndicator,
  ProgressRoot,
} from 'reka-ui'
import { cn } from '@/lib/utils'

const props = withDefaults(
  defineProps<ProgressRootProps & {
    class?: HTMLAttributes['class']
    indicatorClass?: HTMLAttributes['class']
    indeterminate?: boolean
  }>(),
  {
    modelValue: 0,
    indeterminate: false,
  },
)

const delegatedProps = reactiveOmit(props, 'class', 'indicatorClass', 'indeterminate')
</script>

<template>
  <ProgressRoot
    v-bind="delegatedProps"
    :class="
      cn(
        'relative h-2 w-full overflow-hidden rounded-full bg-muted/80',
        props.class,
      )
    "
  >
    <ProgressIndicator
      v-if="!props.indeterminate"
      :class="
        cn(
          'h-full w-full flex-1 bg-primary rounded-full transition-all duration-300 ease-out',
          props.indicatorClass,
        )
      "
      :style="`transform: translateX(-${100 - Math.min(100, Math.max(0, props.modelValue ?? 0))}%);`"
    />
    <div
      v-else
      class="h-full w-full relative overflow-hidden bg-muted/60"
    >
      <div
        :class="
          cn(
            'absolute inset-y-0 rounded-full bg-primary',
            props.indicatorClass,
          )
        "
        style="animation: progress-indeterminate-slide 1.4s cubic-bezier(0.4, 0, 0.2, 1) infinite;"
      />
    </div>
  </ProgressRoot>
</template>

<style scoped>
@keyframes progress-indeterminate-slide {
  0% {
    left: -40%;
    width: 40%;
  }
  50% {
    left: 25%;
    width: 60%;
  }
  100% {
    left: 100%;
    width: 40%;
  }
}
</style>
