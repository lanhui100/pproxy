<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  Activity,
  Check,
  Copy,
  Globe,
  Loader2,
  LoaderCircle,
  Plus,
  RefreshCw,
  Sparkles,
  Zap,
} from '@lucide/vue'

import { api, type RouteDto } from '@/api/client'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useSessionSecret } from '@/composables/useSessionSecret'
import { copySecret } from '@/composables/useSecretCopy'
import { useToast } from '@/composables/useToast'
import { provisionTunnel } from '@/composables/useTunnelProvision'
import { loadBackendUrl, loadDataPlaneUrl } from '@/lib/config'
import { errText } from '@/lib/errors'
import { generatePresetSnippets, type PresetSnippet } from '@/lib/presetGenerator'
import { SERVICE_TEMPLATES } from '@/lib/serviceTemplates'
import { upstreamLabel } from '@/lib/statusLabels'
import { cleanDomainInput, deriveDataPlane, parseServiceUrlInput } from '@/lib/urls'

const toast = useToast()
const { getSessionSecret } = useSessionSecret()

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
    toast.success(`已添加 ${v}`, '加速名单已实时热生效')
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

// 客户端成品配置弹窗
const showConfigDialog = ref(false)
const configRouteName = ref('')
const configCustomToken = ref('')
const configCustomUpstreamKey = ref('')
const activePresetTab = ref<string>('openai')
const syncToWhitelist = ref(true)

const configSnippets = computed<PresetSnippet[]>(() => {
  const dataPlaneBase = loadDataPlaneUrl() || deriveDataPlane(loadBackendUrl()) || 'http://127.0.0.1:8899'
  const token = configCustomToken.value.trim() || getSessionSecret() || undefined
  const upstreamKey = configCustomUpstreamKey.value.trim() || undefined

  return generatePresetSnippets({
    dataPlaneBase,
    service: configRouteName.value,
    token,
    upstreamKey,
  })
})

const activeSnippet = computed(() =>
  configSnippets.value.find((s) => s.id === activePresetTab.value) || configSnippets.value[0],
)

const activeDisplayCode = computed(() => {
  if (!activeSnippet.value) return ''
  return (
    activeSnippet.value.psSnippet ||
    activeSnippet.value.bashSnippet ||
    activeSnippet.value.codeSnippet ||
    activeSnippet.value.baseUrl
  )
})

// 编辑与删除
const deleteTarget = ref<RouteDto | null>(null)
const deleting = ref(false)
const testingRouteName = ref<string | null>(null)

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

async function submitSmartAccess(): Promise<void> {
  if (!parsedUrl.value) {
    toast.error('请输入有效的 API 地址或域名')
    return
  }
  addingRoute.value = true
  const { inferredName, cleanHost } = parsedUrl.value

  try {
    await api.createRoute({
      name: inferredName,
      target_host: cleanHost,
    })

    if (syncToWhitelist.value) {
      await addWhitelistEntry(cleanHost)
    }

    inputUrl.value = ''
    configRouteName.value = inferredName
    configCustomToken.value = getSessionSecret() || ''
    configCustomUpstreamKey.value = ''
    showConfigDialog.value = true
    await refreshRoutes()
    toast.success(`服务「${inferredName}」已接入就绪`)
  } catch (e) {
    toast.error(errText(e))
  } finally {
    addingRoute.value = false
  }
}

function selectPresetTemplate(tpl: (typeof SERVICE_TEMPLATES)[number]): void {
  inputUrl.value = `https://${tpl.target_host}`
}

function openRouteConfig(r: RouteDto): void {
  configRouteName.value = r.name
  configCustomToken.value = getSessionSecret() || ''
  configCustomUpstreamKey.value = ''
  showConfigDialog.value = true
}

