<script setup lang="ts">
import { onMounted, ref } from 'vue'
import {
  Activity,
  ChevronDown,
  Globe,
  Loader2,
  Plus,
  RefreshCw,
  ShieldCheck,
  Zap,
} from '@lucide/vue'

import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { useToast } from '@/composables/useToast'
import { provisionTunnel } from '@/composables/useTunnelProvision'
import { cleanDomainInput } from '@/lib/urls'

const toast = useToast()

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
const proxyMode = ref<'whitelist' | 'global'>('whitelist')
const togglingProxy = ref(false)
const switchingMode = ref(false)
const proxyError = ref('')

interface SiteResult { site: string; ok: boolean; ms: number; error: string }
const siteResults = ref<SiteResult[]>([])
const testingSites = ref(false)

function formatSiteName(site: string): string {
  const map: Record<string, string> = {
    'google.com': 'Google',
    'www.google.com': 'Google',
    'x.com': 'X (Twitter)',
    'openai.com': 'OpenAI',
    'anthropic.com': 'Anthropic',
    'github.com': 'GitHub',
  }
  return map[site.toLowerCase()] || site
}

async function refreshProxy(): Promise<void> {
  try {
    if (!isTauri()) return
    const [wl, st, mode] = await Promise.all([
      tauri<string[]>('proxy_whitelist_get'),
      tauri<{ engine_running: boolean; mode?: 'whitelist' | 'global' }>('proxy_status'),
      tauri<string>('proxy_mode_get').catch(() => 'whitelist'),
    ])
    whitelistEntries.value = wl
    proxyEnabled.value = st.engine_running
    proxyMode.value = (mode as 'whitelist' | 'global') || st.mode || 'whitelist'
    proxyError.value = ''
  } catch (e) {
    proxyError.value = String(e)
  }
}

async function setProxyMode(mode: 'whitelist' | 'global'): Promise<void> {
  if (proxyMode.value === mode || switchingMode.value) return
  switchingMode.value = true
  try {
    if (isTauri()) {
      await tauri('proxy_mode_set', { mode })
    }
    proxyMode.value = mode
    toast.success(
      mode === 'global' ? '已切换至「全局模式」' : '已切换至「白名单模式」',
      mode === 'global'
        ? '除局域网外所有流量均走加速通道'
        : '内置海外常用站点并加速自定义域名名单',
    )
  } catch (e) {
    toast.error('切换模式失败', String(e))
  } finally {
    switchingMode.value = false
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
        proxyError.value = '隧道未配置，请先在「设置 → 隧道中继」填写端点与令牌'
        return
      }
    }
    await tauri(on ? 'proxy_enable' : 'proxy_disable')
    const st = await tauri<{ engine_running: boolean; mode?: 'whitelist' | 'global' }>('proxy_status')
    proxyEnabled.value = st.engine_running
    if (st.mode) proxyMode.value = st.mode
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

onMounted(async () => {
  await refreshProxy()

  if (isTauri()) {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen<{ mode: 'whitelist' | 'global' }>('proxy-mode-changed', (event) => {
        if (event.payload?.mode) proxyMode.value = event.payload.mode
      })
      await listen<{ on: boolean; mode?: 'whitelist' | 'global' }>('proxy-status-changed', (event) => {
        if (typeof event.payload?.on === 'boolean') proxyEnabled.value = event.payload.on
        if (event.payload?.mode) proxyMode.value = event.payload.mode
      })
    } catch {
      /* 忽略非 Tauri 报错 */
    }
  }
})

