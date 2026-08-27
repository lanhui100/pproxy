<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  Activity,
  ChevronDown,
  Copy,
  Globe,
  Loader2,
  LoaderCircle,
  Plus,
  RefreshCw,
  Trash2,
  Zap,
} from '@lucide/vue'

import { api, type RouteDto } from '@/api/client'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import LatencyBars from '@/components/common/LatencyBars.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { useAdaptivePoll } from '@/composables/useAdaptivePoll'
import { useSessionSecret } from '@/composables/useSessionSecret'
import { copySecret } from '@/composables/useSecretCopy'
import { useToast } from '@/composables/useToast'
import { provisionTunnel } from '@/composables/useTunnelProvision'
import { loadBackendUrl, loadDataPlaneUrl } from '@/lib/config'
import { errText } from '@/lib/errors'
import { appendLatencyPoint, type LatencyPoint } from '@/lib/latencyHistory'
import { upstreamLabel } from '@/lib/statusLabels'
import { cleanDomainInput, deriveDataPlane, parseServiceUrlInput } from '@/lib/urls'

const toast = useToast()
const { getSessionSecret, setSessionSecret } = useSessionSecret()

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}
async function tauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error('浏览器预览模式下不可用，请在桌面应用内使用')
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

// ==================== 1. 本机系统透明代理 ====================
const whitelistEntries = ref<string[]>([])
const newWhitelistEntry = ref('')
const proxyEnabled = ref(false)
const togglingProxy = ref(false)
const proxyError = ref('')

interface SiteResult { site: string; ok: boolean; ms: number; error: string }
const siteResults = ref<SiteResult[]>([])
const testingSites = ref(false)

async function refreshProxy(): Promise<void> {
  try {
    if (!isTauri()) return
    const [wl, st] = await Promise.all([
      tauri<string[]>('proxy_whitelist_get'),
      tauri<{ engine_running: boolean }>('proxy_status'),
    ])
    whitelistEntries.value = wl
    proxyEnabled.value = st.engine_running
    proxyError.value = ''
  } catch (e) {
    proxyError.value = String(e)
  }
}

async function addWhitelistEntry(domainToAdd?: string): Promise<void> {
  const raw = domainToAdd || newWhitelistEntry.value
  const v = cleanDomainInput(raw)
  if (!v) {
    toast.error('域名格式不正确', '请输入合法的域名（如 google.com）或网址')
    return
  }
  if (whitelistEntries.value.includes(v)) {
    if (!domainToAdd) toast.error('域名已在加速名单中', `${v} 已存在`)
    return
  }
  const next = [...whitelistEntries.value, v]
  try {
    if (isTauri()) await tauri('proxy_whitelist_set', { entries: next })
    whitelistEntries.value = next
    if (!domainToAdd) newWhitelistEntry.value = ''
    toast.success(`已添加 ${v}`, '加速名单已实时生效')
  } catch (e) {
    proxyError.value = String(e)
  }
}

async function removeWhitelistEntry(i: number): Promise<void> {
  const target = whitelistEntries.value[i]
  const next = whitelistEntries.value.filter((_, idx) => idx !== i)
  try {
    if (isTauri()) await tauri('proxy_whitelist_set', { entries: next })
    whitelistEntries.value = next
    if (target) toast.success(`已移除 ${target}`)
  } catch (e) {
    proxyError.value = String(e)
  }
}

async function toggleProxy(on: boolean): Promise<void> {
  proxyError.value = ''
  togglingProxy.value = true
  try {
    if (on) {
      const r = await provisionTunnel()
      if (r === 'unavailable') {
        proxyError.value = '网关未提供隧道配置，请先在「设置 → 隧道中继」填写'
        return
      }
    }
    await tauri(on ? 'proxy_enable' : 'proxy_disable')
    const st = await tauri<{ engine_running: boolean }>('proxy_status')
    proxyEnabled.value = st.engine_running
    if (on && proxyEnabled.value) toast.success('Windows 系统代理已开启')
  } catch (e) {
    const msg = String(e).replace(/^"|"$/g, '')
    proxyError.value = msg
    proxyEnabled.value = false
    toast.error('代理切换失败', msg)
  } finally {
    togglingProxy.value = false
  }
}

