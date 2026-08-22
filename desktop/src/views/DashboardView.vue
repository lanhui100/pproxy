<script setup lang="ts">
// Dashboard（spec §4）：状态灯 + 路由健康 + 近 24h 合计 + 未读告警处理 + quota 徽标
import { computed, onMounted, ref } from 'vue'
import { RefreshCw } from '@lucide/vue'

import { api, errorMessage, type AlertDto, type HealthResp, type QuotaResp, type UsageResp } from '@/api/client'

const health = ref<HealthResp | null>(null)
const usage = ref<UsageResp | null>(null)
const quota = ref<QuotaResp | null>(null)
const unread = ref<AlertDto[]>([])
const error = ref('')
const loading = ref(false)
const marking = ref<number | null>(null)

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [h, u, q, a] = await Promise.all([
      api.health(),
      api.usage({ hours: 24 }),
      api.quota(),
      api.alerts(true, 50),
    ])
    health.value = h
    usage.value = u
    quota.value = q
    unread.value = a.alerts
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

async function markRead(id: number): Promise<void> {
  marking.value = id
  try {
    await api.markAlertRead(id)
    unread.value = unread.value.filter((a) => a.id !== id)
  } catch (e) {
    // 已读幂等：404/重复均视为已达成，仅真实错误上屏
    error.value = errorMessage(e)
  } finally {
    marking.value = null
  }
}

const dbOk = computed(() => health.value?.db === 'ok')
const fmtBytes = (n: number): string => {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(2)} GB`
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KB`
  return `${n} B`
}
const stateBadge = (s: string): string =>
  s === 'ok' ? 'bg-emerald-100 text-emerald-800' : s === 'error' ? 'bg-red-100 text-red-800' : 'bg-zinc-100 text-zinc-700'

onMounted(refresh)
</script>

<template>
  <div>
    <div class="mb-4 flex items-center justify-between">
      <h1 class="text-xl font-semibold">Dashboard</h1>
      <button
        class="flex items-center gap-1 rounded-md border px-2 py-1 text-sm hover:bg-accent disabled:opacity-50"
        :disabled="loading"
        @click="refresh"
      >
        <RefreshCw class="size-3.5" :class="{ 'animate-spin': loading }" />
        刷新
      </button>
    </div>

    <p v-if="error" class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ error }}</p>

    <div class="grid grid-cols-3 gap-4">
      <div class="rounded-lg border p-4">
        <div class="text-xs text-muted-foreground">服务状态</div>
        <div class="mt-1 flex items-center gap-2">
          <span class="inline-block size-2.5 rounded-full" :class="dbOk ? 'bg-emerald-500' : 'bg-red-500'" />
          <span class="text-lg font-medium">{{ health?.status ?? '—' }}</span>
        </div>
        <div class="mt-1 text-xs text-muted-foreground">活跃 token {{ health?.tokens_active ?? '—' }}</div>
      </div>
      <div class="rounded-lg border p-4">
        <div class="text-xs text-muted-foreground">近 24h 请求</div>
        <div class="mt-1 text-lg font-medium">{{ usage?.total.requests ?? '—' }}</div>
        <div class="mt-1 text-xs text-muted-foreground">↑{{ fmtBytes(usage?.total.bytes_in ?? 0) }} ↓{{ fmtBytes(usage?.total.bytes_out ?? 0) }}</div>
      </div>
      <div class="rounded-lg border p-4">
        <div class="text-xs text-muted-foreground">采集来源</div>
        <div class="mt-2 flex flex-wrap gap-1.5">
          <span
            v-for="s in quota?.sources ?? []"
            :key="s.name"
            class="rounded-full px-2 py-0.5 text-xs"
            :class="stateBadge(s.state)"
          >{{ s.name }}: {{ s.state }}</span>
          <span v-if="!quota" class="text-sm text-muted-foreground">—</span>
        </div>
      </div>
    </div>

    <h2 class="mb-2 mt-6 text-sm font-semibold">路由健康（{{ Object.keys(health?.routes ?? {}).length }}）</h2>
    <div class="flex flex-wrap gap-2">
      <span
        v-for="(cfg, name) in health?.routes ?? {}"
        :key="name"
        class="rounded-md border px-2 py-1 text-xs"
        :class="cfg.enabled ? '' : 'opacity-50'"
      >
        {{ name }}
        <span class="ml-1 text-muted-foreground">{{ cfg.upstream }}</span>
        <span v-if="!cfg.enabled" class="ml-1 text-destructive">disabled</span>
      </span>
      <span v-if="!health" class="text-sm text-muted-foreground">—</span>
    </div>

    <h2 class="mb-2 mt-6 text-sm font-semibold">未读告警（{{ unread.length }}）</h2>
    <div v-if="unread.length === 0" class="text-sm text-muted-foreground">暂无未读告警</div>
    <ul v-else class="space-y-2">
      <li
        v-for="a in unread"
        :key="a.id"
        class="flex items-center justify-between rounded-md border px-3 py-2 text-sm"
        :class="a.level === 'critical' ? 'border-red-300 bg-red-50' : 'border-amber-300 bg-amber-50'"
      >
        <span>[{{ a.level }}] {{ a.message }}</span>
        <button
          class="ml-3 shrink-0 rounded border px-2 py-0.5 text-xs hover:bg-accent disabled:opacity-50"
          :disabled="marking === a.id"
          @click="markRead(a.id)"
        >
          标记已读
        </button>
      </li>
    </ul>
  </div>
</template>
