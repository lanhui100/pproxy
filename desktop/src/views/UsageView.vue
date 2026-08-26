<script setup lang="ts">
// 用量统计（SPEC §3.7）：时间档 segmented + 按服务柱图 + 上游额度进度 + 请求明细表。
// 刷新契约：Promise.all([usage(hours), quota, listTokens]) 三路并行重取不缓存；
// listTokens 失败不阻塞 usage 展示（catch 后传空数组，明细回退 #id）。
import { computed, onMounted, ref } from 'vue'
import type { ChartOptions } from 'chart.js'
import { BarElement, BarController, CategoryScale, Chart as ChartJS, Legend, LinearScale } from 'chart.js'
import { Bar } from 'vue-chartjs'
import { RouterLink } from 'vue-router'

import { api, type QuotaResp, type TokenDto, type UsageResp } from '@/api/client'
import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonCard from '@/components/common/SkeletonCard.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { errText } from '@/lib/errors'
import { fmtBytes, fmtCount } from '@/lib/format'
import { upstreamLabel } from '@/lib/statusLabels'
import { joinUsageTokenName } from '@/lib/usageJoin'

ChartJS.register(CategoryScale, LinearScale, BarElement, BarController, Legend)

// 图表强调色静态常量（spec §3.7：定值 hex，禁读 CSS 变量进 canvas）
const BAR_COLOR = '#0f766e'

const RANGE_OPTIONS = [
  { hours: 24, label: '近 24 小时' },
  { hours: 168, label: '近 7 天' },
  { hours: 720, label: '近 30 天' },
] as const

// quota metric 枚举 → 中文（命名见 crates/core/src/quota.rs），未收录值原样展示
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
      api.listTokens().catch(() => null), // 仅影响密钥名列的 join，不阻塞主数据
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

onMounted(refresh)

// 首拉未落地 → 骨架；首拉失败 → 整页错误态（重试）
const firstLoading = computed(() => usage.value === null && error.value === '')
const showFatal = computed(() => usage.value === null && error.value !== '' && !loading.value)

const rows = computed(() => usage.value?.rows ?? [])

// 按服务聚合请求数（降序）画柱图
const byService = computed(() => {
  const m = new Map<string, number>()
  for (const r of rows.value) m.set(r.route, (m.get(r.route) ?? 0) + r.requests)
  return [...m.entries()].sort((a, b) => b[1] - a[1])
})

const chartData = computed(() => ({
  labels: byService.value.map(([name]) => name),
  datasets: [{ label: '请求次数', data: byService.value.map(([, n]) => n), backgroundColor: BAR_COLOR }],
}))