</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="代理与服务"
      subtitle="Windows 系统全局透明加速"
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
            开启后常用海外网站及名单内域名自动走高速通道，免配置浏览器与终端。
          </p>
        </div>
        <div class="flex items-center gap-3">
          <span
            class="inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-xs font-medium transition-colors"
            :class="proxyEnabled ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/25' : 'bg-muted/80 text-muted-foreground border border-border/40'"
          >
            <span class="size-1.5 rounded-full" :class="proxyEnabled ? 'bg-emerald-500 animate-pulse' : 'bg-muted-foreground/50'" />
            {{ proxyEnabled ? '系统已接管' : '未开启' }}
          </span>
          <div class="flex items-center gap-1.5">
            <Switch
              :model-value="proxyEnabled"
              :disabled="togglingProxy"
              @update:model-value="toggleProxy"
            />
            <Loader2 v-if="togglingProxy" class="size-4 animate-spin text-muted-foreground" />
          </div>
        </div>
      </div>

      <p v-if="proxyError" class="rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad break-all">
        {{ proxyError }}
      </p>

      <!-- 模式切换与总控（彩色与柔和协调 Tab） -->
      <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3 pt-0.5">
        <div class="inline-flex rounded-lg bg-muted/60 p-1 text-xs font-medium self-start border border-border/40 gap-1">
          <button
            type="button"
            class="rounded-md px-3 py-1.5 transition cursor-pointer flex items-center gap-1.5 border border-transparent"
            :class="proxyMode === 'whitelist'
              ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 border-emerald-500/30 shadow-xs font-semibold'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted/40'"
            :disabled="switchingMode"
            @click="setProxyMode('whitelist')"
          >
            <ShieldCheck class="size-3.5 text-emerald-600 dark:text-emerald-400" />
            <span>白名单模式</span>
            <span class="text-[10px] opacity-80 font-normal">（智能分流）</span>
          </button>
          <button
            type="button"
            class="rounded-md px-3 py-1.5 transition cursor-pointer flex items-center gap-1.5 border border-transparent"
            :class="proxyMode === 'global'
              ? 'bg-indigo-500/15 text-indigo-700 dark:text-indigo-300 border-indigo-500/30 shadow-xs font-semibold'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted/40'"
            :disabled="switchingMode"
            @click="setProxyMode('global')"
          >
            <Zap class="size-3.5 text-indigo-600 dark:text-indigo-400" />
            <span>全局模式</span>
            <span class="text-[10px] opacity-80 font-normal">（全量流量）</span>
          </button>
        </div>

        <p class="text-[11px] text-muted-foreground sm:text-right">
          <template v-if="proxyMode === 'whitelist'">
            海外常用站点走加速通道，直连服务与国内流量极速直出。
          </template>
          <template v-else>
            除局域网外，所有公网 HTTP/HTTPS 流量均全量经由隧道转发。
          </template>
        </p>
      </div>

      <!-- 网页访问检测（5 站点微卡片矩阵） -->
      <div class="rounded-lg bg-muted/40 p-3 space-y-2.5">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <Activity class="size-3.5 text-primary" />
            <span class="text-foreground font-semibold">网页访问检测</span>
            <InfoTip text="通过本机代理探测 Google、X/Twitter、OpenAI、Anthropic、GitHub 等主流官网的实际连接畅通度与延迟" />
          </div>
          <Button
            variant="outline"
            size="xs"
            class="h-7 px-2.5 text-xs rounded-md"
            :disabled="!proxyEnabled || testingSites"
            @click="testSites"
          >
            <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': testingSites }" />
            {{ testingSites ? '检测中…' : '开始检测' }}
          </Button>
        </div>

        <div v-if="siteResults.length" class="grid grid-cols-2 sm:grid-cols-5 gap-2 pt-0.5">
          <div
            v-for="r in siteResults"
            :key="r.site"
            class="flex flex-col justify-between p-2.5 rounded-lg bg-background text-xs border border-border/50 shadow-xs gap-1.5"
          >
            <div class="flex items-center justify-between min-w-0">
              <span class="font-medium text-foreground truncate text-xs">{{ formatSiteName(r.site) }}</span>
              <StatusDot :tone="r.ok ? 'ok' : 'error'" size="sm" />
            </div>
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground text-[10px] truncate max-w-[65px]" :title="r.site">{{ r.site }}</span>
              <span v-if="r.ok" class="font-mono font-medium text-emerald-600 dark:text-emerald-400 tabular-nums">
                {{ r.ms }}ms
              </span>
              <span v-else class="text-bad font-medium truncate max-w-[65px]" :title="r.error">
                {{ r.error || '失败' }}
              </span>
            </div>
          </div>
        </div>
        <p v-else class="text-xs text-muted-foreground py-0.5">
          {{ proxyEnabled ? '点击右上角「开始检测」测试 Google、X、OpenAI、Anthropic、GitHub 等官网连通性' : '开启系统代理后可进行网页访问连通性检测' }}
        </p>
      </div>

      <!-- 加速域名名单（折叠，仅白名单模式下重点配置） -->
      <details :open="proxyMode === 'whitelist'" class="group rounded-lg bg-muted/40 p-3">
        <summary class="flex cursor-pointer items-center justify-between font-medium text-xs text-muted-foreground select-none list-none">
          <div class="flex items-center gap-1.5">
            <span>自定义加速域名名单</span>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground font-mono">
              {{ whitelistEntries.length }} 个自定义
            </span>
            <InfoTip text="添加主域名（如 google.com）将自动覆盖全部子域名 (*.google.com) 与地区域名 (google.com.hk 等)；常用海外网站已默认内置，无需手动重复录入。" />
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
  </div>
</template>
