<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  Power,
  RefreshCw,
  Server,
  ShieldCheck,
  Zap,
} from '@lucide/vue'
import LatencyBars from '@/components/common/LatencyBars.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useToast } from '@/composables/useToast'
import { importConnectCode, isTauri, isUserAdmin, loadTunnelConfig, parseGateInput, saveTunnelToken, type UserClaims } from '@/lib/config'
import {
  appendLatencyPoint,
  loadLatencySeries,
  saveLatencySeries,
  type LatencyPoint,
} from '@/lib/latencyHistory'
import {
  appendSpeedSample,
  calculateSmoothedSpeed,
  formatSpeedParts,
  generateSpeedWaveform,
  type SpeedSample,
} from '@/lib/speedTracker'
import { buildMergedUsageChart, formatBytes, localDateKey, localHourKey } from '@/lib/usageChart'
import { isConfigured, setAppConfigured, refreshAppConfigured } from '@/composables/useAppConfig'

const toast = useToast()

// 运行状态
const isRunning = ref(false)
const isToggling = ref(false)
const proxyMode = ref<'whitelist' | 'global'>('whitelist')
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
const syncPassphrase = ref('')
const remoteHost = ref('')
const remoteUser = ref('')
const remotePass = ref('')
const isSubmitting = ref(false)

// ---- 用量统计（引擎本地计数：按出口归账，含近 7 日与近 24 小时历史）----
interface TrafficBucket {
  requests: number
  bytes_up: number
  bytes_down: number
}
interface DayUsage {
  date: string
  cf: TrafficBucket
  vercel: TrafficBucket
  rn?: TrafficBucket
  upstream?: TrafficBucket
}
interface HourUsage {
  hour: string
  cf: TrafficBucket
  vercel: TrafficBucket
  rn?: TrafficBucket
  upstream?: TrafficBucket
}
interface TrafficStats {
  today: { cf: TrafficBucket; vercel: TrafficBucket; rn?: TrafficBucket; upstream?: TrafficBucket }
  total: { cf: TrafficBucket; vercel: TrafficBucket; rn?: TrafficBucket; upstream?: TrafficBucket }
  history: DayUsage[]
  hourly?: HourUsage[]
}
const traffic = ref<TrafficStats | null>(null)
const usageDimension = ref<'7d' | '24h'>('24h')
const userClaims = ref<UserClaims | null>(null)
const isAdmin = computed(() => isUserAdmin(userClaims.value, cfToken.value))

interface ClusterNodeItem {
  name: string
  address: string
  online: boolean
  latency_ms?: number
}
const clusterNodes = ref<ClusterNodeItem[]>([])
const isRefreshingCluster = ref(false)

