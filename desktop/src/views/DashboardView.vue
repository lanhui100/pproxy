<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Activity, Bell, Loader2, RefreshCw, Zap } from '@lucide/vue'

import { api, type AlertDto, type HealthResp, type QuotaResp, type UsageResp } from '@/api/client'
import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonCard from '@/components/common/SkeletonCard.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import UsageDrawer from '@/components/usage/UsageDrawer.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { useAdaptivePoll } from '@/composables/useAdaptivePoll'
import { useToast } from '@/composables/useToast'
import { errText } from '@/lib/errors'
import { fmtBytes, fmtCount, fmtRelative } from '@/lib/format'
import { alertLevelView, quotaSourceLabel, upstreamLabel, type Tone } from '@/lib/statusLabels'

const toast = useToast()

const health = ref<HealthResp | null>(null)
const usage = ref<UsageResp | null>(null)
const quota = ref<QuotaResp | null>(null)
const unread = ref<AlertDto[]>([])
const error = ref('')
const markingId = ref<number | null>(null)
const markAllBusy = ref(false)
const showUsageDrawer = ref(false)

// 真实出口与服务连通性探活结果
interface ProbeResult {
  ok: boolean
  ms?: number
  err?: string
}
const upstreamProbes = ref<Record<string, ProbeResult>>({})
const routeProbes = ref<Record<string, ProbeResult>>({})
const probingRoute = ref<string>('')

async function probeAllRoutes(h: HealthResp): Promise<void> {
  const entries = Object.entries(h.routes || {})
  for (const [name, cfg] of entries) {
    if (!cfg.enabled) continue
    const up = cfg.upstream || 'worker'
    const upKey = up === 'worker' ? 'cf' : up

    try {
      const res = await api.testRoute(name, { skipAuthRedirect: true })
      const result: ProbeResult = {
        ok: res.ok,
        ms: res.latency_ms ?? undefined,
        err: res.error ? errText(res.error) : undefined,
      }
      routeProbes.value[name] = result

      // 同时更新上游出口的代表性探活结果
      if (!upstreamProbes.value[upKey] || result.ok) {
        upstreamProbes.value[upKey] = result
      }
    } catch (e) {
      const result: ProbeResult = {
        ok: false,
        err: errText(e),
      }
      routeProbes.value[name] = result
      if (!upstreamProbes.value[upKey]) {
        upstreamProbes.value[upKey] = result
      }
    }
  }
}

async function testSingleRouteInDashboard(name: string): Promise<void> {
  probingRoute.value = name
  try {
    const res = await api.testRoute(name, { skipAuthRedirect: true })
    routeProbes.value[name] = {
      ok: res.ok,
      ms: res.latency_ms ?? undefined,
      err: res.error ? errText(res.error) : undefined,
    }
  } catch (e) {
    routeProbes.value[name] = {
      ok: false,
      err: errText(e),
    }
  } finally {
    probingRoute.value = ''
  }
}

async function pollDashboard(): Promise<void> {
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

    // 后台轻量触发服务与出口真实连通性探活
    void probeAllRoutes(h)
  } catch (e) {
    error.value = errText(e)
    throw e
  }
}

// 接入自适应轮询：15 秒基础间隔，后台休眠与退避保护
const { start, refreshNow, isPolling } = useAdaptivePoll(pollDashboard, {
  baseIntervalMs: 15_000,
  maxIntervalMs: 120_000,
  immediate: true,
})

start()

const firstLoading = computed(() => health.value === null && error.value === '')
const showFatal = computed(() => health.value === null && error.value !== '')

// 状态卡
const dbView = computed(() =>
  health.value?.db === 'ok' ? { label: '网关运行正常', tone: 'ok' as Tone } : { label: '网关异常', tone: 'error' as Tone },
)

// 上游出口健康度聚合
const quotaSources = computed(() => quota.value?.sources ?? [])

