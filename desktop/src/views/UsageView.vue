<script setup lang="ts">
// Usage（spec §4）：hours 选择 + 按 route 聚合柱图（vue-chartjs）+ token 维度表
// + quota 进度条（pct=-1 → "未知上限"哨兵语义，R3）
import { computed, onMounted, ref } from 'vue'

import { BarElement, CategoryScale, Chart as ChartJS, Legend, LinearScale, BarController } from 'chart.js'
import { Bar } from 'vue-chartjs'

import { api, errorMessage, type QuotaResp, type UsageResp } from '@/api/client'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Button } from '@/components/ui/button'

ChartJS.register(CategoryScale, LinearScale, BarElement, BarController, Legend)

const hours = ref(24)
const usage = ref<UsageResp | null>(null)
const quota = ref<QuotaResp | null>(null)
const error = ref('')
const loading = ref(false)

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    ;[usage.value, quota.value] = await Promise.all([api.usage({ hours: hours.value }), api.quota()])
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

function setHours(h: number): void {
  hours.value = h
  void refresh()
}

// 按 route 聚合 requests
const byRoute = computed(() => {
  const m = new Map<string, number>()
  for (const r of usage.value?.rows ?? []) m.set(r.route, (m.get(r.route) ?? 0) + r.requests)
  return [...m.entries()].sort((a, b) => b[1] - a[1])
})

const chartData = computed(() => ({
  labels: byRoute.value.map(([r]) => r),
  datasets: [
    {
      label: 'requests',
      data: byRoute.value.map(([, n]) => n),
      backgroundColor: '#3b82f6',
    },
  ],
}))

const chartOptions = { responsive: true, plugins: { legend: { display: false } } }

const fmtBytes = (n: number): string => {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(2)} GB`
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KB`
  return `${n} B`
}

onMounted(refresh)
</script>

<template>
  <div>
    <div class="mb-4 flex items-center justify-between">
      <h1 class="text-xl font-semibold">Usage</h1>
      <div class="flex gap-1">
        <Button v-for="h in [24, 168, 720]" :key="h" :variant="hours === h ? 'default' : 'outline'" size="sm" @click="setHours(h)">
          {{ h }}h
        </Button>
      </div>
    </div>

    <p v-if="error" class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ error }}</p>

    <div class="grid grid-cols-2 gap-4">
      <Card>
        <CardHeader><CardTitle class="text-sm">按路由聚合（requests）</CardTitle></CardHeader>
        <CardContent>
          <Bar v-if="byRoute.length" :data="chartData" :options="chartOptions" />
          <p v-else class="text-sm text-muted-foreground">区间内无数据</p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader><CardTitle class="text-sm">上游限额进度</CardTitle></CardHeader>
        <CardContent class="space-y-4">
          <div v-for="s in quota?.snapshots ?? []" :key="`${s.upstream}/${s.metric}`">
            <div class="mb-1 flex items-center justify-between text-sm">
              <span>{{ s.upstream }} · {{ s.metric }}</span>
              <span class="text-muted-foreground">{{ s.used.toLocaleString() }}</span>
            </div>
            <template v-if="s.pct >= 0">
              <div class="h-2 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                <div
                  class="h-full rounded-full"
                  :class="s.pct >= 95 ? 'bg-red-500' : s.pct >= 80 ? 'bg-amber-500' : 'bg-emerald-500'"
                  :style="{ width: `${Math.min(s.pct, 100)}%` }"
                />
              </div>
              <div class="mt-0.5 text-xs text-muted-foreground">{{ s.pct.toFixed(1) }}% of {{ s.quota.toLocaleString() }}</div>
            </template>
            <p v-else class="text-xs text-muted-foreground">未知上限（仅记录用量，不评估告警）</p>
          </div>
        </CardContent>
      </Card>
    </div>

    <h2 class="mb-2 mt-6 text-sm font-semibold">按 token 维度</h2>
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>路由</TableHead>
          <TableHead>token_id</TableHead>
          <TableHead>requests</TableHead>
          <TableHead>bytes_in</TableHead>
          <TableHead>bytes_out</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="r in usage?.rows ?? []" :key="`${r.route}/${r.token_id}`">
          <TableCell>{{ r.route }}</TableCell>
          <TableCell>{{ r.token_id }}</TableCell>
          <TableCell>{{ r.requests }}</TableCell>
          <TableCell>{{ fmtBytes(r.bytes_in) }}</TableCell>
          <TableCell>{{ fmtBytes(r.bytes_out) }}</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
