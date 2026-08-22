<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { Activity, Bell, Gauge, ListTree, Settings, Ticket } from '@lucide/vue'

import { onUnauthorized } from '@/api/client'
import { loadPollIntervalMin } from '@/lib/config'
import { useAlertNotifications } from '@/composables/useAlertNotifications'
import { updateAvailable, checkForUpdate } from '@/composables/useUpdater'

const router = useRouter()

const nav = [
  { to: '/', label: 'Dashboard', icon: Gauge },
  { to: '/routes', label: 'Routes', icon: ListTree },
  { to: '/tokens', label: 'Tokens', icon: Ticket },
  { to: '/usage', label: 'Usage', icon: Activity },
  { to: '/settings', label: 'Settings', icon: Settings },
]

// R7/F8：401 全局拦截 → 单次导航 Settings（豁免/去抖在 client 层）
onUnauthorized(() => {
  router.push('/settings')
})

// M5 §5：告警通知轮询（默认 5min，可配）；未读数驱动侧栏徽标与横幅
const { unreadCount } = useAlertNotifications(() => loadPollIntervalMin())
const bannerDismissed = ref(false)
// M5 拓展：启动即检查更新（Tauri 环境；badge 挂 Settings 页签）
void checkForUpdate()
</script>

<template>
  <div class="flex h-screen">
    <nav class="w-48 shrink-0 border-r p-3">
      <div class="mb-4 px-2 text-sm font-semibold">pony-desktop</div>
      <RouterLink
        v-for="item in nav"
        :key="item.to"
        :to="item.to"
        class="mb-1 flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground"
        active-class="bg-accent text-accent-foreground"
      >
        <component :is="item.icon" class="size-4" />
        {{ item.label }}
        <span
          v-if="item.label === 'Dashboard' && unreadCount > 0"
          class="ml-auto rounded-full bg-destructive px-1.5 text-xs text-white"
        >
          {{ unreadCount }}
        </span>
        <span
          v-else-if="item.label === 'Settings' && updateAvailable"
          class="ml-auto inline-block size-2 rounded-full bg-red-500"
          title="有新版本"
        />
      </RouterLink>
    </nav>
    <main class="flex-1 overflow-auto p-6">
      <div
        v-if="unreadCount > 0 && !bannerDismissed"
        class="mb-4 flex items-center gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm dark:border-amber-700 dark:bg-amber-950"
      >
        <Bell class="size-4 text-amber-600" />
        有 {{ unreadCount }} 条未读告警
        <RouterLink to="/" class="underline">前往处理</RouterLink>
        <button class="ml-auto text-muted-foreground hover:text-foreground" @click="bannerDismissed = true">×</button>
      </div>
      <RouterView />
    </main>
  </div>
</template>
