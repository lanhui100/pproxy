<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  Check,
  Copy,
  Globe,
  Info,
  Loader2,
  LoaderCircle,
  Plus,
  RefreshCw,
  Sparkles,
  Terminal,
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
    token,
    service: configRouteName.value,
    upstreamKey,
  })
})

const isTokenMissing = computed(() => {
  const token = configCustomToken.value.trim() || getSessionSecret()
  return !token
})

async function refreshRoutes(): Promise<void> {
  routesLoading.value = true
  routesError.value = ''
  try {
    routes.value = (await api.listRoutes()).routes
  } catch (e) {
    routesError.value = errText(e)
  } finally {
    routesLoading.value = false
  }
}

function selectPresetTemplate(tpl: { name: string; target_host: string }): void {
  inputUrl.value = `https://${tpl.target_host}`
}

async function submitSmartAccess(): Promise<void> {
  if (!parsedUrl.value) {
    toast.error('请输入有效的服务地址或域名')
    return
  }
  addingRoute.value = true
  const { cleanHost, inferredName, extractedKey, suggestedPreset } = parsedUrl.value

  try {
    const pureHost = cleanHost.split(':')[0] || cleanHost
    // 1. 若路由不存在，则自动创建
    const exists = routes.value.some((r) => r.name === inferredName)
    if (!exists) {
      await api.createRoute({
        name: inferredName,
        target_host: pureHost,
      })
      await refreshRoutes()
    }

    // 2. 智能联动加入加速名单
    if (syncToWhitelist.value && pureHost) {
      void addWhitelistEntry(pureHost)
    }

    // 3. 初始化弹窗上下文
    configRouteName.value = inferredName
    configCustomToken.value = getSessionSecret() || ''
    configCustomUpstreamKey.value = extractedKey || ''
    activePresetTab.value = suggestedPreset === 'claude' ? 'claude' : suggestedPreset === 'cursor' ? 'cursor' : 'openai'
    showConfigDialog.value = true
    inputUrl.value = ''
    toast.success(`服务「${inferredName}」已就绪`, '已生成专属客户端配置')
  } catch (e) {
    toast.error('接入失败', errText(e))
  } finally {
    addingRoute.value = false
  }
}

// 复制配置助手
async function copySnippet(text?: string): Promise<void> {
  if (!text) return
  try {
    await copySecret(text, {
      onCopied: () => toast.success('已复制到剪贴板（60 秒自动清理保护）'),
    })
  } catch {
    toast.error('复制失败，请手动选择文本复制')
  }
}

// 行内开关与测速
const togglingRoute = ref('')
async function toggleRoute(r: RouteDto): Promise<void> {
  togglingRoute.value = r.name
  try {
    const resp = await api.patchRoute(r.name, { enabled: !r.enabled })
    r.enabled = resp.enabled
  } catch (e) {
    toast.error(errText(e))
  } finally {
    togglingRoute.value = ''
  }
}

const testingRoute = ref('')
const routeLatencies = ref<Record<string, { ok: boolean; ms?: number; err?: string }>>({})

async function testSingleRoute(r: RouteDto): Promise<void> {
  testingRoute.value = r.name
  try {
    const res = await api.testRoute(r.name, { skipAuthRedirect: true })
    routeLatencies.value[r.name] = {
      ok: res.ok,
      ms: res.latency_ms ?? undefined,
      err: res.error ? errText(res.error) : undefined,
    }
  } catch (e) {
    routeLatencies.value[r.name] = { ok: false, err: errText(e) }
  } finally {
    testingRoute.value = ''
  }
}

// 删除确认
const deleteTarget = ref<RouteDto | null>(null)
const deleting = ref(false)

