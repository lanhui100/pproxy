<script setup lang="ts">
// 全局未配置门槛（spec §3.1）：baseUrl 为空时统一渲染此卡，不发任何请求；
// CTA 进设置向导。无 props——文案固定，避免各页漂移。
import { RouterLink } from 'vue-router'

import { Button } from '@/components/ui/button'
import EmptyState from './EmptyState.vue'
</script>

<template>
  <EmptyState title="先连接你的网关" description="完成设置后即可管理服务与设备密钥。">
    <template #actions>
      <Button as-child>
        <RouterLink to="/settings">去连接</RouterLink>
      </Button>
    </template>
  </EmptyState>
</template>
