<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  Check,
  ExternalLink,
  Power,
  RefreshCw,
  Server,
  ShieldCheck,
  Sparkles,
  Zap,
} from '@lucide/vue'
import LatencyBars from '@/components/common/LatencyBars.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useToast } from '@/composables/useToast'
import { isTauri } from '@/lib/config'
import {
  appendLatencyPoint,
  loadLatencySeries,
  saveLatencySeries,
  type LatencyPoint,
} from '@/lib/latencyHistory'
import { openExternalUrl } from '@/lib/urls'
import { byteUnit, formatCount, localDateKey, niceScale } from '@/lib/usageChart'

const toast = useToast()

// 运行状态
const isRunning = ref(false)
const proxyMode = ref<'whitelist' | 'global'>('whitelist')
const isConfigured = ref(false)
const configInfo = ref<{
  mode_type: string
  configured: boolean
  worker_url?: string
  remote_host?: string
  username?: string
  has_secret?: boolean
}>({
  mode_type: 'direct',
  configured: false,
})

// 新手向导状态
const setupTab = ref<'direct' | 'chained'>('direct')
const cfToken = ref('')
const syncUriInput = ref('')
const remoteHost = ref('')
const remoteUser = ref('')
const remotePass = ref('')
const isSubmitting = ref(false)

