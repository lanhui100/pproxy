<script setup lang="ts">
import { Activity, Gauge, ListTree, Settings, Ticket } from '@lucide/vue'

const nav = [
  { to: '/', label: 'Dashboard', icon: Gauge },
  { to: '/routes', label: 'Routes', icon: ListTree },
  { to: '/tokens', label: 'Tokens', icon: Ticket },
  { to: '/usage', label: 'Usage', icon: Activity },
  { to: '/settings', label: 'Settings', icon: Settings },
]
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
      </RouterLink>
    </nav>
    <main class="flex-1 overflow-auto p-6">
      <RouterView />
    </main>
  </div>
</template>
