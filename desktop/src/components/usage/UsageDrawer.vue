<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { ChartOptions } from 'chart.js'
import { BarElement, BarController, CategoryScale, Chart as ChartJS, Legend, LinearScale } from 'chart.js'
import { Bar } from 'vue-chartjs'
import { X, RefreshCw } from '@lucide/vue'

import { api, type QuotaResp, type TokenDto, type UsageResp } from '@/api/client'
import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { errText } from '@/lib/errors'
import { fmtBytes, fmtCount } from '@/lib/format'
import { upstreamLabel } from '@/lib/statusLabels'
import { joinUsageTokenName } from '@/lib/usageJoin'

const props = defineProps<{
  open: boolean
}>()

const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
}>()

ChartJS.register(CategoryScale, LinearScale, BarElement, BarController, Legend)

const BAR_COLOR = '#0f766e'

const RANGE_OPTIONS = [
  { hours: 24, label: '近 24 小时' },
  { hours: 168, label: '近 7 天' },
  { hours: 720, label: '近 30 天' },
] as const

const METRIC_LABELS: Record<string, string> = {
  requests_daily: '当日请求数',
  bandwidth: '带宽用量',
  function_invocations: '函数调用次数',
}

const hours = ref<number>(24)
const usage = ref<UsageResp | null>(null)
const quota = ref<QuotaResp | null>(null)
const tokens = ref<TokenDto[]>([])
const error = ref('')
const loading = ref(false)

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [u, q, t] = await Promise.all([
      api.usage({ hours: hours.value }),
      api.quota(),
      api.listTokens().catch(() => null),
    ])
    usage.value = u
    quota.value = q
    tokens.value = t?.tokens ?? []
  } catch (e) {
    error.value = errText(e)
  } finally {
    loading.value = false
  }
}

function setHours(h: number): void {
  if (hours.value === h) return
  hours.value = h
  void refresh()
}

function close(): void {
  emit('update:open', false)
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape' && props.open) {
    close()
  }
}

onMounted(() => {
  if (typeof window !== 'undefined') {
    window.addEventListener('keydown', onKeydown)
  }
})

onUnmounted(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('keydown', onKeydown)
  }
})

watch(
  () => props.open,
  (v) => {
    if (v) {
      void refresh()
    }
  },
)

const rows = computed(() => usage.value?.rows ?? [])

const byService = computed(() => {
  const m = new Map<string, number>()
  for (const r of rows.value) m.set(r.route, (m.get(r.route) ?? 0) + r.requests)
  return [...m.entries()].sort((a, b) => b[1] - a[1])
})

const chartData = computed(() => ({
  labels: byService.value.map(([name]) => name),
  datasets: [{ label: '请求次数', data: byService.value.map(([, n]) => n), backgroundColor: BAR_COLOR }],
}))

const chartOptions: ChartOptions<'bar'> = {
  responsive: true,
  maintainAspectRatio: false,
  plugins: {
    legend: { display: false },
    tooltip: {
      callbacks: {
        label: (item) => `请求 ${fmtCount(item.parsed.y ?? 0)} 次`,
      },
    },
  },
  scales: {
    y: { beginAtZero: true, ticks: { precision: 0 } },
  },
}

const tokenNames = computed(() => joinUsageTokenName(rows.value, tokens.value))
const tokenName = (id: number): string => tokenNames.value.get(id) ?? `#${id}`

function metricLabel(metric: string): string {
  return METRIC_LABELS[metric] ?? metric
}
</script>