function getUpstreamCardView(sourceName: string, sourceState: string): { label: string; tone: Tone; tip?: string } {
  const probe = upstreamProbes.value[sourceName]
  if (probe && probe.ok) {
    return {
      label: `畅通 · ${probe.ms ?? 0}ms`,
      tone: 'ok',
      tip: sourceState === 'unsupported_plan' ? '真实中转连通畅通；Vercel 免费版不提供用量查询 API' : undefined,
    }
  }
  if (probe && !probe.ok) {
    return {
      label: `出口异常: ${probe.err || '超时'}`,
      tone: 'error',
    }
  }
  const qv = quotaSourceLabel(sourceState)
  return { label: qv.label, tone: qv.tone }
}

// 告警
const sortedAlerts = computed(() =>
  [...unread.value].sort((a, b) => {
    const wa = a.level === 'critical' ? 1 : 0
    const wb = b.level === 'critical' ? 1 : 0
    return wa !== wb ? wb - wa : b.id - a.id
  }),
)

async function markRead(alert: AlertDto): Promise<void> {
  markingId.value = alert.id
  try {
    await api.markAlertRead(alert.id)
    unread.value = unread.value.filter((a) => a.id !== alert.id)
  } catch (e) {
    toast.error(errText(e))
  } finally {
    markingId.value = null
  }
}

async function markAllRead(): Promise<void> {
  markAllBusy.value = true
  try {
    const list = (await api.alerts(true, 500)).alerts
    const results = await Promise.allSettled(list.map((a) => api.markAlertRead(a.id)))
    const doneIds = new Set<number>()
    results.forEach((r, idx) => {
      if (r.status === 'fulfilled') {
        const item = list[idx]
        if (item) doneIds.add(item.id)
      }
    })
    unread.value = list.filter((a) => !doneIds.has(a.id))
    toast.success(`已标记 ${doneIds.size}/${list.length} 条已读`)
  } catch (e) {
    toast.error(errText(e))
  } finally {
    markAllBusy.value = false
  }
}

const TONE_BADGE: Record<Tone, string> = {
  ok: 'bg-ok-soft text-ok',
  warn: 'bg-warn-soft text-warn',
  error: 'bg-bad-soft text-bad',
  muted: 'bg-muted text-muted-foreground',
  accent: 'bg-accent text-foreground',
}

// 核心服务列表
const routeEntries = computed(() => Object.entries(health.value?.routes ?? {}))
</script>

