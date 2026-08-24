<script setup lang="ts">
// 二次确认弹窗（危险操作保留，spec §2-5）：基于 ui/dialog 组装。
// destructive 时确认钮红色；busy 时禁用两钮并转圈、隐藏右上角关闭钮；
// Esc/遮罩关闭由 DialogRoot 托管，照常经 update:open(false) 上报。
import { LoaderCircle } from '@lucide/vue'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

withDefaults(
  defineProps<{
    open: boolean
    title: string
    description?: string
    confirmText?: string
    destructive?: boolean
    busy?: boolean
  }>(),
  { confirmText: '确认', destructive: false, busy: false },
)

const emit = defineEmits<{ 'update:open': [value: boolean]; confirm: [] }>()
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent :show-close-button="!busy" class="sm:max-w-sm">
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription v-if="description">{{ description }}</DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button variant="outline" :disabled="busy" @click="emit('update:open', false)">取消</Button>
        <Button
          :variant="destructive ? 'destructive' : 'default'"
          :disabled="busy"
          data-icon="inline-start"
          @click="emit('confirm')"
        >
          <LoaderCircle v-if="busy" class="animate-spin" />
          {{ confirmText }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