<template>
  <Transition
    enter-active-class="transition duration-200 ease-out"
    enter-from-class="opacity-0"
    enter-to-class="opacity-100"
    leave-active-class="transition duration-150 ease-in"
    leave-from-class="opacity-100"
    leave-to-class="opacity-0"
  >
    <div
      v-if="open"
      class="fixed inset-0 z-40 bg-black/40 backdrop-blur-xs"
      @click="close"
    />
  </Transition>

  <Transition
    enter-active-class="transition duration-300 ease-out transform"
    enter-from-class="translate-x-full"
    enter-to-class="translate-x-0"
    leave-active-class="transition duration-200 ease-in transform"
    leave-from-class="translate-x-0"
    leave-to-class="translate-x-full"
  >
    <aside
      v-if="open"
      class="fixed inset-y-0 right-0 z-50 flex w-full max-w-2xl flex-col bg-background shadow-2xl border-l border-border"
      role="dialog"
      aria-modal="true"
      aria-labelledby="drawer-title"
    >
      <div class="flex items-center justify-between border-b border-border/70 px-6 py-4">
        <div>
          <h2 id="drawer-title" class="text-base font-semibold tracking-tight">用量与流量分析</h2>
          <p class="mt-0.5 text-xs text-muted-foreground">统计历史请求量、流量消耗与上游中转明细</p>
        </div>
        <div class="flex items-center gap-2">
          <Button variant="ghost" size="icon-sm" :disabled="loading" title="刷新" @click="refresh">
            <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />
          </Button>
          <Button variant="ghost" size="icon-sm" title="关闭" @click="close">
            <X class="size-4" />
          </Button>
        </div>
      </div>

      <div class="flex items-center justify-between border-b border-border/50 bg-muted/30 px-6 py-2.5">
        <div class="flex gap-0.5 rounded-lg bg-muted/70 p-0.5 text-xs" role="group" aria-label="统计区间">
          <button
            v-for="opt in RANGE_OPTIONS"
            :key="opt.hours"
            type="button"
            class="rounded-md px-2.5 py-1 transition-colors duration-150"
            :class="
              hours === opt.hours
                ? 'bg-card font-medium text-foreground shadow-[0_1px_2px_rgba(0,0,0,0.06)]'
                : 'text-muted-foreground hover:text-foreground'
            "
            :disabled="loading"
            @click="setHours(opt.hours)"
          >
            {{ opt.label }}
          </button>
        </div>
        <span v-if="loading" class="text-xs text-muted-foreground animate-pulse">加载中…</span>
      </div>

      <div class="flex-1 overflow-y-auto p-6 space-y-6 transition-opacity duration-200" :class="{ 'opacity-60 pointer-events-none': loading }">
        <div v-if="error" class="rounded-lg bg-bad-soft px-3 py-2 text-sm text-bad flex items-center justify-between">
          <span>{{ error }}</span>
          <Button variant="outline" size="xs" @click="refresh">重试</Button>
        </div>

        <Card>
          <CardContent class="p-4">
            <div class="flex items-center justify-between">
              <span class="text-xs font-medium text-muted-foreground">各服务请求次数分布</span>
            </div>
            <div class="mt-3 h-52">
              <Bar v-if="byService.length > 0" :data="chartData" :options="chartOptions" />
              <EmptyState v-else title="区间内暂无请求" description="有设备通过代理网关访问后，这里将展示请求柱图。" />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent class="p-4">
            <div class="flex items-center gap-1 text-xs font-medium text-muted-foreground">
              上游中转额度
              <InfoTip text="中转线路的免费用量额度，触顶后会临时限流，次日恢复" />
            </div>
            <div v-if="(quota?.snapshots ?? []).length > 0" class="mt-3 space-y-3">
              <div v-for="s in quota?.snapshots ?? []" :key="`${s.upstream}/${s.metric}`">
                <div class="mb-1 flex items-center justify-between text-xs">
                  <span class="font-medium">{{ upstreamLabel(s.upstream).label }} · {{ metricLabel(s.metric) }}</span>
                  <span v-if="s.pct >= 0" class="tabular-nums text-muted-foreground">已用 {{ fmtCount(s.used) }} / {{ fmtCount(s.quota) }}</span>
                </div>
                <template v-if="s.pct >= 0">
                  <div class="h-1.5 overflow-hidden rounded-full bg-muted">
                    <div
                      class="h-full rounded-full transition-all duration-150"
                      :class="s.pct >= 95 ? 'bg-bad' : s.pct >= 80 ? 'bg-warn' : 'bg-ok'"
                      :style="{ width: `${Math.min(s.pct, 100)}%` }"
                    />
                  </div>
                  <div class="mt-1 flex justify-end text-[11px] tabular-nums text-muted-foreground">
                    已达 {{ s.pct.toFixed(1) }}%
                  </div>
                </template>
                <p v-else class="text-[11px] text-muted-foreground">无固定上限，已用 {{ fmtCount(s.used) }}</p>
              </div>
            </div>
            <p v-else class="mt-2 text-xs text-muted-foreground">暂无额度数据</p>
          </CardContent>
        </Card>

        <div>
          <h3 class="mb-2 text-xs font-semibold tracking-tight text-foreground">请求明细</h3>
          <SkeletonTable v-if="loading && rows.length === 0" :rows="3" />
          <EmptyState v-else-if="rows.length === 0" title="暂无明细记录" description="当前时间段内还没有产生代理请求记录。" />
          <Card v-else class="overflow-hidden">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead class="pl-3 text-xs">服务</TableHead>
                  <TableHead class="text-xs">设备密钥</TableHead>
                  <TableHead class="text-right text-xs">请求数</TableHead>
                  <TableHead class="text-right text-xs">上行</TableHead>
                  <TableHead class="pr-3 text-right text-xs">下行</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="r in rows" :key="`${r.route}/${r.token_id}`" class="text-xs border-b border-border/40">
                  <TableCell class="pl-3 font-medium">{{ r.route }}</TableCell>
                  <TableCell class="text-muted-foreground">{{ tokenName(r.token_id) }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{ fmtCount(r.requests) }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{ fmtBytes(r.bytes_in) }}</TableCell>
                  <TableCell class="pr-3 text-right tabular-nums">{{ fmtBytes(r.bytes_out) }}</TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </Card>
        </div>
      </div>
    </aside>
  </Transition>
</template>