async function doDeleteRoute(): Promise<void> {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    await api.deleteRoute(deleteTarget.value.name)
    deleteTarget.value = null
    await refreshRoutes()
    toast.success('服务已移除')
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
  <div class="space-y-8 max-w-3xl">
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

      <CardContent class="space-y-4 pt-2">
        <p v-if="proxyError" class="rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad break-all">
          {{ proxyError }}
        </p>

        <!-- 连通性体检 -->
        <div class="flex items-center justify-between rounded-lg bg-muted/40 p-2.5">
          <div class="flex items-center gap-2 text-xs">
            <span class="font-medium text-muted-foreground">网络状态:</span>
            <template v-if="siteResults.length">
              <span
                v-for="r in siteResults"
                :key="r.site"
                class="inline-flex items-center gap-1 text-[11px] rounded bg-background px-1.5 py-0.5"
              >
                <StatusDot :tone="r.ok ? 'ok' : 'error'" :label="r.ok ? '正常' : '异常'" class="text-xs" />
                <span>{{ r.site.replace(/.(com|org|net|ai|io|cn|top)$/i, '') }}</span>
                <span v-if="r.ok" class="text-muted-foreground">{{ r.ms }}ms</span>
              </span>
            </template>
            <span v-else class="text-muted-foreground">{{ proxyEnabled ? '等待体检' : '代理未开启' }}</span>
          </div>
          <Button
            variant="outline"
            size="xs"
            :disabled="!proxyEnabled || testingSites"
            @click="testSites"
          >
            <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': testingSites }" />
            {{ testingSites ? '测速中…' : '测速体检' }}
          </Button>
        </div>

        <!-- 加速域名白名单 -->
        <div class="space-y-2">
          <div class="flex items-center justify-between">
            <label class="text-xs font-medium text-muted-foreground flex items-center gap-1">
              加速域名名单
              <InfoTip text="添加主域名（如 google.com）将自动加速其全部子域名 (*.google.com)，修改即时热生效。" />
            </label>
            <span class="text-[11px] text-muted-foreground">{{ whitelistEntries.length }} 个域名</span>
          </div>

          <div class="flex gap-2">
            <Input
              v-model="newWhitelistEntry"
              placeholder="输入需要加速的域名或网址，如 google.com"
              class="h-8 text-xs bg-muted/50"
              @keyup.enter="addWhitelistEntry()"
            />
            <Button size="xs" :disabled="!newWhitelistEntry.trim()" @click="addWhitelistEntry()">
              <Plus class="size-3 mr-1" />
              添加
            </Button>
          </div>

          <div v-if="whitelistEntries.length" class="flex flex-wrap gap-1.5 pt-1">
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
        </div>
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
              class="h-9 text-xs bg-background font-mono"
              @keyup.enter="submitSmartAccess"
            />
            <Button
              size="sm"
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
              同步加入加速名单
            </label>
          </div>
        </CardContent>
      </Card>

      <!-- 已配置服务列表 -->
      <div class="space-y-2 pt-2">
        <div class="flex items-center justify-between">
          <h3 class="text-xs font-semibold text-muted-foreground">已接入服务（{{ routes.length }}）</h3>
          <Button variant="ghost" size="xs" :disabled="routesLoading" @click="refreshRoutes">
            <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': routesLoading }" />
            刷新列表
          </Button>
        </div>

        <SkeletonTable v-if="routesLoading && routes.length === 0" :rows="3" />
        <EmptyState
          v-else-if="routes.length === 0"
          title="还没有接入任何服务"
          description="在上方粘贴 API 地址或点击常用模板，一键建立专线接入。"
        />
        <Card v-else class="overflow-hidden py-1">
          <Table>
            <TableHeader>
              <TableRow class="text-xs">
                <TableHead class="pl-4">服务名</TableHead>
                <TableHead>目标地址</TableHead>
                <TableHead>中转线路</TableHead>
                <TableHead>启用</TableHead>
                <TableHead>连通性</TableHead>
                <TableHead class="pr-4 text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="r in routes" :key="r.name" class="text-xs border-b border-border/40">
                <TableCell class="pl-4 font-medium">{{ r.name }}</TableCell>
                <TableCell class="font-mono text-[11px] text-muted-foreground">{{ r.target_host }}</TableCell>
                <TableCell>
                  <Badge variant="secondary" class="text-[10px]">{{ upstreamLabel(r.effective_upstream).label }}</Badge>
                </TableCell>
                <TableCell>
                  <Switch
                    :model-value="r.enabled"
                    :disabled="togglingRoute === r.name"
                    @update:model-value="toggleRoute(r)"
                  />
                </TableCell>
                <TableCell>
                  <template v-if="testingRoute === r.name">
                    <span class="text-muted-foreground animate-pulse">测速中…</span>
                  </template>
                  <template v-else-if="routeLatencies[r.name]">
                    <StatusDot
                      v-if="routeLatencies[r.name]?.ok"
                      tone="ok"
                      :label="`正常 · ${routeLatencies[r.name]?.ms ?? '?'}ms`"
                    />
                    <StatusDot
                      v-else
                      tone="error"
                      :label="`失败: ${routeLatencies[r.name]?.err || '超时'}`"
                    />
                  </template>
                  <span v-else class="text-muted-foreground">未测</span>
                </TableCell>
                <TableCell class="pr-4 text-right space-x-1">
                  <Button
                    variant="outline"
                    size="xs"
                    :disabled="testingRoute === r.name"
                    @click="testSingleRoute(r)"
                  >
                    测速
                  </Button>
                  <Button
                    variant="ghost"
                    size="xs"
                    class="text-bad hover:bg-bad-soft hover:text-bad"
                    @click="deleteTarget = r"
                  >
                    删除
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </Card>
      </div>
    </div>

    <!-- ==================== 3. 客户端成品配置弹窗 ==================== -->
    <Dialog :open="showConfigDialog" @update:open="(v) => (showConfigDialog = v)">
      <DialogContent class="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle class="flex items-center gap-2">
            <Terminal class="size-4 text-primary" />
            「{{ configRouteName }}」专线配置已生成
          </DialogTitle>
        </DialogHeader>

        <div class="space-y-4 pt-1">
          <!-- Token 缺失警告与实时补充 -->
          <div v-if="isTokenMissing" class="rounded-lg bg-warn-soft p-3 text-xs text-warn space-y-2">
            <div class="flex items-start gap-2">
              <Info class="size-4 shrink-0 mt-0.5" />
              <span>当前未暂存设备密钥，上方代码使用了占位符 <code>&lt;YOUR_TOKEN&gt;</code>。你可直接在下方填入设备密钥实时替换代码。</span>
            </div>
            <div class="flex gap-2 pt-1">
              <Input
                v-model="configCustomToken"
                placeholder="在此填入你的设备密钥（如 pony_live_xxx）"
                class="h-7 text-xs bg-background text-foreground"
              />
            </div>
          </div>

          <!-- 预设 Tab 切换 -->
          <div class="flex gap-1 rounded-lg bg-muted/60 p-1 text-xs overflow-x-auto">
            <button
              v-for="s in configSnippets"
              :key="s.id"
              type="button"
              class="flex-1 min-w-20 rounded-md py-1.5 font-medium transition-colors cursor-pointer text-center"
              :class="
                activePresetTab === s.id
                  ? 'bg-card text-foreground shadow-xs'
                  : 'text-muted-foreground hover:text-foreground'
              "
              @click="activePresetTab = s.id"
            >
              {{ s.name }}
            </button>
          </div>

          <!-- 当前 Tab 内容展示 -->
          <template v-for="s in configSnippets" :key="s.id">
            <div v-if="activePresetTab === s.id" class="space-y-3">
              <!-- Base URL 框 -->
              <div class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-muted-foreground">专属 Base URL</span>
                  <Button variant="ghost" size="xs" @click="copySnippet(s.baseUrl)">
                    <Copy class="size-3 mr-1" /> 复制地址
                  </Button>
                </div>
                <code class="block break-all rounded-md bg-muted p-2.5 font-mono text-xs text-foreground select-all">
                  {{ s.baseUrl }}
                </code>
              </div>

              <!-- PowerShell 配置（Windows 优先） -->
              <div v-if="s.psSnippet" class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-muted-foreground">PowerShell 环境变量 (Windows 终端)</span>
                  <Button variant="ghost" size="xs" @click="copySnippet(s.psSnippet)">
                    <Copy class="size-3 mr-1" /> 一键复制
                  </Button>
                </div>
                <pre class="overflow-x-auto rounded-md bg-muted p-2.5 font-mono text-xs text-foreground select-all">{{ s.psSnippet }}</pre>
              </div>

              <!-- Bash 配置 -->
              <div v-if="s.bashSnippet" class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-muted-foreground">Bash / Linux / macOS 环境变量</span>
                  <Button variant="ghost" size="xs" @click="copySnippet(s.bashSnippet)">
                    <Copy class="size-3 mr-1" /> 一键复制
                  </Button>
                </div>
                <pre class="overflow-x-auto rounded-md bg-muted p-2.5 font-mono text-xs text-foreground select-all">{{ s.bashSnippet }}</pre>
              </div>

              <!-- 代码片段 -->
              <div v-if="s.codeSnippet" class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-muted-foreground">代码调用示例</span>
                  <Button variant="ghost" size="xs" @click="copySnippet(s.codeSnippet)">
                    <Copy class="size-3 mr-1" /> 复制代码
                  </Button>
                </div>
                <pre class="overflow-x-auto rounded-md bg-muted p-2.5 font-mono text-xs text-foreground select-all">{{ s.codeSnippet }}</pre>
              </div>

              <p v-if="s.notes" class="text-xs text-ok bg-ok-soft px-3 py-1.5 rounded-md">
                💡 {{ s.notes }}
              </p>
            </div>
          </template>

          <!-- 选填上游 API Key 替换 -->
          <div class="pt-1 border-t border-border/50">
            <details class="text-xs text-muted-foreground">
              <summary class="cursor-pointer font-medium hover:text-foreground">可选：填入上游 API Key 自动拼入代码</summary>
              <div class="pt-2 space-y-1">
                <Label for="cfg-upstream-key" class="text-[11px]">上游 API Key</Label>
                <Input
                  id="cfg-upstream-key"
                  v-model="configCustomUpstreamKey"
                  placeholder="如 sk-ant-api03-xxx 或 sk-proj-xxx"
                  class="h-8 text-xs font-mono"
                />
              </div>
            </details>
          </div>
        </div>

        <DialogFooter>
          <Button @click="showConfigDialog = false">
            <Check class="size-3 mr-1" />
            完成
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 删除确认 -->
    <ConfirmDialog
      :open="deleteTarget !== null"
      :title="`删除服务「${deleteTarget?.name ?? ''}」？`"
      :description="`目标 ${deleteTarget?.target_host ?? ''} 将从网关移除，使用中的客户端将无法继续访问。`"
      confirm-text="确认删除"
      destructive
      :busy="deleting"
      @update:open="(v) => !v && (deleteTarget = null)"
      @confirm="doDeleteRoute"
    />
  </div>
</template>