async function testSites(): Promise<void> {
  testingSites.value = true
  try {
    siteResults.value = await tauri<SiteResult[]>('proxy_test_sites')
    const fails = siteResults.value.filter((r) => !r.ok)
    if (fails.length > 0) {
      toast.error(`${fails.length} 个站点连接异常`)
    } else if (siteResults.value.length > 0) {
      toast.success('网络连接畅通')
    }
  } catch (e) {
    toast.error('体检失败', String(e))
  } finally {
    testingSites.value = false
  }
}

// ==================== 2. 服务专线网关与智能接入 ====================
const routes = ref<RouteDto[]>([])
const routesLoading = ref(false)
const routesError = ref('')

const inputUrl = ref('')
const parsedUrl = computed(() => parseServiceUrlInput(inputUrl.value))
const addingRoute = ref(false)
const syncToWhitelist = ref(true)

// 最近接入成功生成的专线信息
interface CreatedAccessResult {
  service: string
  token: string
  publicUrl: string
  lanUrl: string
}
const lastCreated = ref<CreatedAccessResult | null>(null)

// 专线测速时序历史（30 分钟轮询采样）
const STORAGE_KEY = 'pony_route_latency_history_v1'
const routeHistories = ref<Record<string, LatencyPoint[]>>({})
const probingRoute = ref<string>('')
const switchingRoute = ref<string>('')

// 编辑与删除
const deleteTarget = ref<RouteDto | null>(null)
const deleting = ref(false)

function loadHistories(): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw)
      routeHistories.value = parsed.routes || {}
    }
  } catch {
    /* 忽略异常 */
  }
}

function saveHistories(): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    const prev = raw ? JSON.parse(raw) : {}
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        ...prev,
        routes: routeHistories.value,
      }),
    )
  } catch {
    /* 忽略异常 */
  }
}

function getBaseUrls(): { publicBase: string; lanBase: string } {
  const configuredDataPlane = loadDataPlaneUrl()
  const backend = loadBackendUrl() || 'http://127.0.0.1:8900'
  const derivedDataPlane = deriveDataPlane(backend) || 'http://127.0.0.1:8899'

  const publicBase = configuredDataPlane || derivedDataPlane
  let lanBase = 'http://127.0.0.1:8899'

  try {
    const u = new URL(derivedDataPlane)
    lanBase = `http://${u.hostname}:8899`
  } catch {
    /* 默认 127.0.0.1 */
  }
  return { publicBase, lanBase }
}

function buildServiceUrls(service: string, token: string, subPath = ''): { publicUrl: string; lanUrl: string } {
  const { publicBase, lanBase } = getBaseUrls()
  const tok = token.trim() || 'default_token'
  const cleanPath = subPath ? (subPath.startsWith('/') ? subPath : `/${subPath}`) : '/v1'
  return {
    publicUrl: `${publicBase.replace(/\/$/, '')}/${tok}/${service}${cleanPath}`,
    lanUrl: `${lanBase.replace(/\/$/, '')}/${tok}/${service}${cleanPath}`,
  }
}

function getRouteUpstreamKey(r: RouteDto): string {
  return r.override_upstream || r.effective_upstream || r.upstream || 'worker'
}

async function refreshRoutes(): Promise<void> {
  routesLoading.value = true
  routesError.value = ''
  try {
    const res = await api.listRoutes()
    routes.value = res.routes
  } catch (e) {
    routesError.value = errText(e)
  } finally {
    routesLoading.value = false
  }
}

// 自动探活全部路由（30 分钟轮询一次）
async function probeAllRoutes(): Promise<void> {
  if (routes.value.length === 0) return
  const now = Date.now()

  for (const r of routes.value) {
    if (!r.enabled) continue
    try {
      const res = await api.testRoute(r.name, { skipAuthRedirect: true })
      routeHistories.value[r.name] = appendLatencyPoint(routeHistories.value[r.name], {
        ts: now,
        ok: res.ok,
        ms: res.latency_ms ?? undefined,
        err: res.error ? errText(res.error) : undefined,
      })
    } catch (e) {
      routeHistories.value[r.name] = appendLatencyPoint(routeHistories.value[r.name], {
        ts: now,
        ok: false,
        err: errText(e),
      })
    }
  }
  saveHistories()
}