<template>
  <div class="space-y-6">
    <PageHeader title="仪表盘" subtitle="服务状态、上游额度与网络流量分析">
      <template #actions>
        <Button variant="outline" size="sm" :disabled="isPolling" @click="refreshNow">
          <RefreshCw :class="{ 'animate-spin': isPolling }" class="size-3.5 mr-1" />
          {{ isPolling ? '更新中…' : '刷新' }}
        </Button>
      </template>
    </PageHeader>

    <!-- 首次加载骨架 -->
    <template v-if="firstLoading">
      <div class="grid gap-4 md:grid-cols-3">
        <SkeletonCard />
        <SkeletonCard />
        <SkeletonCard />
      </div>
      <div class="mt-8">
        <SkeletonTable :rows="3" />
      </div>
    </template>

    <!-- 首拉完全失败错误态 -->
    <div v-else-if="showFatal" class="rounded-xl bg-bad-soft px-4 py-10 text-center">
      <p class="mx-auto max-w-lg break-all text-sm text-bad">{{ error }}</p>
      <Button class="mt-4" variant="outline" size="sm" :disabled="isPolling" @click="refreshNow">
        <RefreshCw :class="{ 'animate-spin': isPolling }" class="size-3.5 mr-1" />
        {{ isPolling ? '正在连接…' : '重试连接' }}
      </Button>
    </div>

    <template v-else>
      <!-- 轮询异常横幅 -->
      <div
        v-if="error"
        class="flex items-center justify-between gap-3 rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad"
      >
        <span class="min-w-0 flex-1 truncate">{{ error }}</span>
        <Button variant="outline" size="xs" :disabled="isPolling" @click="refreshNow">重试</Button>
      </div>

      <!-- 核心三卡 -->
      <div class="grid gap-4 md:grid-cols-3">
        <!-- 卡 1：网关运行状态 -->
        <Card class="flex flex-col justify-between">
          <CardContent class="p-4">
            <div class="text-xs font-medium text-muted-foreground flex items-center justify-between">
              <span>网关服务状态</span>
              <Activity class="size-3.5 text-muted-foreground" />
            </div>
            <div class="mt-3">
              <StatusDot :tone="dbView.tone" :label="dbView.label" size="lg" />
            </div>
            <div class="mt-2 text-xs text-muted-foreground">
              当前活跃设备：<span class="font-medium text-foreground">{{ health?.tokens_active ?? '—' }}</span>
            </div>
          </CardContent>
        </Card>

        <!-- 卡 2：近 24 小时流量 -->
        <Card class="flex flex-col justify-between">
          <CardContent class="p-4 flex flex-1 flex-col">
            <div class="text-xs font-medium text-muted-foreground">近 24 小时请求流量</div>
            <template v-if="usage">
              <div class="mt-2 text-2xl font-semibold tabular-nums text-foreground">
                {{ fmtCount(usage.total.requests) }}
                <span class="text-xs font-normal text-muted-foreground">次请求</span>
              </div>
              <div class="mt-1 text-xs tabular-nums text-muted-foreground">
                ↑{{ fmtBytes(usage.total.bytes_in) }} · ↓{{ fmtBytes(usage.total.bytes_out) }}
              </div>
            </template>
            <p v-else class="mt-3 text-xs text-muted-foreground">暂无用量产生</p>
            <button
              type="button"
              class="mt-auto pt-3 text-left text-xs font-medium text-primary hover:underline cursor-pointer"
              @click="showUsageDrawer = true"
            >
              查看详细用量分析 ›
            </button>
          </CardContent>
        </Card>

        <!-- 卡 3：上游出口与额度监控 (CF / Vercel) -->
        <Card class="flex flex-col justify-between">
          <CardContent class="p-4 flex flex-1 flex-col">
            <div class="flex items-center gap-1 text-xs font-medium text-muted-foreground">
              <span>上游中转出口与连通度</span>
              <InfoTip text="中转线路出口的连通性与免费配额状态，自动双轨轮询探活" />
            </div>
            <div v-if="quotaSources.length > 0" class="mt-3 space-y-2">
              <div v-for="s in quotaSources" :key="s.name" class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                  <span class="block truncate text-xs font-medium flex items-center gap-1">
                    {{ upstreamLabel(s.name).label }}
                    <InfoTip v-if="getUpstreamCardView(s.name, s.state).tip" :text="getUpstreamCardView(s.name, s.state).tip!" />
                  </span>
                  <span v-if="s.last_ok !== null && s.state === 'error'" class="block text-[11px] tabular-nums text-muted-foreground">
                    上次正常: {{ fmtRelative(s.last_ok * 1000) }}
                  </span>
                </div>
                <StatusDot
                  :tone="getUpstreamCardView(s.name, s.state).tone"
                  :label="getUpstreamCardView(s.name, s.state).label"
                  class="shrink-0 text-xs"
                />
              </div>
            </div>
            <p v-else class="mt-3 text-xs text-muted-foreground">暂无可用中转线路</p>
            <button
              type="button"
              class="mt-auto pt-3 text-left text-xs font-medium text-primary hover:underline cursor-pointer"
              @click="showUsageDrawer = true"
            >
              额度明细 ›
            </button>
          </CardContent>
        </Card>
      </div>

      <!-- 告警列表 -->
      <section v-if="sortedAlerts.length > 0" class="mt-6">
        <div class="mb-2 flex items-center justify-between">
          <h2 class="text-xs font-semibold tracking-tight flex items-center gap-1.5 text-warn">
            <Bell class="size-3.5" />
            未读告警（{{ sortedAlerts.length }}）
          </h2>
          <Button variant="ghost" size="xs" :disabled="markAllBusy" @click="markAllRead">
            <Loader2 v-if="markAllBusy" class="size-3 animate-spin mr-1" />
            全部标为已读
          </Button>
        </div>

        <ul class="space-y-1.5">
          <li
            v-for="a in sortedAlerts"
            :key="a.id"
            class="flex items-center justify-between gap-3 rounded-lg px-3 py-2 text-xs"
            :class="a.level === 'critical' ? 'bg-bad-soft' : 'bg-card border border-border/50'"
          >
            <div class="flex min-w-0 items-center gap-2">
              <span class="shrink-0 rounded-full px-2 py-0.5 text-[10px]" :class="TONE_BADGE[alertLevelView(a.level).tone]">
                {{ alertLevelView(a.level).label }}
              </span>
              <span class="min-w-0 break-words">{{ a.message }}</span>
            </div>
            <div class="flex shrink-0 items-center gap-2">
              <span class="text-[11px] tabular-nums text-muted-foreground">{{ fmtRelative(a.ts * 1000) }}</span>
              <Button variant="ghost" size="xs" :disabled="markingId === a.id" @click="markRead(a)">
                {{ markingId === a.id ? '标记中…' : '标为已读' }}
              </Button>
            </div>
          </li>
        </ul>
      </section>

      <!-- 核心服务健康度矩阵（含畅通性与延时指标） -->
      <section class="mt-6">
        <div class="mb-2 flex items-center justify-between">
          <h2 class="text-xs font-semibold tracking-tight text-foreground flex items-center gap-1.5">
            <Zap class="size-3.5 text-primary" />
            已配置服务健康度（{{ routeEntries.length }}）
          </h2>
          <RouterLink to="/core" class="text-xs text-primary hover:underline">去管理服务 ›</RouterLink>
        </div>

        <EmptyState
          v-if="routeEntries.length === 0"
          title="还没有添加服务"
          description="前往「代理与服务」添加常用的大模型或 API 服务。"
        >
          <template #actions>
            <Button as-child size="sm">
              <RouterLink to="/core">去添加服务</RouterLink>
            </Button>
          </template>
        </EmptyState>
        <Card v-else class="py-1">
          <ul class="divide-y divide-border/40">
            <li
              v-for="[name, cfg] in routeEntries"
              :key="name"
              class="mx-4 flex items-center justify-between gap-3 py-2.5 text-xs first:pt-2.5 last:pb-2.5"
              :class="{ 'opacity-60': !cfg.enabled }"
            >
              <!-- 服务名与中转出口 -->
              <div class="flex items-center gap-2 min-w-36">
                <span class="font-medium">{{ name }}</span>
                <span class="font-mono text-[10px] text-muted-foreground rounded bg-muted px-1.5 py-0.5">
                  {{ upstreamLabel(cfg.upstream).label }}
                </span>
              </div>

              <!-- 畅通性与延时指标 -->
              <div class="flex items-center gap-2">
                <template v-if="!cfg.enabled">
                  <StatusDot tone="muted" label="已停用" class="text-xs" />
                </template>
                <template v-else-if="probingRoute === name">
                  <span class="text-[11px] text-muted-foreground animate-pulse">测速中…</span>
                </template>
                <template v-else-if="routeProbes[name]">
                  <StatusDot
                    v-if="routeProbes[name]?.ok"
                    tone="ok"
                    :label="`畅通 · ${routeProbes[name]?.ms ?? '?'}ms`"
                    class="text-xs font-mono"
                  />
                  <StatusDot
                    v-else
                    tone="error"
                    :label="`异常: ${routeProbes[name]?.err || '超时'}`"
                    class="text-xs"
                  />
                </template>
                <template v-else>
                  <StatusDot tone="ok" label="启用中" class="text-xs" />
                </template>

                <!-- 行内单点快速复测 -->
                <button
                  v-if="cfg.enabled"
                  type="button"
                  class="text-muted-foreground hover:text-foreground cursor-pointer p-0.5"
                  :disabled="probingRoute === name"
                  title="重新测速"
                  @click="testSingleRouteInDashboard(name)"
                >
                  <RefreshCw class="size-3" :class="{ 'animate-spin': probingRoute === name }" />
                </button>
              </div>
            </li>
          </ul>
        </Card>
      </section>
    </template>

    <!-- 用量分析侧边抽屉 -->
    <UsageDrawer v-model:open="showUsageDrawer" />
  </div>
</template>
