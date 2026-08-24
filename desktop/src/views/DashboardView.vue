<script setup lang="ts">
// 总览（SPEC §3.4）：三卡 + 我的接入模板 + 告警处理 + 服务健康列表。
// 取数序列等价迁移：onMounted 并行 [health, usage(24h), quota, alerts(true,50)]。
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Loader2, RefreshCw } from '@lucide/vue'

import { api, type AlertDto, type HealthResp, type QuotaResp, type UsageResp } from '@/api/client'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonCard from '@/components/common/SkeletonCard.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { useToast } from '@/composables/useToast'
import { loadBackendUrl, loadDataPlaneUrl } from '@/lib/config'
import { errText } from '@/lib/errors'
import { fmtBytes, fmtCount, fmtRelative } from '@/lib/format'
import { alertLevelView, quotaSourceLabel, upstreamLabel, type Tone } from '@/lib/statusLabels'
import { deriveDataPlane } from '@/lib/urls'

const toast = useToast()

const health = ref<HealthResp | null>(null)
const usage = ref<UsageResp | null>(null)
const quota = ref<QuotaResp | null>(null)
const unread = ref<AlertDto[]>([])
const error = ref('')
const loading = ref(false)
const markingId = ref<number | null>(null)
const markAllBusy = ref(false)

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [h, u, q, a] = await Promise.all([api.health(), api.usage({ hours: 24 }), api.quota(), api.alerts(true, 50)])
    health.value = h
    usage.value = u
    quota.value = q
    unread.value = a.alerts
  } catch (e) {
    error.value = errText(e)
  } finally {
    loading.value = false
  }
}

onMounted(refresh)

// 首拉未落地 → 骨架；首拉失败且无任何数据 → 整页错误态（重试）
const firstLoading = computed(() => health.value === null && error.value === '')
const showFatal = computed(() => health.value === null && error.value !== '' && !loading.value)

// ---- 卡一：服务状态 ----
// db==='ok' 视为运行正常，其余一律异常；health.status 自由字符串不猜语义，灰点小字展示
const dbView = computed(() =>
  health.value?.db === 'ok' ? { label: '运行正常', tone: 'ok' as Tone } : { label: '异常', tone: 'error' as Tone },
)

// ---- 卡三：上游额度 ----
const quotaSources = computed(() => quota.value?.sources ?? [])

// ---- 我的接入 ----
// 接入底座：显式数据面地址优先，否则按管理面地址推导（spec §3.4）；两者皆空提示先设置。
// setup 时读取一次即可（与 useBackendGate 同策略：保存后跨页导航自然刷新）。
const accessBase = loadDataPlaneUrl() || deriveDataPlane(loadBackendUrl()) || ''

// 模板含占位符、非秘密：不走 useSecretCopy（无 60s 自清），复制后明示需替换
async function copyAccessTemplate(): Promise<void> {
  try {
    await navigator.clipboard.writeText(`${accessBase}/<令牌>/<服务>`)
    toast.info('内容含占位符，需替换后使用')
  } catch {
    toast.error('复制失败，请手动选择文本复制')
  }
}

// ---- 告警 ----
// critical 置顶（级别权重优先，再按 id 倒序——新的在前）
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
    unread.value = unread.value.filter((a) => a.id !== alert.id) // 行内移除即反馈，不打扰
  } catch (e) {
    toast.error(errText(e))
  } finally {
    markingId.value = null
  }
}

// 全部已读契约：limit=500 重拉 → for 循环逐条 await markAlertRead → busy 全程 →
// toast 报「已读 ok/total」；部分失败时 error toast 提示可重试
async function markAllRead(): Promise<void> {
  markAllBusy.value = true
  try {
    const list = (await api.alerts(true, 500)).alerts
    const done = new Set<number>()
    for (const a of list) {
      try {
        await api.markAlertRead(a.id)
        done.add(a.id)
      } catch {
        // 单条失败不中断循环，最后统一汇报
      }
    }
    unread.value = list.filter((a) => !done.has(a.id))
    toast.success(`已读 ${done.size}/${list.length}`)
    if (done.size < list.length) toast.error(`有 ${list.length - done.size} 条未能标记，可重试`)
  } catch (e) {
    toast.error(errText(e)) // 重拉即失败：本轮一条都没标
  } finally {
    markAllBusy.value = false
  }
}

