<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  CheckCircle2,
  ExternalLink,
  Power,
  RefreshCw,
  Server,
  Sparkles,
  Zap,
} from '@lucide/vue'
import LatencyBars from '@/components/common/LatencyBars.vue'
import { Button } from '@/components/ui/button'
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

/** 近 7 日柱状图数据：高度相对最大日归一（百分比）；缺勤日期补零保持 7 根柱 */
const weekBars = computed(() => {
  const days = traffic.value?.history ?? []
  const byDate = new Map(days.map((d) => [d.date, d]))
  const list: { date: string; cf: number; vercel: number }[] = []
  for (let i = 6; i >= 0; i--) {
    const date = new Date(Date.now() - i * 24 * 3600 * 1000)
    const key = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
    const d = byDate.get(key)
    list.push({
      date: key,
      cf: d ? bucketBytes(d.cf) : 0,
      vercel: d ? bucketBytes(d.vercel) : 0,
    })
  }
  const max = Math.max(1, ...list.map((d) => d.cf + d.vercel))
  return list.map((d) => ({
    ...d,
    label: d.date.slice(5).replace('-', '/'),
    cfPct: (d.cf / max) * 100,
    vercelPct: (d.vercel / max) * 100,
  }))
})

/** 占比环：今日 CF / Vercel 流量份额；今日为空时回落到累计口径 */
const shareDonut = computed(() => {
  if (!traffic.value) return { cfPct: 0, vercelPct: 0, total: 0, source: 'today' as const }
  const tCf = bucketBytes(traffic.value.today.cf)
  const tV = bucketBytes(traffic.value.today.vercel)
  const cf = tCf + tV > 0 ? tCf : bucketBytes(traffic.value.total.cf)
  const vercel = tCf + tV > 0 ? tV : bucketBytes(traffic.value.total.vercel)
  const sum = cf + vercel
  if (sum === 0) return { cfPct: 0, vercelPct: 0, total: 0, source: 'today' as const }
  return {
    cfPct: (cf / sum) * 100,
    vercelPct: (vercel / sum) * 100,
    total: sum,
    source: tCf + tV > 0 ? ('today' as const) : ('total' as const),
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
      date: new Date(Date.now() - ago * day).toISOString().slice(0, 10),
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

// ---- 链接状态（接口 + 常用站点；10 分钟轮询，仅显示近 2 小时时序）----
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
  { name: 'Anthropic', host: 'anthropic.com', iface: 'cf', history: [], testing: false },
])

const IFACE_CHOICE_PREFIX = 'pony-site-iface:'

function siteSeriesKey(host: string, iface: Iface): string {
  return `site:${host}:${iface}`
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
    row.history = loadLatencySeries(siteSeriesKey(row.host, row.iface))
  }
}