// tooltip 中文化；y 轴整数刻度
const chartOptions: ChartOptions<'bar'> = {
  responsive: true,
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

// 明细表「设备密钥」列：纯函数 join（usageJoin.test.ts 钉住回退与 revoked 标注）
const tokenNames = computed(() => joinUsageTokenName(rows.value, tokens.value))
const tokenName = (id: number): string => tokenNames.value.get(id) ?? `#${id}`

function metricLabel(metric: string): string {
  return METRIC_LABELS[metric] ?? metric
}
</script>

<template>
  <div>
    <PageHeader title="用量统计" subtitle="请求量、流量与上游额度">
      <template #actions>
        <!-- 时间档 segmented：当前档高亮（浅灰槽内白色滑块） -->
        <div class="flex gap-0.5 rounded-lg bg-muted/70 p-0.5 text-sm" role="group" aria-label="统计区间">
          <button
            v-for="opt in RANGE_OPTIONS"
            :key="opt.hours"
            type="button"
            class="rounded-md px-2.5 py-1 transition-colors duration-150"
            :class="
              hours === opt.hours
                ? 'bg-card font-medium shadow-[0_1px_3px_rgba(0,0,0,0.08)]'
                : 'text-muted-foreground hover:text-foreground'
            "
            :disabled="loading"
            @click="setHours(opt.hours)"
          >
            {{ opt.label }}
          </button>
        </div>
      </template>
    </PageHeader>

    <!-- 首次加载骨架：双卡 + 明细表 -->
    <template v-if="firstLoading">
      <div class="grid gap-4 lg:grid-cols-5">
        <SkeletonCard class="lg:col-span-3" />
        <SkeletonCard class="lg:col-span-2" />
      </div>
      <div class="mt-8">
        <SkeletonTable :rows="4" />
      </div>
    </template>

    <!-- 首拉失败且无数据：整页错误态 -->
    <div v-else-if="showFatal" class="rounded-xl bg-bad-soft px-4 py-10 text-center">
      <p class="mx-auto max-w-lg break-all text-sm text-bad">{{ error }}</p>
      <Button class="mt-4" variant="outline" size="sm" :disabled="loading" @click="refresh">重试</Button>
    </div>

    <template v-else>
      <!-- 刷新失败的横幅（已有旧数据时叠加展示） -->
      <div
        v-if="error"
        class="mb-6 flex items-center gap-3 rounded-lg bg-bad-soft px-3 py-2 text-sm text-bad"
      >
        <span class="min-w-0 flex-1 break-all">{{ error }}</span>
        <Button variant="outline" size="sm" :disabled="loading" @click="refresh">重试</Button>
      </div>

      <div class="grid gap-4 lg:grid-cols-5">
        <!-- 柱图 -->
        <Card class="lg:col-span-3">
          <CardContent>
            <div class="text-sm font-medium tracking-tight">各服务请求次数</div>
            <div class="mt-3">
              <Bar v-if="byService.length > 0" :data="chartData" :options="chartOptions" />
              <EmptyState v-else title="区间内暂无请求" description="有设备开始访问后，这里会出现按服务的请求柱图。">
                <template #actions>
                  <Button as-child>
                    <RouterLink to="/routes">去添加服务</RouterLink>
                  </Button>
                </template>
              </EmptyState>
            </div>
          </CardContent>
        </Card>

        <!-- 上游额度 -->
        <Card class="lg:col-span-2">
          <CardContent>
            <div class="flex items-center gap-1 text-sm font-medium tracking-tight">
              上游额度
              <InfoTip text="中转线路的免费用量额度，触顶后会临时限流，次日恢复" />
            </div>
            <div v-if="(quota?.snapshots ?? []).length > 0" class="mt-3 space-y-4">
              <div v-for="s in quota?.snapshots ?? []" :key="`${s.upstream}/${s.metric}`">
                <div class="mb-1 flex items-center justify-between gap-2 text-sm">
                  <span class="min-w-0 truncate">{{ upstreamLabel(s.upstream).label }} · {{ metricLabel(s.metric) }}</span>
                  <span v-if="s.pct >= 0" class="shrink-0 tabular-nums text-muted-foreground">已用 {{ fmtCount(s.used) }}</span>
                </div>
                <template v-if="s.pct >= 0">
                  <!-- 三色阈值沿用既有逻辑：<80 绿 / ≥80 黄 / ≥95 红 -->
                  <div class="h-1.5 overflow-hidden rounded-full bg-muted">
                    <div
                      class="h-full rounded-full transition-all duration-150"
                      :class="s.pct >= 95 ? 'bg-bad' : s.pct >= 80 ? 'bg-warn' : 'bg-ok'"
                      :style="{ width: `${Math.min(s.pct, 100)}%` }"
                    />
                  </div>
                  <div class="mt-1 text-xs tabular-nums text-muted-foreground">
                    上限 {{ fmtCount(s.quota) }} · 已达 {{ s.pct.toFixed(1) }}%
                  </div>
                </template>
                <p v-else class="text-xs text-muted-foreground">无固定上限，已用 {{ fmtCount(s.used) }}</p>
              </div>
            </div>
            <p v-else class="mt-3 text-sm text-muted-foreground">暂无额度数据（未启用上游监控或暂不支持）</p>
          </CardContent>
        </Card>
      </div>

      <!-- 请求明细 -->
      <section class="mt-8">
        <h2 class="mb-3 text-sm font-semibold tracking-tight">请求明细</h2>
        <EmptyState v-if="rows.length === 0" title="暂无请求明细" description="当前时间范围内还没有请求记录。">
          <template #actions>
            <Button as-child>
              <RouterLink to="/routes">去添加服务</RouterLink>
            </Button>
          </template>
        </EmptyState>
        <Card v-else class="py-1">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="pl-4">服务</TableHead>
                <TableHead>设备密钥</TableHead>
                <TableHead class="text-right">请求次数</TableHead>
                <TableHead class="text-right">上行流量</TableHead>
                <TableHead class="pr-4 text-right">下行流量</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="r in rows" :key="`${r.route}/${r.token_id}`" class="border-b-border/60 last:border-b-0">
                <TableCell class="pl-4 font-medium">{{ r.route }}</TableCell>
                <TableCell>{{ tokenName(r.token_id) }}</TableCell>
                <TableCell class="text-right tabular-nums">{{ fmtCount(r.requests) }}</TableCell>
                <TableCell class="text-right tabular-nums">{{ fmtBytes(r.bytes_in) }}</TableCell>
                <TableCell class="pr-4 text-right tabular-nums">{{ fmtBytes(r.bytes_out) }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </Card>
      </section>
    </template>
  </div>
</template>