// 自适应 30 分钟轮询（1,800,000 ms）
const { start: start30MinPoll } = useAdaptivePoll(
  async () => {
    await refreshRoutes()
    await probeAllRoutes()
  },
  {
    baseIntervalMs: 1_800_000,
    maxIntervalMs: 3_600_000,
    immediate: false,
  },
)

async function testSingleRoute(r: { name: string }): Promise<void> {
  probingRoute.value = r.name
  const now = Date.now()
  try {
    const res = await api.testRoute(r.name)
    const pt: LatencyPoint = {
      ts: now,
      ok: res.ok,
      ms: res.latency_ms ?? undefined,
      err: res.error ? errText(res.error) : undefined,
    }
    routeHistories.value[r.name] = appendLatencyPoint(routeHistories.value[r.name], pt)
    if (res.ok) {
      toast.success(`${r.name} 连接正常`, `延迟: ${res.latency_ms}ms`)
    } else {
      toast.error(`${r.name} 测速异常`, res.error ? errText(res.error) : '连接失败')
    }
  } catch (e) {
    const pt: LatencyPoint = {
      ts: now,
      ok: false,
      err: errText(e),
    }
    routeHistories.value[r.name] = appendLatencyPoint(routeHistories.value[r.name], pt)
    toast.error('测速失败', errText(e))
  } finally {
    probingRoute.value = ''
    saveHistories()
  }
}

async function submitSmartAccess(): Promise<void> {
  if (!parsedUrl.value) {
    toast.error('请输入有效的 API 地址或域名')
    return
  }
  addingRoute.value = true
  const { inferredName, cleanHost, subPath } = parsedUrl.value

  try {
    // 1. 自动为该服务创建专属 Token（名称符合 ^[a-zA-Z0-9._-]{1,64}$ 规范）
    const tokenName = `route_${inferredName.replace(/[^a-zA-Z0-9._-]/g, '_')}`.slice(0, 60)
    let tokenStr = getSessionSecret() || ''
    try {
      const tokRes = await api.createToken({
        name: tokenName,
      })
      tokenStr = tokRes.token
      setSessionSecret(tokRes.token, tokRes.name)
    } catch {
      /* 若已存在同名密钥则沿用当前会话密钥 */
    }

    // 2. 创建服务路由
    await api.createRoute({
      name: inferredName,
      target_host: cleanHost,
    })

    // 3. 同步加入加速名单
    if (syncToWhitelist.value) {
      await addWhitelistEntry(cleanHost)
    }

    // 4. 生成成品专线 URL（含已拼入的 Token 与完整子路径）
    const urls = buildServiceUrls(inferredName, tokenStr, subPath)
    lastCreated.value = {
      service: inferredName,
      token: tokenStr,
      publicUrl: urls.publicUrl,
      lanUrl: urls.lanUrl,
    }

    inputUrl.value = ''
    await refreshRoutes()
    void testSingleRoute({ name: inferredName })
    toast.success(`专线「${inferredName}」已接入就绪`)
  } catch (e) {
    toast.error('创建专线失败', errText(e))
  } finally {
    addingRoute.value = false
  }
}

// 手动切换线路（CF Worker <-> Vercel）
async function switchRouteUpstream(r: RouteDto, nextUpstream: 'worker' | 'vercel'): Promise<void> {
  switchingRoute.value = r.name
  try {
    await api.patchRoute(r.name, { override_upstream: nextUpstream })
    r.override_upstream = nextUpstream
    await refreshRoutes()
    toast.success(`「${r.name}」已切换至 ${upstreamLabel(nextUpstream).label}`)
    void testSingleRoute({ name: r.name })
  } catch (e) {
    toast.error('切换线路失败', errText(e))
  } finally {
    switchingRoute.value = ''
  }
}