function bucketBytes(b?: TrafficBucket): number {
  if (!b) return 0
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

/** 近 24 小时双出口用量：缺勤小时补零保持 24 根柱，抽样标轴 */
const hourItems = computed<UsageDay[]>(() => {
  const hours = traffic.value?.hourly ?? []
  const byHour = new Map(hours.map((h) => [h.hour, h]))
  const list: UsageDay[] = []
  const now = Date.now()
  const oneHour = 3600 * 1000
  for (let i = 23; i >= 0; i--) {
    const d = new Date(now - i * oneHour)
    const key = localHourKey(d)
    const h = byHour.get(key)
    const hh = String(d.getHours()).padStart(2, '0')
    let label = ''
    if (i === 0) {
      label = '现在'
    } else if (d.getHours() % 6 === 0) {
      label = `${hh}:00`
    }
    list.push({
      date: key,
      label,
      cfReq: h?.cf.requests ?? 0,
      vReq: h?.vercel.requests ?? 0,
      cfBytes: h ? bucketBytes(h.cf) : 0,
      vBytes: h ? bucketBytes(h.vercel) : 0,
    })
  }
  return list
})

const activeUsageItems = computed(() => {
  return usageDimension.value === '7d' ? weekDays.value : hourItems.value
})

/**
 * 极简合并用量图模型：无纵轴、无横线、紧凑间距、调用次数直观可见、合并实时网速波形
 */
const mergedChart = computed(() => {
  return buildMergedUsageChart(activeUsageItems.value, 320, 72)
})

const todayTotals = computed(() => {
  if (!traffic.value) return { bytes: 0, requests: 0 }
  const t = traffic.value.today
  return {
    bytes: bucketBytes(t.cf) + bucketBytes(t.vercel) + bucketBytes(t.rn) + bucketBytes(t.upstream),
    requests: (t.cf?.requests ?? 0) + (t.vercel?.requests ?? 0) + (t.rn?.requests ?? 0) + (t.upstream?.requests ?? 0),
  }
})

const totalBytes = computed(() => {
  if (!traffic.value) return 0
  return (
    bucketBytes(traffic.value.total.cf) +
    bucketBytes(traffic.value.total.vercel) +
    bucketBytes(traffic.value.total.rn) +
    bucketBytes(traffic.value.total.upstream)
  )
})

// ---- 出口R (VPS) 专属流量监控指标 (每月 500GB 额度) ----
const VPS_MONTHLY_LIMIT_BYTES = 500 * 1024 * 1024 * 1024 // 500 GB

function getUserStorageKey(sub: string): string {
  return `pony-user-base-bytes:${sub}`
}

function getUserBaselineBytes(sub: string, currentTotal: number): number {
  if (typeof localStorage === 'undefined') return 0
  const k = getUserStorageKey(sub)
  const saved = localStorage.getItem(k)
  if (saved !== null) {
    const num = Number(saved)
    return isNaN(num) ? 0 : num
  }
  // 初次接入该用户时，记录当前机器已产生的 total 作为此用户的起始基线
  localStorage.setItem(k, String(currentTotal))
  return currentTotal
}

const userQuotaDisplay = computed(() => {
  if (!userClaims.value) return null
  const quota = userClaims.value.quota_bytes
  const currentTotal = totalBytes.value
  const baseline = getUserBaselineBytes(userClaims.value.sub, currentTotal)
  const used = Math.max(0, currentTotal - baseline)
  const pct = quota > 0 ? Math.min(100, Math.max(0, (used / quota) * 100)) : 0
  const expDate = new Date(userClaims.value.exp * 1000)
  return {
    sub: userClaims.value.sub,
    name: userClaims.value.name || userClaims.value.sub,
    usedBytes: used,
    quotaBytes: quota,
    maxConns: userClaims.value.max_conns,
    percent: pct.toFixed(1),
    expStr: expDate.toLocaleDateString(),
    isExpired: Date.now() > userClaims.value.exp * 1000,
  }
})

const vpsUsage = computed(() => {
  const t = traffic.value
  const todayRn = t?.today.rn
  const totalRn = t?.total.rn
  const todayBytes = bucketBytes(todayRn)
  const todayReqs = todayRn?.requests ?? 0
  const monthBytes = bucketBytes(totalRn)
  const pct = Math.min(100, Math.max(0, (monthBytes / VPS_MONTHLY_LIMIT_BYTES) * 100))
  return {
    todayBytes,
    todayReqs,
    monthBytes,
    monthLimitBytes: VPS_MONTHLY_LIMIT_BYTES,
    percent: pct.toFixed(2),
  }
})

const currentSpeed = ref<{ up: number; down: number }>({ up: 0, down: 0 })
const speedHistory = ref<SpeedSample[]>([])
const prevTotals = ref<{ up: number; down: number; ts: number } | null>(null)

const speedChart = computed(() => {
  return generateSpeedWaveform(speedHistory.value, 320, 72, 24, 6, 16)
})

const speedDownParts = computed(() => formatSpeedParts(currentSpeed.value.down))
const speedUpParts = computed(() => formatSpeedParts(currentSpeed.value.up))

async function refreshTraffic(): Promise<void> {
  const now = Date.now()
  if (!isTauri()) {
    const day = 24 * 3600 * 1000
    const hour = 3600 * 1000
    const mk = (ago: number, cfB: number, vB: number): DayUsage => ({
      date: localDateKey(new Date(Date.now() - ago * day)),
      cf: { requests: Math.round(cfB / 400_000), bytes_up: Math.round(cfB * 0.08), bytes_down: cfB },
      vercel: { requests: Math.round(vB / 500_000), bytes_up: Math.round(vB * 0.06), bytes_down: vB },
    })
    const mkHour = (ago: number, cfB: number, vB: number): HourUsage => ({
      hour: localHourKey(new Date(Date.now() - ago * hour)),
      cf: { requests: Math.round(cfB / 400_000), bytes_up: Math.round(cfB * 0.08), bytes_down: cfB },
      vercel: { requests: Math.round(vB / 500_000), bytes_up: Math.round(vB * 0.06), bytes_down: vB },
    })

    const mockDownDelta = isRunning.value ? Math.floor(Math.random() * 800_000) + 120_000 : 0
    const mockUpDelta = isRunning.value ? Math.floor(mockDownDelta * 0.08) : 0
    const baseTodayDown = 38_000_000 + (traffic.value ? 0 : 0)

    const hourlyMock: HourUsage[] = []
    for (let i = 23; i >= 1; i--) {
      const baseB = ((i % 5) + 1) * 2_500_000
      hourlyMock.push(mkHour(i, baseB, Math.round(baseB * 0.3)))
    }
    hourlyMock.push({
      hour: localHourKey(new Date(now)),
      cf: { requests: 96, bytes_up: 2_400_000 + mockUpDelta, bytes_down: baseTodayDown + mockDownDelta },
      vercel: { requests: 32, bytes_up: 800_000, bytes_down: 12_000_000 },
    })

    traffic.value = {
      today: {
        cf: { requests: 96, bytes_up: 2_400_000 + mockUpDelta, bytes_down: baseTodayDown + mockDownDelta },
        vercel: { requests: 32, bytes_up: 800_000, bytes_down: 12_000_000 },
      },
      total: {
        cf: { requests: 3100, bytes_up: 72_000_000 + mockUpDelta, bytes_down: 960_000_000 + mockDownDelta },
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
      hourly: hourlyMock,
    }

    if (isRunning.value) {
      currentSpeed.value = { down: mockDownDelta, up: mockUpDelta }
      speedHistory.value = appendSpeedSample(speedHistory.value, { down: mockDownDelta, up: mockUpDelta, ts: now })
    } else {
      currentSpeed.value = { down: 0, up: 0 }
      speedHistory.value = appendSpeedSample(speedHistory.value, { down: 0, up: 0, ts: now })
    }
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const stats = await invoke<TrafficStats>('proxy_traffic_stats')
    traffic.value = stats

    const totalUp =
      (stats.total.cf?.bytes_up ?? 0) +
      (stats.total.vercel?.bytes_up ?? 0) +
      (stats.total.upstream?.bytes_up ?? 0)
    const totalDown =
      (stats.total.cf?.bytes_down ?? 0) +
      (stats.total.vercel?.bytes_down ?? 0) +
      (stats.total.upstream?.bytes_down ?? 0)

    if (prevTotals.value && isRunning.value) {
      const sp = calculateSmoothedSpeed(
        prevTotals.value,
        { up: totalUp, down: totalDown, ts: now },
        currentSpeed.value,
      )
      currentSpeed.value = sp
      speedHistory.value = appendSpeedSample(speedHistory.value, { down: sp.down, up: sp.up, ts: now })
    } else {
      currentSpeed.value = { down: 0, up: 0 }
      speedHistory.value = appendSpeedSample(speedHistory.value, { down: 0, up: 0, ts: now })
    }
    prevTotals.value = { up: totalUp, down: totalDown, ts: now }
  } catch (e) {
    console.error('Failed to load traffic stats:', e)
  }
}

// ---- 连接状态（接口 + 常用站点；10 分钟轮询，仅显示近 2 小时时序）----
type Iface = 'rn' | 'cf' | 'vercel'

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
  { id: 'rn', name: '出口R', endpoint: '', history: [], testing: false },
  { id: 'cf', name: '出口C', endpoint: '', history: [], testing: false },
  { id: 'vercel', name: '出口V', endpoint: '', history: [], testing: false },
])