// 徽章底色按语义 tone 映射（极简色板：绿/黄/红 + 灰）
const TONE_BADGE: Record<Tone, string> = {
  ok: 'border-emerald-200 bg-emerald-50 text-emerald-700',
  warn: 'border-amber-200 bg-amber-50 text-amber-700',
  error: 'border-red-200 bg-red-50 text-red-700',
  muted: 'border-zinc-200 bg-zinc-100 text-zinc-600',
  accent: 'border-primary/20 bg-primary/5 text-foreground',
}

// ---- 服务健康 ----
const routeEntries = computed(() => Object.entries(health.value?.routes ?? {}))
</script>

<template>
  <div>
    <PageHeader title="总览" subtitle="服务健康与用量一览">
      <template #actions>
        <Button variant="outline" size="sm" :disabled="loading" @click="refresh">
          <RefreshCw :class="{ 'animate-spin': loading }" />
          刷新
        </Button>
      </template>
    </PageHeader>

    <!-- 首次加载骨架：三卡 + 告警区 -->
    <template v-if="firstLoading">
      <div class="grid gap-4 md:grid-cols-3">
        <SkeletonCard />
        <SkeletonCard />
        <SkeletonCard />
      </div>
      <div class="mt-6">
        <SkeletonTable :rows="3" />
      </div>
    </template>

    <!-- 首拉失败且无数据：整页错误态 -->
    <div v-else-if="showFatal" class="rounded-lg border border-red-200 bg-red-50 px-4 py-10 text-center">
      <p class="mx-auto max-w-lg break-all text-sm text-red-800">{{ error }}</p>
      <Button class="mt-4" variant="outline" size="sm" :disabled="loading" @click="refresh">重试</Button>
    </div>

    <template v-else>
      <!-- 刷新/重试失败的横幅（已有旧数据时叠加展示） -->
      <div
        v-if="error"
        class="mb-4 flex items-center gap-3 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800"
      >
        <span class="min-w-0 flex-1 break-all">{{ error }}</span>
        <Button variant="outline" size="sm" :disabled="loading" @click="refresh">重试</Button>
      </div>

      <!-- 三卡 -->
      <div class="grid gap-4 md:grid-cols-3">
        <div class="rounded-lg border p-4">
          <div class="text-xs text-muted-foreground">服务状态</div>
          <div class="mt-3">
            <!-- 大号状态点：放大 StatusDot 的点与文字（透传 class） -->
            <StatusDot
              :tone="dbView.tone"
              :label="dbView.label"
              class="gap-2 text-xl font-semibold [&>span:first-child]:size-3"
            />
          </div>
          <div v-if="health?.status" class="mt-1.5">
            <StatusDot tone="muted" :label="health.status" class="text-xs" />
          </div>
          <div class="mt-2 text-sm text-muted-foreground">活跃设备 {{ health?.tokens_active ?? '—' }}</div>
        </div>

        <div class="rounded-lg border p-4">
          <div class="text-xs text-muted-foreground">近 24 小时流量</div>
          <template v-if="usage">
            <div class="mt-3 text-2xl font-semibold tabular-nums">{{ fmtCount(usage.total.requests) }}</div>
            <div class="mt-1 text-xs tabular-nums text-muted-foreground">
              ↑{{ fmtBytes(usage.total.bytes_in) }} ↓{{ fmtBytes(usage.total.bytes_out) }}
            </div>
          </template>
          <p v-else class="mt-3 text-sm text-muted-foreground">暂无数据</p>
        </div>

        <div class="flex flex-col rounded-lg border p-4">
          <div class="text-xs text-muted-foreground">上游额度</div>
          <div v-if="quotaSources.length > 0" class="mt-3 space-y-1.5">
            <div v-for="s in quotaSources" :key="s.name" class="flex items-center justify-between gap-2">
              <span class="min-w-0 truncate text-sm">{{ s.name }}</span>
              <StatusDot v-bind="quotaSourceLabel(s.state)" class="shrink-0 text-xs" />
            </div>
          </div>
          <p v-else class="mt-3 text-sm text-muted-foreground">暂无额度数据（未启用上游监控或暂不支持）</p>
          <RouterLink to="/usage" class="mt-auto pt-3 text-sm text-primary hover:underline">查看详情 ›</RouterLink>
        </div>
      </div>

      <!-- 我的接入 -->
      <div class="mt-4 rounded-lg border p-4">
        <div class="flex items-start justify-between gap-3">
          <div>
            <div class="text-xs text-muted-foreground">我的接入</div>
            <p class="mt-1 text-sm">把下面的接入地址发给你的设备</p>
          </div>
          <Button v-if="accessBase" variant="outline" size="sm" @click="copyAccessTemplate">复制模板</Button>
        </div>
        <template v-if="accessBase">
          <p class="mt-3 break-all rounded-md bg-muted/60 px-3 py-2 font-mono text-sm">
            {{ accessBase }}/<span class="rounded bg-amber-100 px-1 py-0.5 text-amber-800">&lt;令牌&gt;</span>/<span
              class="rounded border border-dashed px-1 py-0.5"
              >&lt;服务&gt;</span
            >
          </p>
          <p class="mt-2 text-xs text-muted-foreground">
            &lt;令牌&gt; 请替换为设备密钥页创建的明文（仅创建时可见）；&lt;服务&gt; 填要访问的服务名。
          </p>
        </template>
        <p v-else class="mt-3 text-sm text-muted-foreground">
          先在<RouterLink to="/settings" class="text-primary hover:underline">设置</RouterLink>完成连接
        </p>
      </div>

      <!-- 告警 -->
      <div class="mt-6">
        <div class="mb-2 flex items-center justify-between gap-3">
          <h2 class="text-sm font-semibold">
            告警<span class="ml-1.5 text-xs font-normal text-muted-foreground">未读 {{ sortedAlerts.length }}</span>
          </h2>
          <Button v-if="sortedAlerts.length > 0" variant="outline" size="xs" :disabled="markAllBusy" @click="markAllRead">
            <Loader2 v-if="markAllBusy" class="animate-spin" />
            全部标为已读
          </Button>
        </div>

        <p v-if="sortedAlerts.length === 0" class="text-sm text-muted-foreground">暂无未读告警</p>
        <ul v-else class="space-y-2">
          <li
            v-for="a in sortedAlerts"
            :key="a.id"
            class="flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-sm"
            :class="a.level === 'critical' ? 'border-red-200 bg-red-50/60' : ''"
          >
            <div class="flex min-w-0 items-center gap-2">
              <span
                class="shrink-0 rounded-full border px-2 py-0.5 text-xs"
                :class="TONE_BADGE[alertLevelView(a.level).tone]"
              >
                {{ alertLevelView(a.level).label }}
              </span>
              <span class="min-w-0 break-words">{{ a.message }}</span>
            </div>
            <div class="flex shrink-0 items-center gap-2">
              <span class="text-xs tabular-nums text-muted-foreground">{{ fmtRelative(a.ts * 1000) }}</span>
              <Button variant="ghost" size="xs" :disabled="markingId === a.id || markAllBusy" @click="markRead(a)">
                <Loader2 v-if="markingId === a.id" class="animate-spin" />
                {{ markingId === a.id ? '标记中…' : '标为已读' }}
              </Button>
            </div>
          </li>
        </ul>
      </div>

      <!-- 服务健康 -->
      <div class="mt-6">
        <h2 class="mb-2 text-sm font-semibold">服务健康（{{ routeEntries.length }}）</h2>
        <EmptyState
          v-if="routeEntries.length === 0"
          title="还没有服务"
          description="添加服务后，这里会显示每个服务的启用状态与出口线路。"
        >
          <template #actions>
            <Button as-child>
              <RouterLink to="/routes">去添加服务</RouterLink>
            </Button>
          </template>
        </EmptyState>
        <ul v-else class="divide-y overflow-hidden rounded-lg border">
          <li
            v-for="[name, cfg] in routeEntries"
            :key="name"
            class="flex items-center gap-3 px-3 py-2.5 text-sm"
            :class="{ 'opacity-60': !cfg.enabled }"
          >
            <span class="font-medium">{{ name }}</span>
            <StatusDot :tone="cfg.enabled ? 'ok' : 'muted'" :label="cfg.enabled ? '启用中' : '已停用'" class="text-xs" />
            <span class="ml-auto shrink-0 text-xs text-muted-foreground">{{ upstreamLabel(cfg.upstream).label }}</span>
          </li>
        </ul>
      </div>
    </template>
  </div>
</template>