async function copyUrl(url: string, label: string): Promise<void> {
  await copySecret(url, {
    onCopied: () => toast.success(`已复制 ${label}`, '已包含专属访问令牌，可直接粘贴进客户端使用'),
  })
}

async function toggleRouteEnabled(r: RouteDto, enabled: boolean): Promise<void> {
  try {
    await api.patchRoute(r.name, { enabled })
    r.enabled = enabled
    toast.success(`服务「${r.name}」已${enabled ? '启用' : '停用'}`)
  } catch (e) {
    toast.error(errText(e))
  }
}

async function doDeleteRoute(): Promise<void> {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    await api.deleteRoute(deleteTarget.value.name)
    toast.success(`已删除服务「${deleteTarget.value.name}」`)
    deleteTarget.value = null
    await refreshRoutes()
  } catch (e) {
    toast.error(errText(e))
  } finally {
    deleting.value = false
  }
}

onMounted(async () => {
  loadHistories()
  await refreshProxy()
  await refreshRoutes()
  start30MinPoll()
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="代理与服务"
      subtitle="Windows 系统全局透明加速与大模型 API 专线网关"
    />

    <!-- ==================== 1. 本机透明代理总控（无边框 微圆角背景色） ==================== -->
    <div class="rounded-xl bg-card p-4 space-y-4 shadow-xs">
      <div class="flex items-center justify-between">
        <div class="space-y-1">
          <h2 class="text-sm font-semibold flex items-center gap-2">
            <Globe class="size-4 text-primary" />
            Windows 系统代理加速
          </h2>
          <p class="text-xs text-muted-foreground">
            开启后名单内的网站自动走加速通道，免配置浏览器与终端。
          </p>
        </div>
        <div class="flex items-center gap-2">
          <Switch
            :model-value="proxyEnabled"
            :disabled="togglingProxy"
            @update:model-value="toggleProxy"
          />
          <span v-if="togglingProxy"><Loader2 class="size-4 animate-spin text-muted-foreground" /></span>
          <span v-else class="text-xs font-medium">{{ proxyEnabled ? '已接管' : '未开启' }}</span>
        </div>
      </div>

      <p v-if="proxyError" class="rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad break-all">
        {{ proxyError }}
      </p>

      <!-- 连通性体检（微网格卡片矩阵） -->
      <div class="rounded-lg bg-muted/40 p-3 space-y-2.5">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <Activity class="size-3.5" />
            <span>网络连通性体检</span>
            <InfoTip text="通过本机代理探测常用全球站点的连接畅通度与延迟" />
          </div>
          <Button
            variant="outline"
            size="xs"
            class="h-7 px-2.5 text-xs rounded-md"
            :disabled="!proxyEnabled || testingSites"
            @click="testSites"
          >
            <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': testingSites }" />
            {{ testingSites ? '测速中…' : '测速体检' }}
          </Button>
        </div>

        <div v-if="siteResults.length" class="grid grid-cols-2 sm:grid-cols-4 gap-2 pt-0.5">
          <div
            v-for="r in siteResults"
            :key="r.site"
            class="flex items-center justify-between p-2 rounded-md bg-background text-xs shadow-xs"
          >
            <div class="flex items-center gap-1.5 min-w-0">
              <StatusDot :tone="r.ok ? 'ok' : 'error'" size="md" />
              <span class="font-medium truncate">{{ r.site.replace(/\.(com|org|net|ai|io|cn|top)$/i, '') }}</span>
            </div>
            <span v-if="r.ok" class="font-mono text-[11px] font-medium text-foreground tabular-nums">
              {{ r.ms }}ms
            </span>
            <span v-else class="text-[11px] text-bad font-medium truncate max-w-16" :title="r.error">
              {{ r.error || '超时' }}
            </span>
          </div>
        </div>
        <p v-else class="text-xs text-muted-foreground py-0.5">
          {{ proxyEnabled ? '点击右上角「测速体检」测试常用站点延迟' : '开启系统代理后可进行连通性测速体检' }}
        </p>
      </div>

      <!-- 加速域名名单（折叠，大号三角指示器） -->
      <details class="group rounded-lg bg-muted/40 p-3">
        <summary class="flex cursor-pointer items-center justify-between font-medium text-xs text-muted-foreground select-none list-none">
          <div class="flex items-center gap-1.5">
            <span>加速域名名单</span>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground font-mono">
              {{ whitelistEntries.length }} 个
            </span>
            <InfoTip text="添加主域名（如 google.com）将自动加速其全部子域名 (*.google.com)，修改即时热生效。" />
          </div>
          <ChevronDown class="size-4 text-muted-foreground transition-transform duration-200 group-open:rotate-180" />
        </summary>

        <div class="mt-3 space-y-2.5 pt-1">
          <!-- 内嵌添加按钮的输入框 -->
          <div class="relative flex items-center rounded-md bg-background shadow-xs">
            <input
              v-model="newWhitelistEntry"
              placeholder="输入需要加速的域名或网址，如 google.com"
              class="w-full bg-transparent pl-3 pr-20 py-1.5 text-xs outline-none"
              @keyup.enter="addWhitelistEntry()"
            />
            <Button
              size="xs"
              class="absolute right-1 h-6.5 px-2.5 text-xs rounded-sm shrink-0"
              :disabled="!newWhitelistEntry.trim()"
              @click="addWhitelistEntry()"
            >
              <Plus class="size-3 mr-1" />
              添加
            </Button>
          </div>

          <div v-if="whitelistEntries.length" class="flex flex-wrap gap-1.5 pt-1">
            <span
              v-for="(e, i) in whitelistEntries"
              :key="e"
              class="inline-flex items-center gap-1 rounded-full bg-background px-2.5 py-0.5 text-xs text-foreground/80 shadow-xs"
            >
              {{ e }}
              <button
                type="button"
                class="rounded-full text-muted-foreground hover:text-bad leading-none p-0.5 cursor-pointer"
                @click="removeWhitelistEntry(i)"
              >
                ×
              </button>
            </span>
          </div>
          <p v-else class="text-[11px] text-muted-foreground">名单为空，可在上方输入框添加</p>
        </div>
      </details>
    </div>

    <!-- ==================== 2. 大模型 API 专线接入（无边框 微圆角背景色） ==================== -->
    <div class="rounded-xl bg-card p-4 space-y-4 shadow-xs">
      <div>
        <h2 class="text-sm font-semibold tracking-tight flex items-center gap-2">
          <Zap class="size-4 text-primary" />
          大模型与 API 专线接入
        </h2>
        <p class="text-xs text-muted-foreground mt-0.5">
          粘贴目标 API 地址，自动生成已嵌入专属 Token 的公网与局域网接入 URL，即拷即用。
        </p>
      </div>

      <!-- 智能 URL 接入器：内嵌一键接入按钮 -->
      <div class="space-y-2">
        <div class="relative flex items-center rounded-lg bg-muted/50 p-1 shadow-xs">
          <input
            v-model="inputUrl"
            placeholder="粘贴任意 API 地址，如 https://api.openai.com/v1 或 opencode.ai/zen/v1"
            class="w-full bg-transparent pl-3 pr-24 py-2 text-xs font-mono outline-none"
            @keyup.enter="submitSmartAccess"
          />
          <Button
            size="sm"
            class="absolute right-1.5 h-7.5 px-3.5 text-xs font-medium rounded-md shrink-0"
            :disabled="!inputUrl.trim() || addingRoute"
            @click="submitSmartAccess"
          >
            <LoaderCircle v-if="addingRoute" class="size-3 animate-spin mr-1" />
            一键接入
          </Button>
        </div>

        <!-- 智能识别提示 -->
        <div v-if="parsedUrl" class="flex items-center justify-between text-xs text-muted-foreground bg-muted/30 rounded-md px-3 py-1.5">
          <span class="flex items-center gap-2">
            <span class="text-ok font-medium">✓ 智能识别:</span>
            <span>服务名 <strong>{{ parsedUrl.inferredName }}</strong></span>
            <span>目标 <code>{{ parsedUrl.cleanHost }}</code></span>
          </span>
          <label class="flex items-center gap-1.5 text-[11px] cursor-pointer">
            <input v-model="syncToWhitelist" type="checkbox" class="rounded text-primary" />
            同时加入 Windows 代理加速名单
          </label>
        </div>
      </div>

      <!-- 最近一次生成的专线直出卡片 -->
      <div v-if="lastCreated" class="rounded-lg bg-ok-soft/30 border border-ok/20 p-3.5 space-y-2.5">
        <div class="flex items-center justify-between">
          <div class="text-xs font-semibold text-ok flex items-center gap-1.5">
            <span>🎉 专线「{{ lastCreated.service }}」接入就绪</span>
            <span class="text-[11px] font-normal text-muted-foreground">（Token 已在设置页自动备案）</span>
          </div>
          <button type="button" class="text-xs text-muted-foreground hover:text-foreground cursor-pointer" @click="lastCreated = null">
            ✕
          </button>
        </div>

        <div class="grid gap-2 sm:grid-cols-2 text-xs">
          <!-- 公网接入地址 -->
          <div class="rounded-md bg-background p-2.5 space-y-1 shadow-xs">
            <div class="text-[11px] font-medium text-muted-foreground flex items-center justify-between">
              <span>公网 / 外部设备接入 URL</span>
              <Button size="xs" variant="ghost" class="h-5 px-1.5 text-[11px]" @click="copyUrl(lastCreated!.publicUrl, '公网接入地址')">
                <Copy class="size-3 mr-1" />
                复制
              </Button>
            </div>
            <code class="block text-[11px] font-mono break-all text-foreground select-all">{{ lastCreated.publicUrl }}</code>
          </div>

          <!-- 局域网接入地址 -->
          <div class="rounded-md bg-background p-2.5 space-y-1 shadow-xs">
            <div class="text-[11px] font-medium text-muted-foreground flex items-center justify-between">
              <span>局域网 / 本地接入 URL</span>
              <Button size="xs" variant="ghost" class="h-5 px-1.5 text-[11px]" @click="copyUrl(lastCreated!.lanUrl, '局域网接入地址')">
                <Copy class="size-3 mr-1" />
                复制
              </Button>
            </div>
            <code class="block text-[11px] font-mono break-all text-foreground select-all">{{ lastCreated.lanUrl }}</code>
          </div>
        </div>
      </div>

      <!-- 已接入专线列表（待用户添加后显示，带时序微网格测速） -->
      <div class="space-y-2 pt-2">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2">
            <h3 class="text-xs font-semibold text-foreground">已接入专线列表（{{ routes.length }}）</h3>
            <span class="text-[11px] text-muted-foreground font-normal">（30 分钟轮询测速）</span>
          </div>
          <Button variant="ghost" size="xs" :disabled="routesLoading" @click="refreshRoutes">
            <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': routesLoading }" />
            刷新列表
          </Button>
        </div>

        <SkeletonTable v-if="routesLoading && routes.length === 0" :rows="2" />
        
        <!-- 初始空状态 -->
        <EmptyState
          v-else-if="routes.length === 0"
          title="暂无 API 专线服务"
          description="在上方粘贴 API 地址一键接入，系统将自动生成专线接入地址与访问令牌。"
        />

        <!-- 专线卡片列表 -->
        <div v-else class="space-y-2">
          <div
            v-for="r in routes"
            :key="r.name"
            class="flex flex-col sm:flex-row sm:items-center justify-between gap-3 p-3 rounded-lg bg-muted/30 transition hover:bg-muted/50"
            :class="{ 'opacity-60': !r.enabled }"
          >
            <!-- 左侧：服务信息、线路切换器与快捷复制 -->
            <div class="space-y-1.5 min-w-0">
              <div class="flex items-center gap-2 flex-wrap">
                <span class="font-semibold text-xs font-mono">{{ r.name }}</span>
                <span class="text-[11px] font-mono text-muted-foreground">({{ r.target_host }})</span>
                
                <!-- 当前线路与手动切换器 (CF Worker <-> Vercel) -->
                <div class="inline-flex items-center gap-1 rounded bg-muted/80 p-0.5 text-[10px]">
                  <button
                    type="button"
                    class="rounded px-1.5 py-0.5 transition cursor-pointer font-medium"
                    :class="getRouteUpstreamKey(r) === 'worker' || getRouteUpstreamKey(r) === 'cf' ? 'bg-primary text-primary-foreground shadow-xs' : 'text-muted-foreground hover:text-foreground'"
                    :disabled="switchingRoute === r.name"
                    @click="switchRouteUpstream(r, 'worker')"
                  >
                    CF Worker
                  </button>
                  <button
                    type="button"
                    class="rounded px-1.5 py-0.5 transition cursor-pointer font-medium"
                    :class="getRouteUpstreamKey(r) === 'vercel' ? 'bg-primary text-primary-foreground shadow-xs' : 'text-muted-foreground hover:text-foreground'"
                    :disabled="switchingRoute === r.name"
                    @click="switchRouteUpstream(r, 'vercel')"
                  >
                    Vercel 出口
                  </button>
                </div>
              </div>

              <!-- 快捷复制专线 URL -->
              <div class="flex flex-wrap items-center gap-2 pt-0.5 text-[11px]">
                <button
                  type="button"
                  class="inline-flex items-center gap-1 text-primary hover:underline cursor-pointer font-mono"
                  @click="copyUrl(buildServiceUrls(r.name, getSessionSecret() || '').publicUrl, '公网专线地址')"
                >
                  <Copy class="size-2.5" />
                  复制公网 URL
                </button>
                <span class="text-muted-foreground/40">·</span>
                <button
                  type="button"
                  class="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground hover:underline cursor-pointer font-mono"
                  @click="copyUrl(buildServiceUrls(r.name, getSessionSecret() || '').lanUrl, '局域网专线地址')"
                >
                  <Copy class="size-2.5" />
                  复制局域网 URL
                </button>
              </div>
            </div>

            <!-- 右侧：时序微图、纯图标即时测速、启停开关与删除 -->
            <div class="flex items-center gap-3 shrink-0">
              <!-- 12 根时序微柱条 -->
              <div class="flex items-center gap-2">
                <LatencyBars :history="routeHistories[r.name]" />
                <span class="min-w-12 text-right font-mono text-xs tabular-nums font-medium text-foreground">
                  <template v-if="probingRoute === r.name">
                    <span class="text-muted-foreground animate-pulse text-[11px]">测速中…</span>
                  </template>
                  <template v-else-if="routeHistories[r.name]?.length">
                    <span :class="routeHistories[r.name]?.at(-1)?.ok ? 'text-foreground' : 'text-bad'">
                      {{ routeHistories[r.name]?.at(-1)?.ok ? `${routeHistories[r.name]?.at(-1)?.ms}ms` : '异常' }}
                    </span>
                  </template>
                  <template v-else>
                    <span class="text-muted-foreground text-[11px]">待测</span>
                  </template>
                </span>
              </div>

              <!-- 纯图标即时测速按钮 -->
              <button
                v-if="r.enabled"
                type="button"
                class="p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-background cursor-pointer transition"
                :disabled="probingRoute === r.name"
                title="立即测速"
                @click="testSingleRoute({ name: r.name })"
              >
                <RefreshCw class="size-3.5" :class="{ 'animate-spin': probingRoute === r.name }" />
              </button>

              <!-- 启停开关 -->
              <Switch
                :model-value="r.enabled"
                size="sm"
                @update:model-value="(val: boolean) => toggleRouteEnabled(r, val)"
              />

              <!-- 删除按钮 -->
              <button
                type="button"
                class="p-1.5 rounded-md text-muted-foreground hover:text-bad hover:bg-bad-soft cursor-pointer transition"
                title="删除专线"
                @click="deleteTarget = r"
              >
                <Trash2 class="size-3.5" />
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 删除服务确认 -->
    <ConfirmDialog
      :open="deleteTarget !== null"
      :title="`删除专线服务「${deleteTarget?.name ?? ''}」？`"
      description="删除后，通过该专线路由的请求将无法连接。此操作不可恢复。"
      confirm-text="确认删除"
      destructive
      :busy="deleting"
      @update:open="(v) => !v && (deleteTarget = null)"
      @confirm="doDeleteRoute"
    />
  </div>
</template>