const userProxyRow = ref<IfaceRow>({
  id: 'rn',
  name: '代理状态',
  endpoint: '',
  history: [],
  testing: false,
})

async function testUserProxyRow(): Promise<void> {
  if (userProxyRow.value.testing) return
  if (!isRunning.value) {
    // 代理未开启时无需发起探测
    return
  }
  userProxyRow.value.testing = true
  try {
    const point = await probeSite('google.com')
    userProxyRow.value.history = appendLatencyPoint(userProxyRow.value.history, point)
    saveLatencySeries('iface:user_proxy', userProxyRow.value.history)
  } catch (e) {
    userProxyRow.value.history = appendLatencyPoint(userProxyRow.value.history, { ts: Date.now(), ok: false, err: String(e) })
    saveLatencySeries('iface:user_proxy', userProxyRow.value.history)
  } finally {
    userProxyRow.value.testing = false
  }
}

async function refreshClusterNodes(): Promise<void> {
  if (isRefreshingCluster.value) return
  isRefreshingCluster.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      const res = (await invoke('proxy_cluster_nodes_get')) as { nodes: ClusterNodeItem[] }
      if (res && Array.isArray(res.nodes)) {
        clusterNodes.value = res.nodes
      }
    } else {
      clusterNodes.value = [
        { name: 'devserver (主力节点)', address: '100.95.193.103:8899', online: true, latency_ms: 18 },
        { name: 'preprod (备灾节点1)', address: '100.97.143.121:8899', online: true, latency_ms: 32 },
        { name: 'tencent (备灾节点2)', address: '100.105.241.39:8899', online: true, latency_ms: 25 },
      ]
    }
  } catch (e) {
    console.error('Failed to refresh cluster nodes:', e)
  } finally {
    isRefreshingCluster.value = false
  }
}

const siteRows = ref<SiteRow[]>([
  { name: 'Google', host: 'google.com', iface: 'vercel', history: [], testing: false },
  { name: 'Anthropic', host: 'api.anthropic.com', iface: 'rn', history: [], testing: false },
  { name: 'GitHub', host: 'github.com', iface: 'cf', history: [], testing: false },
  { name: 'OpenAI', host: 'openai.com', iface: 'vercel', history: [], testing: false },
  { name: 'X', host: 'x.com', iface: 'cf', history: [], testing: false },
])

function siteSeriesKey(host: string): string {
  return `site:${host}`
}

