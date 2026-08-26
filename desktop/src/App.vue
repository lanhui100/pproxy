<script setup lang="ts">
import type { Component } from 'vue'
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Activity, Bell, Gauge, Globe, ListTree, Settings, Ticket } from '@lucide/vue'

import { onUnauthorized } from '@/api/client'
import NeedSetupGuide from '@/components/common/NeedSetupGuide.vue'
import ToastHost from '@/components/common/ToastHost.vue'
import { useAlertNotifications } from '@/composables/useAlertNotifications'
import { useBackendGate } from '@/composables/useBackendGate'
import { checkForUpdate, updateAvailable } from '@/composables/useUpdater'

const router = useRouter()
const route = useRoute()

// 导航数据化（spec §3.1）：badge 按路由挂载，禁止按 label 文案匹配
interface NavItem {
  to: string
  label: string
  icon: Component
  badge?: 'alerts' | 'update'
}

const nav: NavItem[] = [
  { to: '/', label: '总览', icon: Gauge, badge: 'alerts' },
  { to: '/routes', label: '服务', icon: ListTree },
  { to: '/tokens', label: '设备密钥', icon: Ticket },
  { to: '/usage', label: '用量统计', icon: Activity },
  { to: '/proxy', label: '代理', icon: Globe },
  { to: '/settings', label: '设置', icon: Settings, badge: 'update' },
]

// 当前项判定：总览精确匹配，其余前缀匹配（供强调样式用，与 active-class 解耦）
function isActive(to: string): boolean {
  return to === '/' ? route.path === '/' : route.path.startsWith(to)
}

// R7/F8：401 全局拦截 → 跳设置页；提示以一次性 history state 承载（禁 query），
// 固定文案由设置页消费后清除（spec §3.1）
onUnauthorized(() => {
  router.push({ path: '/settings', state: { authInvalidHint: '1' } })
})

// 未配置门槛：未连接网关时内容区只渲染引导卡（页面不发请求）；设置页保持可达
const { configured } = useBackendGate()

// M5 §5：告警通知轮询（F2 起为无参签名，内部读响应式 pollIntervalMin）
const { unreadCount } = useAlertNotifications()
const bannerDismissed = ref(false)
// M5 拓展：启动即检查更新（Tauri 环境；红点挂设置页签）
void checkForUpdate()
</script>

<template>
  <div class="flex h-screen">
    <nav class="w-52 shrink-0 bg-card p-3">
      <div class="mb-5 px-2.5">
        <p class="text-sm font-semibold tracking-tight">Pony Proxy</p>
        <p class="mt-0.5 text-xs text-muted-foreground">个人代理网关</p>
      </div>
      <RouterLink
        v-for="item in nav"
        :key="item.to"
        :to="item.to"
        class="mb-0.5 flex items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-sm transition-colors duration-150"
        :class="
          isActive(item.to)
            ? 'bg-accent font-medium text-foreground'
            : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground'
        "
      >
        <component :is="item.icon" class="size-4 shrink-0" />
        <span class="truncate">{{ item.label }}</span>
        <span
          v-if="item.badge === 'alerts' && unreadCount > 0"
          class="ml-auto rounded-full bg-bad px-1.5 text-xs leading-5 text-white"
        >
          {{ unreadCount > 99 ? '99+' : unreadCount }}
        </span>
        <span
          v-else-if="item.badge === 'update' && updateAvailable"
          class="ml-auto size-2 shrink-0 rounded-full bg-red-500"
          title="有新版本"
        />
      </RouterLink>
    </nav>
    <main class="flex-1 overflow-auto p-6 lg:p-8">
      <div class="mx-auto max-w-3xl">
        <NeedSetupGuide v-if="!configured && route.path !== '/settings'" />
        <template v-else>
          <div
            v-if="unreadCount > 0 && !bannerDismissed"
            class="mb-6 flex items-center gap-2 rounded-lg bg-warn-soft px-3 py-2 text-sm text-warn"
          >
            <Bell class="size-4 shrink-0" aria-hidden="true" />
            有 {{ unreadCount }} 条未读告警
            <RouterLink to="/" class="font-medium underline underline-offset-2">前往处理</RouterLink>
            <button
              type="button"
              class="ml-auto rounded p-0.5 transition-colors hover:text-foreground"
              aria-label="关闭横幅"
              @click="bannerDismissed = true"
            >
              ×
            </button>
          </div>
          <RouterView />
        </template>
      </div>
    </main>
  </div>
  <!-- 全局 toast 层：仅挂载一次 -->
  <ToastHost />
</template>