// ---- 用量统计（引擎本地计数：按 gate 端点拆分 CF / Vercel 双出口，含近 7 日历史）----
interface TrafficBucket {
  requests: number
  bytes_up: number
  bytes_down: number
}
interface DayUsage {
  date: string
  cf: TrafficBucket
  vercel: TrafficBucket
}
interface TrafficStats {
  today: { cf: TrafficBucket; vercel: TrafficBucket }
  total: { cf: TrafficBucket; vercel: TrafficBucket }
  history: DayUsage[]
}
const traffic = ref<TrafficStats | null>(null)

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`
  return `${(n / 1024 ** 3).toFixed(2)} GB`
}

function bucketBytes(b: TrafficBucket): number {
  return b.bytes_up + b.bytes_down
}

interface UsageDay {
  date: string
  label: string
  cfReq: number
  vReq: number
  cfBytes: number
  vBytes: number
}

/** 近 7 日双出口用量：缺勤日期补零保持 7 天 */
const weekDays = computed<UsageDay[]>(() => {
  const days = traffic.value?.history ?? []
  const byDate = new Map(days.map((d) => [d.date, d]))
  const list: UsageDay[] = []
  for (let i = 6; i >= 0; i--) {
    const key = localDateKey(new Date(Date.now() - i * 24 * 3600 * 1000))
    const d = byDate.get(key)
    list.push({
      date: key,
      label: key.slice(8),
      cfReq: d?.cf.requests ?? 0,
      vReq: d?.vercel.requests ?? 0,
      cfBytes: d ? bucketBytes(d.cf) : 0,
      vBytes: d ? bucketBytes(d.vercel) : 0,
    })
  }
  return list
})

/**
 * 双轴用量图模型：左轴调用次数（CF/Vercel 双柱），右轴调用量（CF/Vercel 双曲线）。
 * 两轴独立 nice-scale，右轴按最大值动态选 B/KB/MB/GB 单位。
 */
const usageChart = computed(() => {
  const W = 332
  const H = 128
  const L = 34
  const R = 42
  const T = 10
  const B = 22
  const pw = W - L - R
  const ph = H - T - B

  const days = weekDays.value
  // 左轴调用次数：整数步长；右轴调用量：先按最大值选单位，再在单位空间内 nice-scale，刻度保持整数
  const req = niceScale(Math.max(0, ...days.map((d) => Math.max(d.cfReq, d.vReq))), 4, true)
  const maxBytes = Math.max(0, ...days.map((d) => Math.max(d.cfBytes, d.vBytes)))
  const unit = byteUnit(maxBytes)
  const bytInUnit = niceScale(maxBytes / unit.div)
  const bytRawMax = bytInUnit.max * unit.div

  const reqY = (v: number) => T + ph * (1 - v / req.max)
  const bytY = (v: number) => T + ph * (1 - v / bytRawMax)

  const n = days.length || 1
  const groupW = pw / n
  const barW = Math.min(8, groupW * 0.18)

  // 曲线与柱同色（出口品牌色：CF 橙 / Vercel 墨），以更细线宽与数据点区分
  const CF_LINE = '#f6821f'

  const bars: { x: number; y: number; w: number; h: number; cf: boolean; title: string }[] = []
  const cfPts: string[] = []
  const vPts: string[] = []
  const dots: { x: number; y: number; cf: boolean; title: string }[] = []

  days.forEach((d, i) => {
    const cx = L + groupW * (i + 0.5)
    const cfH = (d.cfReq / req.max) * ph
    const vH = (d.vReq / req.max) * ph
    bars.push({
      x: cx - barW - 1.5,
      y: T + ph - cfH,
      w: barW,
      h: cfH,
      cf: true,
      title: `${d.date}\nCloudflare 调用 ${d.cfReq} 次`,
    })
    bars.push({
      x: cx + 1.5,
      y: T + ph - vH,
      w: barW,
      h: vH,
      cf: false,
      title: `${d.date}\nVercel 调用 ${d.vReq} 次`,
    })
    const yc = bytY(d.cfBytes)
    const yv = bytY(d.vBytes)
    cfPts.push(`${cx.toFixed(1)},${yc.toFixed(1)}`)
    vPts.push(`${cx.toFixed(1)},${yv.toFixed(1)}`)
    dots.push({ x: cx, y: yc, cf: true, title: `${d.date}\nCloudflare ${formatBytes(d.cfBytes)}` })
    dots.push({ x: cx, y: yv, cf: false, title: `${d.date}\nVercel ${formatBytes(d.vBytes)}` })
  })

  const leftTicks = req.ticks.map((v) => ({ y: reqY(v), text: formatCount(v) }))
  const rightTicks = bytInUnit.ticks.map((v) => ({
    y: bytY(v * unit.div),
    text: `${parseFloat(v.toFixed(1))}`,
  }))

  return {
    W,
    H,
    L,
    R,
    T,
    B,
    pw,
    ph,
    bars,
    cfLine: cfPts.join(' '),
    vLine: vPts.join(' '),
    dots,
    leftTicks,
    rightTicks,
    unitSuffix: unit.suffix,
    cfLineColor: CF_LINE,
    days,
  }
})

const todayTotals = computed(() => {
  if (!traffic.value) return { bytes: 0, requests: 0 }
  const t = traffic.value.today
  return {
    bytes: bucketBytes(t.cf) + bucketBytes(t.vercel),
    requests: t.cf.requests + t.vercel.requests,
  }
})

const totalBytes = computed(() => {
  if (!traffic.value) return 0
  return bucketBytes(traffic.value.total.cf) + bucketBytes(traffic.value.total.vercel)
})

async function refreshTraffic(): Promise<void> {
  if (!isTauri()) {
    const day = 24 * 3600 * 1000
    const mk = (ago: number, cfB: number, vB: number): DayUsage => ({
      date: localDateKey(new Date(Date.now() - ago * day)),
      cf: { requests: Math.round(cfB / 400_000), bytes_up: Math.round(cfB * 0.08), bytes_down: cfB },
      vercel: { requests: Math.round(vB / 500_000), bytes_up: Math.round(vB * 0.06), bytes_down: vB },
    })
    traffic.value = {
      today: {
        cf: { requests: 96, bytes_up: 2_400_000, bytes_down: 38_000_000 },
        vercel: { requests: 32, bytes_up: 800_000, bytes_down: 12_000_000 },
      },
      total: {
        cf: { requests: 3100, bytes_up: 72_000_000, bytes_down: 960_000_000 },
        vercel: { requests: 1110, bytes_up: 24_000_000, bytes_down: 280_000_000 },
      },
      history: [
        mk(6, 60_000_000, 8_000_000),
        mk(5, 96_000_000, 20_000_000),
        mk(4, 40_000_000, 30_000_000),
        mk(3, 120_000_000, 12_000_000),
        mk(2, 88_000_000, 44_000_000),
        mk(1, 140_000_000, 26_000_000),
        mk(0, 38_000_000, 12_000_000),
      ],
    }
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    traffic.value = await invoke<TrafficStats>('proxy_traffic_stats')
  } catch (e) {
    console.error('Failed to load traffic stats:', e)
  }
}

// ---- 连接状态（接口 + 常用站点；10 分钟轮询，仅显示近 2 小时时序）----
type Iface = 'cf' | 'vercel'

interface IfaceRow {
  id: Iface
  name: string
  endpoint: string
  history: LatencyPoint[]
  testing: boolean
}
interface SiteRow {
  name: string
  host: string
  iface: Iface
  history: LatencyPoint[]
  testing: boolean
}

const ifaceRows = ref<IfaceRow[]>([
  { id: 'cf', name: 'Cloudflare 接口', endpoint: 'edge.ponyjob.top', history: [], testing: false },
  { id: 'vercel', name: 'Vercel 接口', endpoint: 'vedge.ponyjob.top', history: [], testing: false },
])

const siteRows = ref<SiteRow[]>([
  { name: 'Google', host: 'google.com', iface: 'cf', history: [], testing: false },
  { name: 'GitHub', host: 'github.com', iface: 'cf', history: [], testing: false },
  { name: 'X', host: 'x.com', iface: 'cf', history: [], testing: false },
  { name: 'OpenAI', host: 'openai.com', iface: 'vercel', history: [], testing: false },
])

const IFACE_CHOICE_PREFIX = 'pony-site-iface:'

function siteSeriesKey(host: string): string {
  return `site:${host}`
}

function loadAllHistories(): void {
  for (const row of ifaceRows.value) {
    row.history = loadLatencySeries(`iface:${row.id}`)
  }
  for (const row of siteRows.value) {
    try {
      const saved = localStorage.getItem(IFACE_CHOICE_PREFIX + row.host)
      if (saved === 'cf' || saved === 'vercel') row.iface = saved
    } catch { /* 忽略 */ }
    let hist = loadLatencySeries(siteSeriesKey(row.host))
    if (!hist.length) {
      // 兼容迁移按出口存储的旧版时序数据
      const oldHist = loadLatencySeries(`site:${row.host}:${row.iface}`)
      if (oldHist.length) {
        hist = oldHist
        saveLatencySeries(siteSeriesKey(row.host), hist)
      }
    }
    row.history = hist
  }
}

function switchSiteIface(row: SiteRow, iface: Iface): void {
  if (row.iface === iface) return
  row.iface = iface
  try {
    localStorage.setItem(IFACE_CHOICE_PREFIX + row.host, iface)
  } catch { /* 忽略 */ }
  void testSiteRow(row)
}

async function probeEgress(iface: Iface): Promise<LatencyPoint> {
  if (!isTauri()) {
    await new Promise((r) => setTimeout(r, 300))
    const ms = Math.floor(Math.random() * 600) + 120
    return { ts: Date.now(), ok: true, ms }
  }
  const { invoke } = await import('@tauri-apps/api/core')
  const r = await invoke<{ ok: boolean; ms: number; error?: string }>('proxy_test_egress', { iface })
  return { ts: Date.now(), ok: r.ok, ms: r.ms, err: r.error }
}

async function probeSite(host: string, iface: Iface): Promise<LatencyPoint> {
  if (!isTauri()) {
    await new Promise((r) => setTimeout(r, 400))
    const ms = Math.floor(Math.random() * 900) + 150
    return { ts: Date.now(), ok: true, ms }
  }
  const { invoke } = await import('@tauri-apps/api/core')
  const r = await invoke<{ ok: boolean; ms: number; error?: string }>('proxy_test_site_via', {
    iface,
    host,
  })
  return { ts: Date.now(), ok: r.ok, ms: r.ms, err: r.error }
}

async function testIfaceRow(row: IfaceRow): Promise<void> {
  if (row.testing) return
  row.testing = true
  try {
    const point = await probeEgress(row.id)
    row.history = appendLatencyPoint(row.history, point)
    saveLatencySeries(`iface:${row.id}`, row.history)
  } catch (e) {
    row.history = appendLatencyPoint(row.history, { ts: Date.now(), ok: false, err: String(e) })
    saveLatencySeries(`iface:${row.id}`, row.history)
  } finally {
    row.testing = false
  }
}

async function testSiteRow(row: SiteRow): Promise<void> {
  if (row.testing) return
  row.testing = true
  try {
    const point = await probeSite(row.host, row.iface)
    row.history = appendLatencyPoint(row.history, point)
    saveLatencySeries(siteSeriesKey(row.host), row.history)
  } catch (e) {
    row.history = appendLatencyPoint(row.history, { ts: Date.now(), ok: false, err: String(e) })
    saveLatencySeries(siteSeriesKey(row.host), row.history)
  } finally {
    row.testing = false
  }
}

const isTestingAll = ref(false)

async function runAllTests(): Promise<void> {
  if (isTestingAll.value || !isConfigured.value) return
  isTestingAll.value = true
  try {
    await Promise.all([
      ...ifaceRows.value.map((r) => testIfaceRow(r)),
      ...siteRows.value.map((r) => testSiteRow(r)),
    ])
  } finally {
    isTestingAll.value = false
  }
}

function latestPoint(history: LatencyPoint[]): LatencyPoint | undefined {
  return history[history.length - 1]
}

function latestText(history: LatencyPoint[]): string {
  const p = latestPoint(history)
  if (!p) return '—'
  if (!p.ok) return '失败'
  return `${p.ms ?? 0}ms`
}

function latestClass(history: LatencyPoint[]): string {
  const p = latestPoint(history)
  if (!p) return 'text-muted-foreground'
  if (!p.ok) return 'text-rose-600'
  const ms = p.ms ?? 0
  if (ms <= 800) return 'text-emerald-600'
  if (ms <= 2000) return 'text-amber-600'
  return 'text-rose-600'
}

// 状态弱化文字（按钮之下）
const statusText = computed(() => {
  if (!isRunning.value) return '加速已停止，点击上方开启'
  return proxyMode.value === 'whitelist'
    ? '海外加速，国内直连'
    : '全部流量走加速'
})

// 轮询：测速 10 分钟一轮（对齐 2 小时 12 根柱条）；用量 60 秒一刷
const POLL_TEST_MS = 10 * 60 * 1000
const POLL_TRAFFIC_MS = 60 * 1000
let testTimer: ReturnType<typeof setInterval> | undefined
let trafficTimer: ReturnType<typeof setInterval> | undefined

onMounted(async () => {
  loadAllHistories()
  await refreshStatus()
  void refreshTraffic()
  void runAllTests()
  testTimer = setInterval(() => void runAllTests(), POLL_TEST_MS)
  trafficTimer = setInterval(() => void refreshTraffic(), POLL_TRAFFIC_MS)
})

onUnmounted(() => {
  if (testTimer) clearInterval(testTimer)
  if (trafficTimer) clearInterval(trafficTimer)
})

async function refreshStatus() {
  if (!isTauri()) {
    isConfigured.value = true
    isRunning.value = true
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const status = (await invoke('proxy_status')) as { engine_running: boolean; mode: 'whitelist' | 'global' }
    isRunning.value = status.engine_running
    proxyMode.value = status.mode || 'whitelist'

    const cfg = (await invoke('proxy_get_current_config')) as typeof configInfo.value
    configInfo.value = cfg
    isConfigured.value = cfg.configured
  } catch (e) {
    console.error('Failed to get status:', e)
  }
}

async function toggleProxy() {
  if (!isTauri()) {
    isRunning.value = !isRunning.value
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    if (isRunning.value) {
      await invoke('proxy_disable')
      isRunning.value = false
      toast.success('已关闭加速')
    } else {
      await invoke('proxy_enable')
      // 后端启用成功只是「本地监听 + 系统代理接管」；真实出网由后端拨测保证。
      // 这里复核 status 再置位，杜绝「显示已开启但连不上」的假状态。
      const st = (await invoke('proxy_status')) as { engine_running: boolean; mode?: 'whitelist' | 'global' }
      isRunning.value = st.engine_running
      if (st.mode) proxyMode.value = st.mode
      if (!st.engine_running) {
        toast.error('开启失败', '出网拨测未通过，系统代理未启用。请检查方案 A 授权码 / 方案 B 远端地址后重试')
        return
      }
      toast.success('智能加速已开启！')
    }
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '操作失败')
  }
}

async function setProxyMode(mode: 'whitelist' | 'global') {
  if (!isTauri()) {
    proxyMode.value = mode
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('proxy_mode_set', { mode })
    proxyMode.value = mode
    toast.success(mode === 'whitelist' ? '已切换至智能分流模式' : '已切换至全局加速模式')
  } catch (e: any) {
    toast.error('切换模式失败')
  }
}

// ---- 向导提交 ----
async function submitDirectSetup() {
  if (!cfToken.value.trim()) {
    toast.error('请输入加速授权码')
    return
  }
  isSubmitting.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_mode_switch', {
        modeType: 'direct',
        config: {
          worker_url: 'https://edge.ponyjob.top',
          proxy_secret: cfToken.value.trim(),
        },
      })
    }
    toast.success('配置成功！已准备就绪。')
    isConfigured.value = true
    await refreshStatus()
    await toggleProxy()
  } catch (e: any) {
    toast.error('配置失败：' + (typeof e === 'string' ? e : e?.message))
  } finally {
    isSubmitting.value = false
  }
}

async function submitImportOrChained() {
  if (syncUriInput.value.trim()) {
    // 口令一键导入
    isSubmitting.value = true
    try {
      if (isTauri()) {
        const { invoke } = await import('@tauri-apps/api/core')
        const res = (await invoke('proxy_import_sync', {
          syncUri: syncUriInput.value.trim(),
        })) as any
        toast.success(res.message || '导入成功！')
      } else {
        toast.success('口令导入成功！')
      }
      isConfigured.value = true
      await refreshStatus()
      await toggleProxy()
    } catch (e: any) {
      toast.error('导入失败：' + (typeof e === 'string' ? e : e?.message))
    } finally {
      isSubmitting.value = false
    }
  } else if (remoteHost.value.trim()) {
    // 手动远端输入
    isSubmitting.value = true
    try {
      if (isTauri()) {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('proxy_mode_switch', {
          modeType: 'chained',
          config: {
            remote_host: remoteHost.value.trim(),
            username: remoteUser.value.trim(),
            password: remotePass.value.trim(),
          },
        })
      }
      toast.success('远端代理已连接！')
      isConfigured.value = true
      await refreshStatus()
      await toggleProxy()
    } catch (e: any) {
      toast.error('连接失败：' + (typeof e === 'string' ? e : e?.message))
    } finally {
      isSubmitting.value = false
    }
  } else {
    toast.error('请粘贴一键口令，或填写服务器地址')
  }
}
</script>

<template>
  <div class="h-full overflow-y-auto p-6 max-w-4xl mx-auto">
    <!-- 初始设置向导：选方案 → 填凭据 → 开启 -->
    <div v-if="!isConfigured" class="space-y-6 py-2">
      <div class="text-center space-y-2">
        <span class="inline-flex items-center gap-1.5 rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-medium text-primary">
          <Sparkles class="h-3 w-3" />
          初始设置
        </span>
        <h1 class="text-2xl font-bold tracking-tight text-foreground">欢迎使用 Pony Proxy</h1>
        <p class="text-sm text-muted-foreground">两步完成配置，立即开启加速</p>
      </div>

      <!-- 第 1 步：选择连接方式 -->
      <div class="space-y-2">
        <div class="text-xs font-medium text-muted-foreground">第 1 步 · 选择连接方式</div>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <button
            @click="setupTab = 'direct'"
            :class="[
              'relative rounded-xl border p-4 text-left transition-all',
              setupTab === 'direct'
                ? 'border-primary bg-primary/5 ring-1 ring-primary'
                : 'border-border bg-card hover:border-muted-foreground/40',
            ]"
          >
            <Check
              v-if="setupTab === 'direct'"
              class="absolute right-3 top-3 h-4 w-4 text-primary"
            />
            <div class="flex items-center gap-3">
              <div
                :class="[
                  'rounded-lg p-2',
                  setupTab === 'direct' ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground',
                ]"
              >
                <Sparkles class="h-4 w-4" />
              </div>
              <div>
                <div class="text-sm font-semibold flex items-center gap-1.5">
                  个人独立加速
                  <span class="rounded bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">推荐</span>
                </div>
                <div class="text-xs text-muted-foreground mt-0.5">粘贴授权码，开通 Cloudflare / Vercel 双出口</div>
              </div>
            </div>
          </button>

          <button
            @click="setupTab = 'chained'"
            :class="[
              'relative rounded-xl border p-4 text-left transition-all',
              setupTab === 'chained'
                ? 'border-primary bg-primary/5 ring-1 ring-primary'
                : 'border-border bg-card hover:border-muted-foreground/40',
            ]"
          >
            <Check
              v-if="setupTab === 'chained'"
              class="absolute right-3 top-3 h-4 w-4 text-primary"
            />
            <div class="flex items-center gap-3">
              <div
                :class="[
                  'rounded-lg p-2',
                  setupTab === 'chained' ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground',
                ]"
              >
                <Server class="h-4 w-4" />
              </div>
              <div>
                <div class="text-sm font-semibold">连接远端代理</div>
                <div class="text-xs text-muted-foreground mt-0.5">粘贴同步口令，或连接自己的服务器</div>
              </div>
            </div>
          </button>
        </div>
      </div>

      <!-- 第 2 步：完成授权 -->
      <div class="space-y-2">
        <div class="text-xs font-medium text-muted-foreground">第 2 步 · 完成授权</div>

        <!-- 方案 A：授权码（一枚令牌同时开通 CF / Vercel 双出口） -->
        <Card v-if="setupTab === 'direct'" class="border-border shadow-sm">
          <CardHeader class="pb-3">
            <CardTitle class="text-base">个人独立加速</CardTitle>
            <CardDescription>一枚授权码同时开通 Cloudflare 与 Vercel 双出口，自动故障切换</CardDescription>
          </CardHeader>
          <CardContent class="space-y-3">
            <div class="space-y-1.5">
              <div class="flex items-center justify-between">
                <Label class="text-xs font-medium">加速授权码</Label>
                <button
                  type="button"
                  @click="openExternalUrl('https://dash.cloudflare.com/profile/api-tokens')"
                  class="text-xs text-primary hover:underline flex items-center gap-1 cursor-pointer bg-transparent border-0 p-0"
                >
                  获取授权码 <ExternalLink class="h-3 w-3" />
                </button>
              </div>
              <Input
                v-model="cfToken"
                type="password"
                placeholder="粘贴授权码"
                class="font-mono text-sm"
              />
            </div>
            <p class="text-xs text-muted-foreground flex items-center gap-1.5">
              <ShieldCheck class="h-3.5 w-3.5 text-emerald-600 shrink-0" />
              仅保存在本机系统凭据管理器，绝不上传
            </p>
            <Button
              @click="submitDirectSetup"
              :disabled="isSubmitting || !cfToken.trim()"
              class="w-full h-11 text-sm font-semibold"
            >
              <Zap v-if="!isSubmitting" class="h-4 w-4 mr-2" />
              <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
              {{ isSubmitting ? '正在初始化…' : '开启加速' }}
            </Button>
          </CardContent>
        </Card>

        <!-- 方案 B：口令导入或手动连接 -->
        <Card v-if="setupTab === 'chained'" class="border-border shadow-sm">
          <CardHeader class="pb-3">
            <CardTitle class="text-base">连接远端代理</CardTitle>
            <CardDescription>粘贴同步口令，或手动填写服务器参数</CardDescription>
          </CardHeader>
          <CardContent class="space-y-4">
            <div class="space-y-1.5">
              <Label class="text-xs font-medium">一键连接口令</Label>
              <Input
                v-model="syncUriInput"
                placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
                class="font-mono text-xs"
              />
              <p class="text-xs text-muted-foreground">
                由 Linux Server 的 <code>pproxy user add</code> 或 <code>pproxy sync export</code> 导出
              </p>
            </div>

            <div class="relative flex items-center">
              <div class="flex-grow border-t border-border"></div>
              <span class="flex-shrink mx-4 text-xs text-muted-foreground">或手动填写</span>
              <div class="flex-grow border-t border-border"></div>
            </div>

            <div class="grid grid-cols-2 gap-3">
              <div class="col-span-2 space-y-1.5">
                <Label class="text-xs">服务器地址</Label>
                <Input v-model="remoteHost" placeholder="IP 或域名 : 端口，如 192.168.1.100:8899" class="text-sm" />
              </div>
              <div class="space-y-1.5">
                <Label class="text-xs">用户名</Label>
                <Input v-model="remoteUser" placeholder="用户名" class="text-sm" />
              </div>
              <div class="space-y-1.5">
                <Label class="text-xs">密码</Label>
                <Input v-model="remotePass" type="password" placeholder="密码" class="text-sm" />
              </div>
            </div>

            <Button
              @click="submitImportOrChained"
              :disabled="isSubmitting || (!syncUriInput.trim() && !remoteHost.trim())"
              class="w-full h-11 text-sm font-semibold"
            >
              <Server v-if="!isSubmitting" class="h-4 w-4 mr-2" />
              <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
              {{ isSubmitting ? '正在验证连接…' : '连接并开启加速' }}
            </Button>
          </CardContent>
        </Card>
      </div>
    </div>

    <!-- 已配置：极简主界面 -->
    <div v-else class="space-y-10">
      <!-- 头部：主控（电源按钮 + 模式）居左，用量统计横向并排 -->
      <div class="flex items-center gap-24 pt-6">
        <!-- 主控：圆形电源按钮 + 模式 switch + 弱化状态文字（整体居左） -->
        <section class="flex flex-col items-center shrink-0 w-48">
          <button
            @click="toggleProxy"
            :title="isRunning ? '点击关闭加速' : '点击开启加速'"
            :aria-label="isRunning ? '加速运行中，点击关闭' : '加速已停止，点击开启'"
            :class="[
              'h-20 w-20 rounded-full flex items-center justify-center transition-all duration-200 cursor-pointer',
              isRunning
                ? 'bg-emerald-500 text-white shadow-lg shadow-emerald-500/25 hover:bg-emerald-600'
                : 'bg-muted text-muted-foreground hover:bg-accent hover:text-foreground',
            ]"
          >
            <Power class="h-8 w-8" />
          </button>

          <!-- 加速模式 switch：智能 / 全局 -->
          <div class="mt-5 inline-flex items-center rounded-full bg-muted p-1" role="group" aria-label="加速模式">
            <button
              @click="setProxyMode('whitelist')"
              :aria-pressed="proxyMode === 'whitelist'"
              :class="[
                'px-6 py-1.5 rounded-full text-sm transition-all duration-150 cursor-pointer',
                proxyMode === 'whitelist'
                  ? 'bg-card text-foreground font-medium shadow-sm'
                  : 'text-muted-foreground hover:text-foreground',
              ]"
            >
              智能
            </button>
            <button
              @click="setProxyMode('global')"
              :aria-pressed="proxyMode === 'global'"
              :class="[
                'px-6 py-1.5 rounded-full text-sm transition-all duration-150 cursor-pointer',
                proxyMode === 'global'
                  ? 'bg-card text-foreground font-medium shadow-sm'
                  : 'text-muted-foreground hover:text-foreground',
              ]"
            >
              全局
            </button>
          </div>

          <p class="mt-3 text-xs text-muted-foreground text-center">{{ statusText }}</p>
        </section>

        <!-- 用量统计：近 7 日双轴图（左轴调用次数·双柱，右轴调用量·双曲线） -->
        <section class="flex-1 min-w-0 space-y-2">
          <h3 class="text-sm font-semibold">
            用量统计
            <span class="ml-2 text-xs font-normal text-muted-foreground">
              今日请求
              <span class="text-foreground font-medium tabular-nums">{{ todayTotals.requests }}</span>
              · 累计
              <span class="text-foreground font-medium tabular-nums">{{ formatBytes(totalBytes) }}</span>
            </span>
          </h3>

          <svg
            :viewBox="`0 0 ${usageChart.W} ${usageChart.H}`"
            class="w-full h-auto select-none"
            role="img"
            aria-label="近 7 日双出口用量统计：柱状为调用次数，曲线为调用量"
          >
            <!-- 横向网格线（对齐左轴刻度） -->
            <line
              v-for="(t, i) in usageChart.leftTicks"
              :key="'g' + i"
              :x1="usageChart.L"
              :x2="usageChart.L + usageChart.pw"
              :y1="t.y"
              :y2="t.y"
              class="stroke-muted"
              stroke-width="1"
              :stroke-dasharray="i === 0 ? undefined : '3 4'"
            />

            <!-- 双柱：左轴调用次数 -->
            <rect
              v-for="(b, i) in usageChart.bars"
              :key="'b' + i"
              :x="b.x"
              :y="b.y"
              :width="b.w"
              :height="b.h"
              rx="2"
              :fill="b.cf ? '#f6821f' : undefined"
              :class="b.cf ? 'opacity-90' : 'fill-foreground/80'"
            >
              <title>{{ b.title }}</title>
            </rect>

            <!-- 双曲线：右轴调用量 -->
            <polyline
              :points="usageChart.cfLine"
              fill="none"
              :stroke="usageChart.cfLineColor"
              stroke-width="0.75"
              stroke-linejoin="round"
              stroke-linecap="round"
            />
            <polyline
              :points="usageChart.vLine"
              fill="none"
              class="stroke-foreground/80"
              stroke-width="0.75"
              stroke-linejoin="round"
              stroke-linecap="round"
            />
            <circle
              v-for="(d, i) in usageChart.dots"
              :key="'d' + i"
              :cx="d.x"
              :cy="d.y"
              r="1.25"
              :fill="d.cf ? usageChart.cfLineColor : undefined"
              :class="d.cf ? '' : 'fill-foreground/80'"
            >
              <title>{{ d.title }}</title>
            </circle>

            <!-- 左轴：调用次数 -->
            <text
              v-for="(t, i) in usageChart.leftTicks"
              :key="'l' + i"
              :x="usageChart.L - 6"
              :y="t.y + 3"
              text-anchor="end"
              class="fill-muted-foreground/60 text-[8px] tabular-nums"
            >
              {{ t.text }}
            </text>
            <text
              :x="usageChart.L - 6"
              :y="usageChart.H - 6"
              text-anchor="end"
              class="fill-muted-foreground text-[8px]"
            >
              次数
            </text>

            <!-- 右轴：调用量（单位随最大值动态切换） -->
            <text
              v-for="(t, i) in usageChart.rightTicks"
              :key="'r' + i"
              :x="usageChart.L + usageChart.pw + 6"
              :y="t.y + 3"
              text-anchor="start"
              class="fill-muted-foreground/60 text-[8px] tabular-nums"
            >
              {{ t.text }}
            </text>
            <text
              :x="usageChart.L + usageChart.pw + 6"
              :y="usageChart.H - 6"
              text-anchor="start"
              class="fill-muted-foreground text-[8px]"
            >
              {{ usageChart.unitSuffix }}
            </text>

            <!-- X 轴日期标签 -->
            <text
              v-for="(d, i) in usageChart.days"
              :key="'x' + i"
              :x="usageChart.L + usageChart.pw * ((i + 0.5) / (usageChart.days.length || 1))"
              :y="usageChart.H - 6"
              text-anchor="middle"
              class="fill-muted-foreground/60 text-[8px] tabular-nums"
            >
              {{ d.label }}
            </text>
          </svg>

          <div class="flex items-center justify-end gap-3 text-[11px] text-muted-foreground">
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2.5 w-2.5 rounded-sm bg-[#f6821f]"></span>CF 次数
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2.5 w-2.5 rounded-sm bg-foreground/80"></span>Vercel 次数
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="inline-block h-0.5 w-3.5 rounded-full bg-[#f6821f]"></span>CF 流量
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="inline-block h-0.5 w-3.5 rounded-full bg-foreground/80"></span>Vercel 流量
            </span>
          </div>
        </section>
      </div>

      <!-- 连接状态：接口 + 常用站点连通性 -->
      <section class="space-y-1">
        <div class="flex items-center justify-between mb-2">
          <h3 class="text-sm font-semibold">连接状态</h3>
          <button
            @click="runAllTests"
            :disabled="isTestingAll"
            title="全部重新测速"
            aria-label="全部重新测速"
            class="h-8 w-8 inline-flex items-center justify-center rounded-full text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-50"
          >
            <RefreshCw class="h-4 w-4" :class="{ 'animate-spin': isTestingAll }" />
          </button>
        </div>

        <div class="grid grid-cols-1 lg:grid-cols-2 gap-x-12 lg:gap-x-16">
        <!-- 出网接口行 -->
        <div
          v-for="row in ifaceRows"
          :key="row.id"
          class="flex items-center justify-between gap-3 py-2.5"
        >
          <div class="flex flex-col min-w-0">
            <div class="text-sm font-medium leading-none truncate">{{ row.name }}</div>
            <div class="text-xs text-muted-foreground font-mono mt-1 truncate">{{ row.endpoint }}</div>
          </div>
          <div class="flex items-center gap-3 shrink-0">
            <LatencyBars :history="row.history" />
            <span class="w-14 shrink-0 text-right text-xs font-mono tabular-nums" :class="latestClass(row.history)">
              {{ latestText(row.history) }}
            </span>
            <button
              @click="testIfaceRow(row)"
              :disabled="row.testing"
              title="立即测速"
              :aria-label="`立即测速 ${row.name}`"
              class="h-7 w-7 shrink-0 inline-flex items-center justify-center rounded-full text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-50"
            >
              <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': row.testing }" />
            </button>
          </div>
        </div>

        <!-- 常用站点行 -->
        <div
          v-for="row in siteRows"
          :key="row.host"
          class="flex items-center justify-between gap-3 py-2.5"
        >
          <div class="flex flex-col min-w-0">
            <div class="flex items-center gap-1.5 min-w-0">
              <span class="text-sm font-medium leading-none truncate">{{ row.name }}</span>
              <!-- 出网接口 switch：紧随网站名 -->
              <div
                class="shrink-0 inline-flex items-center rounded-full bg-muted p-0.5 text-xs"
                role="group"
                :aria-label="`${row.name} 测速接口`"
              >
                <button
                  @click="switchSiteIface(row, 'cf')"
                  :aria-pressed="row.iface === 'cf'"
                  :class="[
                    'px-1.5 py-0.5 rounded-full text-[10px] leading-none transition-all duration-150 cursor-pointer',
                    row.iface === 'cf'
                      ? 'bg-card text-foreground font-medium shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                  ]"
                >
                  CF
                </button>
                <button
                  @click="switchSiteIface(row, 'vercel')"
                  :aria-pressed="row.iface === 'vercel'"
                  :class="[
                    'px-1.5 py-0.5 rounded-full text-[10px] leading-none transition-all duration-150 cursor-pointer',
                    row.iface === 'vercel'
                      ? 'bg-card text-foreground font-medium shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                  ]"
                >
                  Vercel
                </button>
              </div>
            </div>
            <div class="text-xs text-muted-foreground font-mono mt-1 truncate">{{ row.host }}</div>
          </div>
          <div class="flex items-center gap-3 shrink-0">
            <LatencyBars :history="row.history" />
            <span class="w-14 shrink-0 text-right text-xs font-mono tabular-nums" :class="latestClass(row.history)">
              {{ latestText(row.history) }}
            </span>
            <button
              @click="testSiteRow(row)"
              :disabled="row.testing"
              title="立即测速"
              :aria-label="`立即测速 ${row.name}`"
              class="h-7 w-7 shrink-0 inline-flex items-center justify-center rounded-full text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-50"
            >
              <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': row.testing }" />
            </button>
          </div>
        </div>
        </div>

        <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 pt-2 text-xs text-muted-foreground">
          <span>每 10 分钟自动测速，柱条仅保留近 2 小时</span>
          <div class="flex items-center gap-3 text-[11px]">
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-full bg-emerald-500"></span>≤ 800ms
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-full bg-amber-500"></span>≤ 2000ms
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-full bg-red-500"></span>超时或失败
            </span>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>