function loadAllHistories(): void {
  for (const row of ifaceRows.value) {
    row.history = loadLatencySeries(`iface:${row.id}`)
  }
  for (const row of siteRows.value) {
    // P2-5：站点测速改为经本地引擎（引擎自动选出口），不再持久化用户手动 C/V 选择；
    // 兼容读取旧版按出口存储的时序数据（迁移到统一 site:host 键）。
    let hist = loadLatencySeries(siteSeriesKey(row.host))
    if (!hist.length) {
      const oldHist = loadLatencySeries(`site:${row.host}:${row.iface}`)
      if (oldHist.length) {
        hist = oldHist
        saveLatencySeries(siteSeriesKey(row.host), hist)
      }
    }
    // 清洗掉因启动早期代理尚未就绪（ENGINE_ON 未置位）导致的假失败历史点
    hist = hist.filter((p) => !p.err?.includes('代理未启用'))
    row.history = hist
  }
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

async function probeSite(host: string): Promise<LatencyPoint> {
  if (!isTauri()) {
    await new Promise((r) => setTimeout(r, 400))
    const ms = Math.floor(Math.random() * 900) + 150
    return { ts: Date.now(), ok: true, ms }
  }
  // P2-5：站点拨测走本地引擎真实分流（命中白名单/全局 → 隧道出网；未命中 → 直连），
  // 与「链接状态 = 实际可用性」一致，而非绕过引擎直拨 gate 的假绿。
  const { invoke } = await import('@tauri-apps/api/core')
  const r = await invoke<{ ok: boolean; ms: number; error?: string }>('proxy_test_site_local', {
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

async function testSiteRow(row: SiteRow, isManual = false): Promise<void> {
  if (row.testing) return
  if (!isRunning.value) {
    if (isManual) toast.info('请先开启加速，再测试站点延迟')
    return
  }
  row.testing = true
  try {
    const point = await probeSite(row.host)
    if (point.err?.includes('代理未启用')) return
    row.history = appendLatencyPoint(row.history, point)
    saveLatencySeries(siteSeriesKey(row.host), row.history)
  } catch (e) {
    const errStr = String(e)
    if (errStr.includes('代理未启用')) return
    row.history = appendLatencyPoint(row.history, { ts: Date.now(), ok: false, err: errStr })
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
    const tasks: Promise<void>[] = []
    if (isAdmin.value) {
      tasks.push(...ifaceRows.value.map((r) => testIfaceRow(r)))
      void refreshClusterNodes()
    } else {
      tasks.push(testUserProxyRow())
    }
    if (isRunning.value) {
      tasks.push(...siteRows.value.map((r) => testSiteRow(r)))
    }
    await Promise.all(tasks)
  } finally {
    isTestingAll.value = false
  }
}

function latestPoint(history: LatencyPoint[]): LatencyPoint | undefined {
  return history[history.length - 1]
}

function latestText(history: LatencyPoint[]): string {
  if (!isRunning.value) return '—'
  const p = latestPoint(history)
  if (!p) return '—'
  if (!p.ok) return '失败'
  return `${p.ms ?? 0}ms`
}

function latestClass(history: LatencyPoint[]): string {
  if (!isRunning.value) return 'text-muted-foreground'
  const p = latestPoint(history)
  if (!p) return 'text-muted-foreground'
  if (!p.ok) return 'text-rose-600'
  const ms = p.ms ?? 0
  if (ms <= 2000) return 'text-emerald-600'
  if (ms <= 5000) return 'text-amber-600'
  return 'text-rose-600'
}

// 状态弱化文字（按钮之下）
const statusText = computed(() => {
  if (!isRunning.value) return '加速已停止，点击上方开启'
  return proxyMode.value === 'whitelist'
    ? '海外加速，国内直连'
    : '全部流量走加速'
})

// 轮询：测速 10 分钟一轮（对齐 2 小时 12 根柱条）；用量与实时速率 1 秒一刷
const POLL_TEST_MS = 10 * 60 * 1000
const POLL_TRAFFIC_MS = 1000
let testTimer: ReturnType<typeof setInterval> | undefined
let trafficTimer: ReturnType<typeof setInterval> | undefined
let unlistenStatus: (() => void) | undefined
let unlistenMode: (() => void) | undefined
let unlistenReady: (() => void) | undefined

onMounted(async () => {
  loadAllHistories()
  await refreshStatus()
  void refreshTraffic()
  if (isRunning.value) {
    void runAllTests()
  } else if (isConfigured.value) {
    // 代理尚未就绪时仅测出口网关，等待代理就绪事件触发站点真实拨测
    void Promise.all(ifaceRows.value.map((r) => testIfaceRow(r)))
  }
  testTimer = setInterval(() => void runAllTests(), POLL_TEST_MS)
  trafficTimer = setInterval(() => void refreshTraffic(), POLL_TRAFFIC_MS)

  if (isTauri()) {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      unlistenStatus = await listen<{ on: boolean; mode?: 'whitelist' | 'global' }>('proxy-status-changed', (event) => {
        const wasRunning = isRunning.value
        isRunning.value = event.payload.on
        if (event.payload.mode) proxyMode.value = event.payload.mode
        if (!wasRunning && event.payload.on) {
          void runAllTests()
        }
      })
      unlistenMode = await listen<{ mode: 'whitelist' | 'global' }>('proxy-mode-changed', (event) => {
        proxyMode.value = event.payload.mode
      })
      unlistenReady = await listen<{ ready?: boolean; on?: boolean; mode?: 'whitelist' | 'global' }>('proxy-ready', (event) => {
        const wasRunning = isRunning.value
        if (typeof event.payload.on === 'boolean') isRunning.value = event.payload.on
        if (event.payload.mode) proxyMode.value = event.payload.mode
        if (!wasRunning && isRunning.value) {
          void runAllTests()
        }
      })
    } catch {}
  }
})

onUnmounted(() => {
  if (testTimer) clearInterval(testTimer)
  if (trafficTimer) clearInterval(trafficTimer)
  if (unlistenStatus) unlistenStatus()
  if (unlistenMode) unlistenMode()
  if (unlistenReady) unlistenReady()
})

async function refreshStatus() {
  if (!isTauri()) {
    await refreshAppConfigured()
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

    const tunnel = await loadTunnelConfig()
    userClaims.value = tunnel.userClaims ?? null
    if (isAdmin.value) {
      void refreshClusterNodes()
    }
  } catch (e) {
    console.error('Failed to get status:', e)
  }
}

async function toggleProxy() {
  if (isToggling.value) return
  isToggling.value = true
  try {
    if (!isTauri()) {
      isRunning.value = !isRunning.value
      return
    }
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
        toast.error('开启失败', '出网拨测未通过，系统代理未启用。请检查隧道令牌或远端服务器配置后重试')
        return
      }
      toast.success('智能加速已开启！')
      void runAllTests()
    }
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '操作失败')
  } finally {
    isToggling.value = false
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
  const raw = cfToken.value.trim()
  if (!raw) {
    toast.error('请粘贴隧道令牌或连接口令')
    return
  }
  isSubmitting.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      const parsed = parseGateInput(raw)
      if (raw.startsWith('pony-gate://')) {
        if (parsed?.kind !== 'code') throw new Error('连接口令已损坏，请向提供方重新索取')
        const res = await importConnectCode(raw)
        if (parsed.official === false) {
          // 非官方端点：只保留警示，不叠加成功 toast
          toast.info('已导入，但端点不是官方域名，请确认来源可信', res.url)
          setAppConfigured(true)
          await refreshStatus()
          return
        }
      } else {
        // 裸授权码：仅保存令牌（无端点配置时后端自动补默认双 gate）
        await saveTunnelToken(raw)
      }
      await invoke('proxy_mode_switch', {
        modeType: 'direct',
        config: { worker_url: 'https://edge.example.com' },
      })
    }
    toast.success('配置成功！已准备就绪。')
    setAppConfigured(true)
    await refreshStatus()
    await toggleProxy()
  } catch (e: any) {
    toast.error('配置失败：' + (typeof e === 'string' ? e : e?.message ?? '未知错误'))
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
          passphrase: syncPassphrase.value.trim() || null,
        })) as any
        toast.success(res.message || '导入成功！')
      } else {
        toast.success('口令导入成功！')
      }
      setAppConfigured(true)
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
        // P0：空密码禁止穿透覆盖（后端同样拒绝，但前端先拦以给出明确提示）
        const pwd = remotePass.value.trim()
        if (!pwd) {
          toast.error('远端密码不能为空', '留空不会保留旧密码，请键入密码后重试')
          return
        }
        await invoke('proxy_mode_switch', {
          modeType: 'chained',
          config: {
            remote_host: remoteHost.value.trim(),
            username: remoteUser.value.trim(),
            password: pwd,
          },
        })
      }
      toast.success('远端代理已连接！')
      setAppConfigured(true)
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
  <div
    class="h-full overflow-y-auto"
    :class="isConfigured ? 'p-6 max-w-4xl mx-auto' : 'p-6 w-full flex flex-col justify-center items-center'"
  >
    <!-- 初始设置向导：极简接入（主输入令牌接入，下方弱化显示接入其他代理） -->
    <div v-if="!isConfigured" class="w-full space-y-6 py-6 max-w-md mx-auto">
      <div class="text-center space-y-2">
        <h1 class="text-2xl font-bold tracking-tight text-foreground">欢迎使用 Pony Proxy</h1>
      </div>

      <!-- 核心接入卡片：极简输入接入令牌 -->
      <Card v-if="setupTab === 'direct'" class="border-border shadow-sm">
        <CardContent class="pt-6 space-y-4">
          <div class="space-y-2">
            <Label class="text-xs font-medium flex items-center justify-between">
              <span>粘贴令牌</span>
            </Label>
            <Input
              v-model="cfToken"
              type="password"
              placeholder="usr_live_***"
              class="font-mono text-sm h-11"
              autofocus
              @keyup.enter="submitDirectSetup"
            />
          </div>

          <p class="text-xs text-muted-foreground flex items-center gap-1.5">
            <ShieldCheck class="h-3.5 w-3.5 text-emerald-600 shrink-0" />
            仅保存在本机系统凭据库，自动获取与同步出海隧道
          </p>

          <Button
            @click="submitDirectSetup"
            :disabled="isSubmitting || !cfToken.trim()"
            class="w-full h-11 text-sm font-semibold"
          >
            <Zap v-if="!isSubmitting" class="h-4 w-4 mr-2" />
            <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
            {{ isSubmitting ? '正在连接…' : '立即接入' }}
          </Button>
        </CardContent>
      </Card>

      <!-- 次级方案：接入其他远端代理（较弱显示） -->
      <Card v-else class="border-border shadow-sm">
        <CardHeader class="pb-3 flex flex-row items-center justify-between space-y-0">
          <div>
            <CardTitle class="text-sm font-semibold">连接自建或远端代理</CardTitle>
            <CardDescription class="text-xs mt-0.5">支持一键同步口令或手动输入服务器</CardDescription>
          </div>
          <button
            type="button"
            @click="setupTab = 'direct'"
            class="text-xs text-primary hover:underline cursor-pointer"
          >
            返回令牌接入
          </button>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="space-y-1.5">
            <Label class="text-xs font-medium">一键连接口令</Label>
            <Input
              v-model="syncUriInput"
              placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
              class="font-mono text-xs"
              @keyup.enter="submitImportOrChained"
            />
            <Input
              v-model="syncPassphrase"
              type="password"
              placeholder="同步口令（可选）"
              class="font-mono text-xs"
              @keyup.enter="submitImportOrChained"
            />
          </div>

          <div class="relative flex items-center">
            <div class="flex-grow border-t border-border"></div>
            <span class="flex-shrink mx-4 text-xs text-muted-foreground">或手动填写</span>
            <div class="flex-grow border-t border-border"></div>
          </div>

          <div class="grid grid-cols-2 gap-3">
            <div class="col-span-2 space-y-1.5">
              <Label class="text-xs">服务器地址</Label>
              <Input v-model="remoteHost" placeholder="IP 或域名 : 端口" class="text-sm" @keyup.enter="submitImportOrChained" />
            </div>
            <div class="space-y-1.5">
              <Label class="text-xs">用户名</Label>
              <Input v-model="remoteUser" placeholder="用户名" class="text-sm" @keyup.enter="submitImportOrChained" />
            </div>
            <div class="space-y-1.5">
              <Label class="text-xs">密码</Label>
              <Input v-model="remotePass" type="password" placeholder="密码" class="text-sm" @keyup.enter="submitImportOrChained" />
            </div>
          </div>

          <Button
            @click="submitImportOrChained"
            :disabled="isSubmitting || (!syncUriInput.trim() && !remoteHost.trim())"
            class="w-full h-10 text-sm font-medium"
          >
            <Server v-if="!isSubmitting" class="h-4 w-4 mr-2" />
            <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
            {{ isSubmitting ? '正在验证…' : '连接' }}
          </Button>
        </CardContent>
      </Card>

      <!-- 较弱显示的接入其他代理入口 -->
      <div v-if="setupTab === 'direct'" class="text-center pt-2">
        <button
          type="button"
          @click="setupTab = 'chained'"
          class="text-xs text-muted-foreground/75 hover:text-foreground inline-flex items-center gap-1.5 transition-colors cursor-pointer"
        >
          <Server class="h-3.5 w-3.5" />
          <span>接入自建服务器或其他远端代理</span>
        </button>
      </div>
    </div>

    <!-- 已配置：极简主界面 -->
    <div v-else class="space-y-16">
      <!-- 头部：主控在左侧空间居中，用量统计靠右收紧 -->
      <div class="flex items-center justify-between gap-8 pt-4">
        <!-- 主控：圆形电源按钮 + 模式 switch + 弱化状态文字（在左侧可用空间中完全居中） -->
        <section class="flex-1 flex flex-col items-center justify-center">
          <button
            @click="toggleProxy"
            :disabled="isToggling"
            :title="isToggling ? '切换中…' : isRunning ? '点击关闭加速' : '点击开启加速'"
            :aria-label="isToggling ? '正在切换加速状态' : isRunning ? '加速运行中，点击关闭' : '加速已停止，点击开启'"
            :class="[
              'h-20 w-20 rounded-full flex items-center justify-center transition-all duration-200 cursor-pointer',
              isToggling ? 'opacity-60 cursor-not-allowed' : '',
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

        <!-- 用量统计：极简合并单图（左上角请求/累计，右上角定宽高精实时速率） -->
        <section class="w-[350px] shrink-0 ml-auto space-y-2">
          <div class="flex items-center justify-between gap-1 whitespace-nowrap">
            <!-- 左上角：今日请求与累计指标 -->
            <div class="flex items-center gap-2.5 shrink-0">
              <span class="inline-flex items-center gap-1 text-xs">
                <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">今日请求</span>
                <span class="text-foreground font-medium tabular-nums font-mono">{{ todayTotals.requests }}</span>
              </span>
              <span class="inline-flex items-center gap-1 text-xs">
                <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">累计</span>
                <span class="text-foreground font-medium tabular-nums font-mono">{{ formatBytes(totalBytes) }}</span>
              </span>
            </div>

            <!-- 右上角：实时网速指标（自适应小数、定宽防抖、单位固定槽位，绝对不换行） -->
            <div class="flex items-center shrink-0">
              <span class="inline-flex items-center gap-1 text-xs">
                <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground shrink-0">实时</span>
                <span class="inline-flex items-center font-mono text-xs tabular-nums text-foreground">
                  <span class="text-muted-foreground">↓</span>
                  <span class="inline-block w-7 text-right font-medium">{{ speedDownParts.val }}</span>
                  <span class="inline-block w-7 text-left text-[10px] text-muted-foreground pl-0.5">{{ speedDownParts.unit }}</span>
                  <span class="text-muted-foreground ml-1">↑</span>
                  <span class="inline-block w-7 text-right font-medium">{{ speedUpParts.val }}</span>
                  <span class="inline-block w-7 text-left text-[10px] text-muted-foreground pl-0.5">{{ speedUpParts.unit }}</span>
                </span>
              </span>
            </div>
          </div>

          <svg
            :viewBox="`0 0 ${mergedChart.W} ${mergedChart.H}`"
            class="w-full h-auto select-none overflow-visible"
            role="img"
            :aria-label="usageDimension === '7d' ? '近 7 日用量统计与实时网速' : '近 24 小时用量统计与实时网速'"
          >
            <!-- 实时网速波形（底部对齐日期上方基准线，左右边距全宽贴合） -->
            <path
              :d="speedChart.area"
              class="fill-primary/10 transition-all duration-300"
            />
            <polyline
              :points="speedChart.points"
              fill="none"
              class="stroke-primary/40"
              stroke-width="1.25"
              stroke-linejoin="round"
              stroke-linecap="round"
            />

            <!-- 经典用量单柱（CF 橙黄色，紧凑居中） -->
            <rect
              v-for="(b, i) in mergedChart.bars"
              :key="'b' + i"
              :x="b.x"
              :y="b.y"
              :width="b.w"
              :height="b.h"
              rx="2"
              fill="#f6821f"
              class="opacity-90 hover:opacity-100 transition-opacity cursor-pointer"
            >
              <title>{{ b.title }}</title>
            </rect>

            <!-- X 轴极简日期/时间标签，位于基准线下方 -->
            <text
              v-for="(d, i) in mergedChart.days"
              :key="'d' + i"
              :x="d.x"
              :y="mergedChart.H - 2"
              text-anchor="middle"
              :class="[
                'text-[9px] tabular-nums transition-colors',
                d.isToday ? 'fill-foreground font-semibold' : 'fill-muted-foreground/60',
              ]"
            >
              {{ d.label }}
            </text>
          </svg>

          <!-- 底部图例栏：用量维度切换（7天 / 24h）与实时网速图例 -->
          <div class="flex items-center justify-between gap-3 text-[11px] text-muted-foreground pt-0.5">
            <!-- 7天 / 24h 维度 switch -->
            <div
              class="shrink-0 inline-flex items-center rounded-full bg-muted p-0.5 text-xs"
              role="group"
              aria-label="用量统计维度"
            >
              <button
                type="button"
                @click="usageDimension = '7d'"
                :aria-pressed="usageDimension === '7d'"
                :class="[
                  'px-2 py-0.5 rounded-full text-[10px] leading-none transition-all duration-150 cursor-pointer',
                  usageDimension === '7d'
                    ? 'bg-card text-foreground font-medium shadow-sm'
                    : 'text-muted-foreground hover:text-foreground',
                ]"
              >
                7天
              </button>
              <button
                type="button"
                @click="usageDimension = '24h'"
                :aria-pressed="usageDimension === '24h'"
                :class="[
                  'px-2 py-0.5 rounded-full text-[10px] leading-none transition-all duration-150 cursor-pointer',
                  usageDimension === '24h'
                    ? 'bg-card text-foreground font-medium shadow-sm'
                    : 'text-muted-foreground hover:text-foreground',
                ]"
              >
                24h
              </button>
            </div>

            <!-- 图例指标 -->
            <div class="flex items-center gap-3.5">
              <span class="inline-flex items-center gap-1.5">
                <span class="h-2.5 w-2.5 rounded-xs bg-[#f6821f]"></span>{{ usageDimension === '7d' ? '近 7 日用量' : '近 24 小时用量' }}
              </span>
              <span class="inline-flex items-center gap-1.5">
                <span class="inline-block h-0.5 w-3.5 rounded-full bg-primary/70"></span>实时网速
              </span>
            </div>
          </div>

          <!-- 用户专属配额指示（若接入了多租户令牌） -->
          <div v-if="userQuotaDisplay" class="pt-3.5 space-y-1.5 text-xs">
            <div class="flex items-center justify-between text-[11px] text-muted-foreground">
              <span class="flex items-center gap-1">
                <span>用户配额 ({{ userQuotaDisplay.name }})</span>
                <span v-if="userQuotaDisplay.isExpired" class="text-rose-500 font-medium">已过期</span>
              </span>
              <span class="font-mono tabular-nums">
                {{ formatBytes(userQuotaDisplay.usedBytes) }} / {{ formatBytes(userQuotaDisplay.quotaBytes) }}
                <span :class="['ml-1', Number(userQuotaDisplay.percent) > 90 ? 'text-rose-500 font-medium' : Number(userQuotaDisplay.percent) > 75 ? 'text-amber-500 font-medium' : 'text-foreground font-medium']">
                  ({{ userQuotaDisplay.percent }}%)
                </span>
              </span>
            </div>
            <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
              <div
                class="h-full rounded-full transition-all duration-500"
                :class="[Number(userQuotaDisplay.percent) > 90 ? 'bg-rose-500' : Number(userQuotaDisplay.percent) > 75 ? 'bg-amber-500' : 'bg-primary']"
                :style="{ width: `${Math.max(1, Math.min(100, Number(userQuotaDisplay.percent)))}%` }"
              ></div>
            </div>
            <div class="flex items-center justify-between text-[10px] text-muted-foreground/75 pt-0.5">
              <span>到期时间：{{ userQuotaDisplay.expStr }}</span>
              <span>最大并发：{{ userQuotaDisplay.maxConns }}</span>
            </div>
          </div>

          <!-- VPS 月度用量指示（无用户令牌时回退展示默认 VPS 500GB） -->
          <div v-else class="pt-3.5 space-y-1.5 text-xs">
            <div class="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>VPS月度用量</span>
              <span class="font-mono tabular-nums">
                {{ formatBytes(vpsUsage.monthBytes) }} / 500GB
                <span :class="['ml-1', Number(vpsUsage.percent) > 80 ? 'text-amber-500 font-medium' : 'text-foreground font-medium']">
                  ({{ vpsUsage.percent }}%)
                </span>
              </span>
            </div>
            <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
              <div
                class="h-full rounded-full bg-primary transition-all duration-500"
                :style="{ width: `${Math.max(1, Number(vpsUsage.percent))}%` }"
              ></div>
            </div>
          </div>
        </section>
      </div>

      <!-- 连接状态：普通用户仅显示 1 个“代理状态”，管理员显示全局 3 出口 -->
      <section class="space-y-1">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-2">
            <h3 class="text-sm font-semibold">连接状态</h3>
            <span v-if="isAdmin" class="rounded bg-primary/10 border border-primary/20 px-1.5 py-0.5 text-[10px] font-medium text-primary">
              管理员全局视图
            </span>
          </div>
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
          <!-- 普通用户视图：仅展示 1 个“代理状态” -->
          <div
            v-if="!isAdmin"
            class="flex items-center justify-between gap-3 py-2.5 col-span-1"
          >
            <div class="flex items-center min-w-0">
              <div class="text-sm font-medium leading-none truncate flex items-center gap-1.5">
                <span>{{ userProxyRow.name }}</span>
                <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] leading-none text-muted-foreground">端到端链路</span>
              </div>
            </div>
            <div class="flex items-center gap-3 shrink-0">
              <LatencyBars :history="userProxyRow.history" />
              <span class="w-14 shrink-0 text-right text-xs font-mono tabular-nums" :class="latestClass(userProxyRow.history)">
                {{ latestText(userProxyRow.history) }}
              </span>
              <button
                @click="testUserProxyRow"
                :disabled="userProxyRow.testing"
                title="立即测速"
                :aria-label="`立即测速 ${userProxyRow.name}`"
                class="h-7 w-7 shrink-0 inline-flex items-center justify-center rounded-full text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-50"
              >
                <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': userProxyRow.testing }" />
              </button>
            </div>
          </div>

          <!-- 管理员视图：完整展示 3 个物理出口状态（出口R、出口C、出口V） -->
          <template v-else>
            <div
              v-for="row in ifaceRows"
              :key="row.id"
              class="flex items-center justify-between gap-3 py-2.5"
            >
              <div class="flex items-center min-w-0">
                <div class="text-sm font-medium leading-none truncate">{{ row.name }}</div>
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
          </template>

          <!-- 常用站点行（全角色展示：Google, Anthropic, GitHub, OpenAI, X） -->
          <div
            v-for="row in siteRows"
            :key="row.host"
            class="flex items-center justify-between gap-3 py-2.5"
          >
            <div class="flex flex-col min-w-0">
              <div class="flex items-center gap-1.5 min-w-0">
                <span class="text-sm font-medium leading-none truncate">{{ row.name }}</span>
                <span class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] leading-none text-muted-foreground">
                  经本地引擎
                </span>
              </div>
              <div class="text-xs text-muted-foreground font-mono mt-1 truncate">{{ row.host }}</div>
            </div>
            <div class="flex items-center gap-3 shrink-0">
              <LatencyBars :history="row.history" />
              <span class="w-14 shrink-0 text-right text-xs font-mono tabular-nums" :class="latestClass(row.history)">
                {{ latestText(row.history) }}
              </span>
              <button
                @click="testSiteRow(row, true)"
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
          <span>每 10 分钟自动测速，柱条仅保留近 2 小时 (≤2s 绿 / 2-5s 黄 / >5s 红)</span>
          <div class="flex items-center gap-3 text-[11px]">
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-xs bg-emerald-500"></span>&le;2s 正常
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-xs bg-amber-500"></span>2-5s 稍慢
            </span>
            <span class="inline-flex items-center gap-1.5">
              <span class="h-2 w-2 rounded-xs bg-rose-500"></span>&gt;5s 延迟
            </span>
          </div>
        </div>
      </section>

      <!-- 管理员专享：多节点分布式集群状态监控大盘 -->
      <section v-if="isAdmin" class="space-y-3 pt-4 border-t border-border/30">
        <div class="flex items-center justify-between">
          <div>
            <h3 class="text-sm font-semibold flex items-center gap-2">
              <span>分布式集群节点状态</span>
              <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] font-mono text-muted-foreground">
                {{ clusterNodes.length }} 节点已注册
              </span>
            </h3>
            <p class="text-xs text-muted-foreground mt-0.5">跨机房热备状态，故障时 0 延时自动漂移中继</p>
          </div>
          <button
            @click="refreshClusterNodes"
            :disabled="isRefreshingCluster"
            title="刷新集群节点"
            aria-label="刷新集群节点"
            class="h-8 w-8 inline-flex items-center justify-center rounded-full text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-50"
          >
            <RefreshCw class="h-4 w-4" :class="{ 'animate-spin': isRefreshingCluster }" />
          </button>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
          <div
            v-for="node in clusterNodes"
            :key="node.address"
            class="rounded-xl border border-border/40 bg-card p-3.5 space-y-2 shadow-xs"
          >
            <div class="flex items-center justify-between">
              <span class="text-xs font-medium text-foreground truncate">{{ node.name }}</span>
              <span
                :class="[
                  'inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium',
                  node.online ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400' : 'bg-rose-500/10 text-rose-600 dark:text-rose-400',
                ]"
              >
                <span class="h-1.5 w-1.5 rounded-full" :class="node.online ? 'bg-emerald-500' : 'bg-rose-500'"></span>
                {{ node.online ? '在线 (Online)' : '离线 (Offline)' }}
              </span>
            </div>
            <div class="flex items-center justify-between text-xs font-mono text-muted-foreground pt-1">
              <span class="text-[11px] truncate">{{ node.address }}</span>
              <span v-if="node.online && typeof node.latency_ms === 'number'" class="text-emerald-600 font-medium">
                {{ node.latency_ms }}ms
              </span>
              <span v-else class="text-muted-foreground/60">—</span>
            </div>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>
