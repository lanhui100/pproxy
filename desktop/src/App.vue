<script setup lang="ts">
import type { Component } from 'vue'
import { onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { Gauge, Settings } from '@lucide/vue'

import ToastHost from '@/components/common/ToastHost.vue'
import { checkForUpdate, updateAvailable } from '@/composables/useUpdater'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { loadAutoProxyConfig, saveAutoProxyConfig, isTauri } from '@/lib/config'

const route = useRoute()

// 导航数据化：badge 按路由挂载，禁止按 label 文案匹配
interface NavItem {
  to: string
  label: string
  icon: Component
  badge?: 'update'
}

const nav: NavItem[] = [
  { to: '/', label: '仪表盘', icon: Gauge },
  { to: '/settings', label: '设置', icon: Settings, badge: 'update' },
]

// 当前项判定：总览精确匹配，其余前缀匹配（供强调样式用，与 active-class 解耦）
function isActive(to: string): boolean {
  return to === '/' ? route.path === '/' : route.path.startsWith(to)
}

// M5 拓展：启动即检查更新（Tauri 环境；红点挂设置页签）
void checkForUpdate()

// ---- Fix4 T5：首次启动 auto_proxy 询问对话框 ----
const showAutoProxyAsk = ref(false)
const dontAskAgain = ref(false)

onMounted(async () => {
  try {
    const cfg = await loadAutoProxyConfig()
    // 若 app_config 无 auto_proxy 字段且未勾选 dont_ask，弹询问
    if (cfg.auto_proxy === undefined && !cfg.dont_ask) {
      // 老用户迁移：whitelist 非空但 config 缺失也视为首次询问（已满足 auto_proxy===undefined）
      showAutoProxyAsk.value = true
    }
  } catch { /* 忽略 */ }
  // 托盘隐藏气球已由 Rust 侧直接 show_balloon，前端仅可选监听 window-hidden-to-tray
  if (isTauri()) {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen('window-hidden-to-tray', () => {
        // 空实现：避免重复气球
      })
    } catch {}
  }
})

async function handleAutoProxyChoice(enable: boolean): Promise<void> {
  const patch: Record<string, unknown> = { auto_proxy: enable }
  if (dontAskAgain.value) patch.dont_ask = true
  try { await saveAutoProxyConfig(patch as never) } catch {}
  showAutoProxyAsk.value = false
  if (enable && isTauri()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_enable')
    } catch {}
  }
}
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
          v-if="item.badge === 'update' && updateAvailable"
          class="ml-auto size-2 shrink-0 rounded-full bg-red-500"
          title="有新版本"
        />
      </RouterLink>
    </nav>
    <main class="flex-1 overflow-auto p-6 lg:p-8">
      <div class="mx-auto max-w-4xl">
        <RouterView />
      </div>
    </main>
  </div>
  <!-- 全局 toast 层：仅挂载一次 -->
  <ToastHost />
  <!-- 首次启动 auto_proxy 询问 -->
  <Dialog :open="showAutoProxyAsk" @update:open="(v:boolean)=> !v && (showAutoProxyAsk=false)">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader>
        <DialogTitle>是否自动开启系统代理？</DialogTitle>
        <DialogDescription>检测到可自动开启系统代理以修复连接问题，是否开启？可在设置页随时关闭。</DialogDescription>
      </DialogHeader>
      <label class="flex items-center gap-2 text-sm">
        <input type="checkbox" v-model="dontAskAgain" class="rounded" />
        下次不再询问
      </label>
      <DialogFooter>
        <Button variant="outline" @click="handleAutoProxyChoice(false)">暂不开启</Button>
        <Button @click="handleAutoProxyChoice(true)">开启并记住</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