async function copyActivePresetSnippet(): Promise<void> {
  if (!activeDisplayCode.value) return
  await copySecret(activeDisplayCode.value, {
    onCopied: () => toast.success(`已复制 ${activeSnippet.value?.name || ''} 配置`),
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

async function testSingleRoute(r: RouteDto): Promise<void> {
  testingRouteName.value = r.name
  try {
    const res = await api.testRoute(r.name)
    if (res.ok) {
      toast.success(`${r.name} 连接正常`, `延迟: ${res.latency_ms}ms`)
    } else {
      toast.error(`${r.name} 连接异常`, res.error ? errText(res.error) : '未能成功连通目标服务器')
    }
  } catch (e) {
    toast.error('测速失败', errText(e))
  } finally {
    testingRouteName.value = null
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

onMounted(() => {
  void refreshProxy()
  void refreshRoutes()
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="代理与服务"
      subtitle="Windows 系统全局透明加速与大模型 API 专线网关"
    />

    <!-- ==================== 1. 本机透明代理总控 ==================== -->
    <Card class="border-border shadow-xs">
      <CardHeader class="pb-3">
        <div class="flex items-center justify-between">
          <div class="space-y-1">
            <CardTitle class="text-sm font-semibold flex items-center gap-2">
              <Globe class="size-4 text-primary" />
              Windows 系统代理加速
            </CardTitle>
            <CardDescription class="text-xs">
              开启后名单内的网站自动走加速通道，免配置浏览器与终端。
            </CardDescription>
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
      </CardHeader>

      <CardContent class="space-y-3.5 pt-1">
        <p v-if="proxyError" class="rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad break-all">
          {{ proxyError }}
        </p>

        <!-- 连通性体检（美观微网格卡片） -->
        <div class="rounded-lg border border-border/50 bg-muted/20 p-3 space-y-2.5">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
              <Activity class="size-3.5" />
              <span>网络连通性体检</span>
              <InfoTip text="通过本机代理探测常用全球站点的连接畅通度与延迟" />
            </div>
            <Button
              variant="outline"
              size="xs"
              class="h-7 px-2.5 text-xs"
              :disabled="!proxyEnabled || testingSites"
              @click="testSites"
            >
              <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': testingSites }" />
              {{ testingSites ? '测速中…' : '测速体检' }}
            </Button>
          </div>

          <!-- 网格卡片展示：2 列 / 4 列响应式微卡 -->
          <div v-if="siteResults.length" class="grid grid-cols-2 sm:grid-cols-4 gap-2 pt-0.5">
            <div
              v-for="r in siteResults"
              :key="r.site"
              class="flex items-center justify-between p-2 rounded-md border border-border/40 bg-background text-xs"
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

        <!-- 加速域名名单（默认折叠，需要时展开） -->
        <details class="group rounded-lg border border-border/50 bg-muted/20 p-3">
          <summary class="flex cursor-pointer items-center justify-between font-medium text-xs text-muted-foreground select-none">
            <div class="flex items-center gap-1.5">
              <span>加速域名名单</span>
              <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground font-mono">
                {{ whitelistEntries.length }} 个
              </span>
              <InfoTip text="添加主域名（如 google.com）将自动加速其全部子域名 (*.google.com)，修改即时热生效。" />
            </div>
            <span class="text-[11px] text-primary group-open:rotate-180 transition-transform duration-200">
              ▾
            </span>
          </summary>

          <div class="mt-3 space-y-2.5 pt-1">
            <div class="flex gap-2">
              <Input
                v-model="newWhitelistEntry"
                placeholder="输入需要加速的域名或网址，如 google.com"
                class="h-8 text-xs bg-background flex-1"
                @keyup.enter="addWhitelistEntry()"
              />
              <Button
                size="xs"
                class="h-8 px-3 text-xs shrink-0"
                :disabled="!newWhitelistEntry.trim()"
                @click="addWhitelistEntry()"
              >
                <Plus class="size-3 mr-1" />
                添加
              </Button>
            </div>

            <div v-if="whitelistEntries.length" class="flex flex-wrap gap-1.5">
              <span
                v-for="(e, i) in whitelistEntries"
                :key="e"
                class="inline-flex items-center gap-1 rounded-full bg-muted px-2.5 py-0.5 text-xs text-foreground/80 hover:bg-muted/80"
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
      </CardContent>
    </Card>

    <!-- ==================== 2. 客户端 API 专线接入 ==================== -->
    <div class="space-y-4">
      <div>
        <h2 class="text-sm font-semibold tracking-tight flex items-center gap-2">
          <Zap class="size-4 text-primary" />
          大模型与 API 专线接入
        </h2>
        <p class="text-xs text-muted-foreground mt-0.5">
          输入任意 API 地址或选择预设，自动生成兼容 Claude Code、Cursor、OpenAI SDK 的成品配置。
        </p>
      </div>

      <!-- 快速预设徽章 -->
      <div class="flex flex-wrap gap-1.5">
        <button
          v-for="tpl in SERVICE_TEMPLATES.slice(0, 6)"
          :key="tpl.name"
          type="button"
          class="inline-flex items-center gap-1 rounded-md bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition hover:bg-accent hover:text-foreground cursor-pointer"
          @click="selectPresetTemplate(tpl)"
        >
          <Sparkles class="size-3 text-primary/70" />
          {{ tpl.label }}
        </button>
      </div>

      <!-- 智能 URL 接入器 -->
      <Card class="border-primary/20 bg-primary/[0.02]">
        <CardContent class="p-4 space-y-3">
          <div class="flex gap-2">
            <Input
              v-model="inputUrl"
              placeholder="粘贴任意 API 地址，如 https://api.openai.com/v1 或 api.groq.com"
              class="h-9 text-xs bg-background font-mono flex-1"
              @keyup.enter="submitSmartAccess"
            />
            <Button
              size="sm"
              class="h-9 px-4 text-xs font-medium shrink-0"
              :disabled="!inputUrl.trim() || addingRoute"
              @click="submitSmartAccess"
            >
              <LoaderCircle v-if="addingRoute" class="size-3 animate-spin mr-1" />
              一键接入
            </Button>
          </div>

          <!-- 智能解析预览 -->
          <div v-if="parsedUrl" class="flex items-center justify-between text-xs text-muted-foreground bg-background/80 rounded-md px-3 py-1.5 border border-border/50">
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
        </CardContent>
      </Card>

      <!-- 已配置服务列表 -->
      <Card>
        <CardHeader class="pb-3">
          <div class="flex items-center justify-between">
            <CardTitle class="text-sm font-semibold">已接入专线列表</CardTitle>
            <Button variant="ghost" size="xs" :disabled="routesLoading" @click="refreshRoutes">
              <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': routesLoading }" />
              刷新
            </Button>
          </div>
        </CardHeader>
        <CardContent class="p-0">
          <SkeletonTable v-if="routesLoading && routes.length === 0" :rows="3" />
          <EmptyState
            v-else-if="routes.length === 0"
            title="还没有添加任何专线服务"
            description="在上方粘贴 API 地址或点击常用大模型预设，一键创建加速专线。"
          />
          <Table v-else>
            <TableHeader>
              <TableRow>
                <TableHead class="text-xs">服务标识</TableHead>
                <TableHead class="text-xs">目标地址</TableHead>
                <TableHead class="text-xs">中转线路</TableHead>
                <TableHead class="text-xs">状态</TableHead>
                <TableHead class="text-right text-xs">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="r in routes" :key="r.name">
                <TableCell class="font-medium text-xs font-mono">{{ r.name }}</TableCell>
                <TableCell class="text-xs font-mono text-muted-foreground">{{ r.target_host }}</TableCell>
                <TableCell>
                  <Badge variant="outline" class="text-[10px]">
                    {{ upstreamLabel(r.upstream ?? '').label }}
                  </Badge>
                </TableCell>
                <TableCell>
                  <Switch
                    :model-value="r.enabled"
                    size="sm"
                    @update:model-value="(val: boolean) => toggleRouteEnabled(r, val)"
                  />
                </TableCell>
                <TableCell class="text-right space-x-1">
                  <Button
                    variant="outline"
                    size="xs"
                    :disabled="testingRouteName === r.name"
                    @click="testSingleRoute(r)"
                  >
                    <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': testingRouteName === r.name }" />
                    测速
                  </Button>
                  <Button variant="default" size="xs" @click="openRouteConfig(r)">
                    客户端配置
                  </Button>
                  <Button
                    variant="ghost"
                    size="xs"
                    class="text-bad hover:bg-bad-soft"
                    @click="deleteTarget = r"
                  >
                    删除
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>

    <!-- ==================== 3. 客户端成品配置弹窗 ==================== -->
    <Dialog :open="showConfigDialog" @update:open="(v) => (showConfigDialog = v)">
      <DialogContent class="sm:max-w-2xl max-h-[85vh] flex flex-col">
        <DialogHeader>
          <DialogTitle class="flex items-center gap-2 text-base">
            <Sparkles class="size-4 text-primary" />
            「{{ configRouteName }}」客户端接入指南
          </DialogTitle>
        </DialogHeader>

        <div class="space-y-4 overflow-y-auto pr-1 py-1">
          <!-- 密钥与 Base URL 实时覆盖 -->
          <div class="rounded-lg border border-border/60 bg-muted/30 p-3 space-y-3">
            <div class="text-xs font-medium text-foreground">快速替换代码占位符：</div>
            <div class="grid gap-3 sm:grid-cols-2">
              <div class="space-y-1">
                <Label for="cfg-token-override" class="text-[11px]">本机接入密钥 (Token)</Label>
                <Input
                  id="cfg-token-override"
                  v-model="configCustomToken"
                  type="password"
                  placeholder="已自动载入本次会话密钥"
                  class="h-8 text-xs font-mono"
                />
              </div>
              <div class="space-y-1">
                <Label for="cfg-upstream-key-override" class="text-[11px]">上游 API Key（如 OpenAI / Anthropic Key）</Label>
                <Input
                  id="cfg-upstream-key-override"
                  v-model="configCustomUpstreamKey"
                  type="password"
                  placeholder="留空则生成占位符"
                  class="h-8 text-xs font-mono"
                />
              </div>
            </div>
            <p v-if="!configCustomToken" class="text-[11px] text-warn">
              ⚠️ 未检测到已缓存的设备密钥，可在下方配置中手动将“&lt;YOUR_TOKEN&gt;”替换为你在「设置」页创建的密钥。
            </p>
          </div>

          <!-- 场景切换 Tab 药丸 -->
          <div class="flex flex-wrap gap-1 border-b border-border/40 pb-2">
            <button
              v-for="snip in configSnippets"
              :key="snip.id"
              type="button"
              class="rounded-md px-2.5 py-1 text-xs font-medium transition"
              :class="
                activePresetTab === snip.id
                  ? 'bg-primary text-primary-foreground shadow-xs'
                  : 'text-muted-foreground hover:bg-muted hover:text-foreground'
              "
              @click="activePresetTab = snip.id"
            >
              {{ snip.name }}
            </button>
          </div>

          <!-- 代码展示区 -->
          <div v-if="activeSnippet" class="space-y-2">
            <div class="flex items-center justify-between text-xs text-muted-foreground">
              <span>{{ activeSnippet.title }}</span>
              <Button size="xs" variant="outline" @click="copyActivePresetSnippet">
                <Copy class="size-3 mr-1" />
                一键复制
              </Button>
            </div>
            <p v-if="activeSnippet.notes" class="text-[11px] text-muted-foreground">{{ activeSnippet.notes }}</p>
            <pre class="overflow-x-auto rounded-lg bg-zinc-950 p-3.5 text-xs text-zinc-100 font-mono leading-relaxed select-all"><code>{{ activeDisplayCode }}</code></pre>
          </div>
        </div>

        <DialogFooter class="border-t border-border/40 pt-3">
          <Button variant="outline" size="sm" @click="showConfigDialog = false">关闭</Button>
          <Button size="sm" @click="copyActivePresetSnippet">
            <Check class="size-3.5 mr-1" />
            复制并完成
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

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