function switchSiteIface(row: SiteRow, iface: Iface): void {
  if (row.iface === iface) return
  saveLatencySeries(siteSeriesKey(row.host, row.iface), row.history)
  row.iface = iface
  try {
    localStorage.setItem(IFACE_CHOICE_PREFIX + row.host, iface)
  } catch { /* 忽略 */ }
  row.history = loadLatencySeries(siteSeriesKey(row.host, iface))
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
    saveLatencySeries(siteSeriesKey(row.host, row.iface), row.history)
  } catch (e) {
    row.history = appendLatencyPoint(row.history, { ts: Date.now(), ok: false, err: String(e) })
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
  if (!isRunning.value) return '加速已停止，点击上方按钮即可开启'
  return proxyMode.value === 'whitelist'
    ? '智能分流中：国内网络直连，海外服务经加速通道'
    : '全局加速中：全部网络流量经加速通道'
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
    toast.error('请输入 Cloudflare API Token')
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
    <!-- 未配置向导（零代码小白专属） -->
    <div v-if="!isConfigured" class="space-y-8">
      <div class="text-center py-4">
        <h1 class="text-2xl font-bold tracking-tight text-foreground">欢迎使用 Pony Proxy</h1>
        <p class="text-sm text-muted-foreground mt-1">请选择适合您的加速连接方案，1 分钟内即可完成配置</p>
      </div>

      <div class="grid grid-cols-2 gap-4">
        <button
          @click="setupTab = 'direct'"
          :class="[
            'p-5 text-left rounded-xl transition-all flex flex-col justify-between',
            setupTab === 'direct'
              ? 'bg-card shadow-sm ring-1 ring-primary'
              : 'bg-card/60 hover:bg-card',
          ]"
        >
          <div class="flex items-center gap-3">
            <div class="p-2.5 rounded-lg bg-primary/10 text-primary">
              <Sparkles class="h-5 w-5" />
            </div>
            <div>
              <div class="font-semibold text-base">方案 A：个人独立加速 (推荐)</div>
              <div class="text-xs text-muted-foreground mt-0.5">本机自给自足，速度快、专属独立通道</div>
            </div>
          </div>
          <div class="mt-4 text-xs text-primary font-medium flex items-center gap-1">
            仅需一键授权出口 <CheckCircle2 class="h-3.5 w-3.5" />
          </div>
        </button>

        <button
          @click="setupTab = 'chained'"
          :class="[
            'p-5 text-left rounded-xl transition-all flex flex-col justify-between',
            setupTab === 'chained'
              ? 'bg-card shadow-sm ring-1 ring-primary'
              : 'bg-card/60 hover:bg-card',
          ]"
        >
          <div class="flex items-center gap-3">
            <div class="p-2.5 rounded-lg bg-blue-500/10 text-blue-600">
              <Server class="h-5 w-5" />
            </div>
            <div>
              <div class="font-semibold text-base">方案 B：连接远端代理 / 跨端导入</div>
              <div class="text-xs text-muted-foreground mt-0.5">连接自己的 Linux Server 或粘贴分享口令</div>
            </div>
          </div>
          <div class="mt-4 text-xs text-blue-600 font-medium flex items-center gap-1">
            支持一键粘贴连接口令 <Zap class="h-3.5 w-3.5" />
          </div>
        </button>
      </div>

      <!-- 方案 A 表单 -->
      <div v-if="setupTab === 'direct'" class="bg-card rounded-xl p-6 space-y-4">
        <div class="flex items-center justify-between">
          <Label class="text-sm font-medium">Cloudflare API Token 授权码</Label>
          <button
            type="button"
            @click="openExternalUrl('https://dash.cloudflare.com/profile/api-tokens')"
            class="text-xs text-primary hover:underline flex items-center gap-1 cursor-pointer bg-transparent border-0 p-0"
          >
            点击直达获取 Token <ExternalLink class="h-3 w-3" />
          </button>
        </div>
        <Input
          v-model="cfToken"
          type="password"
          placeholder="粘贴您的 Cloudflare API Token"
          class="font-mono text-sm"
        />
        <p class="text-xs text-muted-foreground">
          💡 提示：用于自动在云端部署个人加速节点，凭据将安全保存在本机 Windows 凭据管理器中，绝不上报。
        </p>
        <div class="pt-2">
          <Button
            @click="submitDirectSetup"
            :disabled="isSubmitting || !cfToken.trim()"
            class="w-full h-11 text-sm font-semibold"
          >
            <Zap v-if="!isSubmitting" class="h-4 w-4 mr-2" />
            <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
            {{ isSubmitting ? '正在初始化加速节点...' : '一键开启个人独立加速' }}
          </Button>
        </div>
      </div>

      <!-- 方案 B 表单 -->
      <div v-if="setupTab === 'chained'" class="bg-card rounded-xl p-6 space-y-5">
        <div class="space-y-2">
          <Label class="text-sm font-medium">方式 1：粘贴一键连接口令 (最快捷)</Label>
          <Input
            v-model="syncUriInput"
            placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
            class="font-mono text-xs"
          />
          <p class="text-xs text-muted-foreground">
            可直接粘贴从 Linux Server（运行 <code>pproxy user add</code> 或 <code>pproxy sync export</code>）导出的口令。
          </p>
        </div>

        <div class="relative flex items-center py-2">
          <div class="flex-grow border-t border-border"></div>
          <span class="flex-shrink mx-4 text-xs text-muted-foreground uppercase">或者手动填写参数</span>
          <div class="flex-grow border-t border-border"></div>
        </div>

        <div class="grid grid-cols-2 gap-4">
          <div class="col-span-2 space-y-1.5">
            <Label class="text-xs">代理服务器地址 (如 192.168.1.100:8899)</Label>
            <Input v-model="remoteHost" placeholder="IP 或域名 : 端口" class="text-sm" />
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

        <div class="pt-2">
          <Button
            @click="submitImportOrChained"
            :disabled="isSubmitting || (!syncUriInput.trim() && !remoteHost.trim())"
            class="w-full h-11 text-sm font-semibold"
          >
            <Server v-if="!isSubmitting" class="h-4 w-4 mr-2" />
            <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
            {{ isSubmitting ? '正在验证连接...' : '连接远端代理并开启' }}
          </Button>
        </div>
      </div>
    </div>

    <!-- 已配置：极简主界面 -->
    <div v-else class="space-y-10">
      <!-- 头部主控：圆形电源按钮 + 模式 switch + 弱化状态文字 -->
      <section class="flex flex-col items-center pt-8">
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
        <div class="mt-6 inline-flex items-center rounded-full bg-muted p-1" role="group" aria-label="加速模式">
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

        <p class="mt-3 text-xs text-muted-foreground">{{ statusText }}</p>
      </section>

      <!-- 用量统计：近 7 日双出口柱状图 + 占比环 + 指标 -->
      <section class="space-y-4">
        <div class="flex items-center justify-between">
          <h3 class="text-sm font-semibold">用量统计</h3>
          <div class="flex items-center gap-4 text-xs text-muted-foreground">
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2.5 w-2.5 rounded-sm bg-[#f6821f]"></span>Cloudflare
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2.5 w-2.5 rounded-sm bg-foreground/80"></span>Vercel
            </span>
          </div>
        </div>

        <div class="grid grid-cols-[1fr_auto] items-center gap-8">
          <!-- 近 7 日流量柱状图（堆叠：CF 橙 + Vercel 墨） -->
          <div>
            <div class="flex items-end gap-2 h-28">
              <div
                v-for="day in weekBars"
                :key="day.date"
                class="flex-1 flex flex-col justify-end h-full group"
                :title="`${day.date}\nCloudflare ${formatBytes(day.cf)}\nVercel ${formatBytes(day.vercel)}`"
              >
                <div class="w-full rounded-t-md bg-muted/50 flex flex-col-reverse overflow-hidden" style="height: 100%">
                  <div class="bg-[#f6821f] transition-all duration-300" :style="{ height: day.cfPct + '%' }"></div>
                  <div class="bg-foreground/80 transition-all duration-300" :style="{ height: day.vercelPct + '%' }"></div>
                </div>
              </div>
            </div>
            <div class="flex gap-2 mt-1.5">
              <span
                v-for="day in weekBars"
                :key="day.date"
                class="flex-1 text-center text-[10px] text-muted-foreground tabular-nums"
              >
                {{ day.label }}
              </span>
            </div>
          </div>

          <!-- 出口占比环 -->
          <div class="flex items-center gap-5">
            <div class="relative h-28 w-28">
              <svg viewBox="0 0 42 42" class="h-full w-full -rotate-90">
                <circle cx="21" cy="21" r="15.9155" fill="none" class="stroke-muted" stroke-width="5" />
                <circle
                  v-if="shareDonut.cfPct > 0"
                  cx="21" cy="21" r="15.9155" fill="none"
                  stroke="#f6821f" stroke-width="5" stroke-linecap="round"
                  :stroke-dasharray="`${shareDonut.cfPct} 100`"
                />
                <circle
                  v-if="shareDonut.vercelPct > 0"
                  cx="21" cy="21" r="15.9155" fill="none"
                  class="stroke-foreground/80" stroke-width="5" stroke-linecap="round"
                  :stroke-dasharray="`${shareDonut.vercelPct} 100`"
                  :stroke-dashoffset="-shareDonut.cfPct"
                />
              </svg>
              <div class="absolute inset-0 flex flex-col items-center justify-center">
                <span class="text-sm font-semibold tabular-nums">{{ formatBytes(shareDonut.total) }}</span>
                <span class="text-[10px] text-muted-foreground">{{ shareDonut.source === 'today' ? '今日' : '累计' }}</span>
              </div>
            </div>
            <div class="space-y-2 text-xs">
              <div class="flex items-center gap-2">
                <span class="h-2.5 w-2.5 rounded-full bg-[#f6821f]"></span>
                <span class="text-muted-foreground">Cloudflare</span>
                <span class="font-medium tabular-nums">{{ shareDonut.cfPct.toFixed(0) }}%</span>
              </div>
              <div class="flex items-center gap-2">
                <span class="h-2.5 w-2.5 rounded-full bg-foreground/80"></span>
                <span class="text-muted-foreground">Vercel</span>
                <span class="font-medium tabular-nums">{{ shareDonut.vercelPct.toFixed(0) }}%</span>
              </div>
              <div class="pt-1 text-muted-foreground">
                今日请求 <span class="text-foreground font-medium tabular-nums">{{ todayTotals.requests }}</span>
                 · 累计 <span class="text-foreground font-medium tabular-nums">{{ formatBytes(totalBytes) }}</span>
              </div>
            </div>
          </div>
        </div>
        <p class="text-xs text-muted-foreground">按出口归账：Cloudflare 与 Vercel 分别统计，按日本地计数，柱条为近 7 日流量。</p>
      </section>

      <!-- 链接状态：接口 + 常用站点连通性 -->
      <section class="space-y-1">
        <div class="flex items-center justify-between mb-2">
          <h3 class="text-sm font-semibold">链接状态</h3>
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

        <div class="grid grid-cols-1 lg:grid-cols-2 gap-x-8">
        <!-- 出网接口行 -->
        <div
          v-for="row in ifaceRows"
          :key="row.id"
          class="flex items-center gap-3 py-2.5"
        >
          <div class="w-32 shrink-0">
            <div class="text-sm">{{ row.name }}</div>
            <div class="text-xs text-muted-foreground">{{ row.endpoint }}</div>
          </div>
          <div class="flex-1 min-w-0">
            <LatencyBars :history="row.history" />
          </div>
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

        <!-- 常用站点行 -->
        <div
          v-for="row in siteRows"
          :key="row.host"
          class="flex items-center gap-3 py-2.5"
        >
          <div class="w-32 shrink-0">
            <div class="text-sm">{{ row.name }}</div>
            <div class="text-xs text-muted-foreground">{{ row.host }}</div>
          </div>
          <div class="flex-1 min-w-0">
            <LatencyBars :history="row.history" />
          </div>
          <!-- 出网接口 switch -->
          <div class="shrink-0 inline-flex items-center rounded-full bg-muted p-0.5 text-xs" role="group" :aria-label="`${row.name} 测速接口`">
            <button
              @click="switchSiteIface(row, 'cf')"
              :aria-pressed="row.iface === 'cf'"
              :class="[
                'px-2 py-1 rounded-full transition-all duration-150 cursor-pointer',
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
                'px-2 py-1 rounded-full transition-all duration-150 cursor-pointer',
                row.iface === 'vercel'
                  ? 'bg-card text-foreground font-medium shadow-sm'
                  : 'text-muted-foreground hover:text-foreground',
              ]"
            >
              Vercel
            </button>
          </div>
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

        <p class="text-xs text-muted-foreground pt-2">每 10 分钟自动测速，柱条仅保留近 2 小时；绿 ≤ 800ms，黄 ≤ 2000ms，红为超时或失败。</p>
      </section>
    </div>
  </div>
</template>
