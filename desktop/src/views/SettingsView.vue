<script setup lang="ts">
import { onMounted, ref } from 'vue'
import {
  Check,
  Download,
  ExternalLink,
  LifeBuoy,
  Plus,
  RefreshCw,
  Server,
  Share2,
  ShieldCheck,
  Sparkles,
  Zap,
} from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Progress } from '@/components/ui/progress'
import { useToast } from '@/composables/useToast'
import {
  checkForUpdate,
  downloadAndInstall,
  downloadProgress,
  downloading,
  downloaded,
  checking,
  updateAvailable,
  updateVersion,
  updateError,
} from '@/composables/useUpdater'
import {
  clearTunnelToken,
  isTauri,
  isValidTunnelUrl,
  loadAutoProxyConfig,
  loadTunnelConfig,
  saveAutoProxyConfig,
  saveTunnelConfig,
} from '@/lib/config'
import { cleanDomainInput, openExternalUrl } from '@/lib/urls'

const toast = useToast()

// ---- 自定义加速域名名单（白名单）----
const whitelistEntries = ref<string[]>([])
const newWhitelistEntry = ref('')
const isAddingDomain = ref(false)

async function refreshWhitelist(): Promise<void> {
  if (!isTauri()) {
    if (whitelistEntries.value.length === 0) {
      whitelistEntries.value = ['openai.com', 'anthropic.com', 'github.com']
    }
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const wl = await invoke<string[]>('proxy_whitelist_get')
    whitelistEntries.value = wl
  } catch (e: any) {
    console.error('Failed to load whitelist:', e)
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
  isAddingDomain.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_whitelist_set', { entries: next })
    }
    whitelistEntries.value = next
    if (!domainToAdd) newWhitelistEntry.value = ''
    toast.success(`已添加 ${v}`, '加速名单已实时生效')
  } catch (e: any) {
    toast.error('添加失败', typeof e === 'string' ? e : e?.message || String(e))
  } finally {
    isAddingDomain.value = false
  }
}

async function removeWhitelistEntry(i: number): Promise<void> {
  const target = whitelistEntries.value[i]
  const next = whitelistEntries.value.filter((_, idx) => idx !== i)
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_whitelist_set', { entries: next })
    }
    whitelistEntries.value = next
    if (target) toast.success(`已移除 ${target}`)
  } catch (e: any) {
    toast.error('移除失败', typeof e === 'string' ? e : e?.message || String(e))
  }
}

// ---- 隧道中继（方案 A 出网通道：WS 端点 + 令牌）----
const tunnelUrlInput = ref('')
const tunnelTokenInput = ref('')
const tunnelHasToken = ref(false)
const tunnelSaving = ref(false)

async function refreshTunnel(): Promise<void> {
  try {
    const c = await loadTunnelConfig()
    tunnelUrlInput.value = c.url
    tunnelHasToken.value = c.hasToken
  } catch { /* 首次启动无配置，保持空表单 */ }
}

async function saveTunnel(): Promise<void> {
  if (tunnelSaving.value) return
  if (!isValidTunnelUrl(tunnelUrlInput.value)) {
    toast.error('隧道端点必须以 wss:// 或 ws:// 开头且不含空白')
    return
  }
  tunnelSaving.value = true
  try {
    await saveTunnelConfig(tunnelUrlInput.value.trim(), tunnelTokenInput.value)
    tunnelTokenInput.value = ''
    tunnelHasToken.value = true
    await refreshTunnel()
    toast.success('隧道配置已保存，重新开启代理后生效')
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message))
  } finally {
    tunnelSaving.value = false
  }
}

async function clearTunnelTokenAction(): Promise<void> {
  const cleared = await clearTunnelToken()
  if (cleared) {
    tunnelTokenInput.value = ''
    tunnelHasToken.value = false
    toast.success('已清除隧道令牌')
    return
  }
  toast.error('清除失败：请在系统凭据管理器中手动删除「pony-desktop / tunnel_token」后重试')
}

const currentMode = ref<'direct' | 'chained'>('direct')
const cfToken = ref('')
const remoteHost = ref('')
const remoteUser = ref('')
const remotePass = ref('')
const isSaving = ref(false)

// 跨端同步
const importSyncUri = ref('')

// 启动与系统偏好
const autoProxyEnabled = ref(true)
const autoProxySaving = ref(false)

async function refreshAutoProxy(): Promise<void> {
  try {
    const cfg = await loadAutoProxyConfig()
    autoProxyEnabled.value = cfg.auto_proxy !== false
  } catch {
    autoProxyEnabled.value = true
  }
}

async function handleAutoProxyToggle(val: boolean): Promise<void> {
  autoProxyEnabled.value = val
  autoProxySaving.value = true
  try {
    await saveAutoProxyConfig({ auto_proxy: val })
    toast.success(val ? '已开启启动自动代理' : '已关闭启动自动代理')
  } catch (e: any) {
    toast.error('保存设置失败', typeof e === 'string' ? e : e?.message)
  } finally {
    autoProxySaving.value = false
  }
}

onMounted(async () => {
  void refreshTunnel()
  void refreshWhitelist()
  void refreshAutoProxy()

  if (isTauri()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const cfg = (await invoke('proxy_get_current_config')) as any
      currentMode.value = cfg.mode_type === 'chained' ? 'chained' : 'direct'
      remoteHost.value = cfg.remote_host || ''
      remoteUser.value = cfg.username || ''

      const { listen } = await import('@tauri-apps/api/event')
      await listen('proxy-whitelist-updated', () => {
        void refreshWhitelist()
      })
    } catch {}
  }
})

async function saveModeConfig() {
  isSaving.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      if (currentMode.value === 'direct') {
        if (cfToken.value.trim()) {
          await invoke('proxy_mode_switch', {
            modeType: 'direct',
            config: {
              worker_url: 'https://edge.ponyjob.top',
              proxy_secret: cfToken.value.trim(),
            },
          })
        }
      } else {
        await invoke('proxy_mode_switch', {
          modeType: 'chained',
          config: {
            remote_host: remoteHost.value.trim(),
            username: remoteUser.value.trim(),
            password: remotePass.value.trim(),
          },
        })
      }
    }
    toast.success('配置已保存生效！')
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message))
  } finally {
    isSaving.value = false
  }
}

async function doImportSync() {
  if (!importSyncUri.value.trim()) {
    toast.error('请先粘贴同步口令')
    return
  }
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      const res = (await invoke('proxy_import_sync', {
        syncUri: importSyncUri.value.trim(),
      })) as any
      toast.success(res.message || '导入成功！')
      importSyncUri.value = ''
      // 切换模式至 chained 并刷新配置状态
      currentMode.value = 'chained'
      const cfg = (await invoke('proxy_get_current_config')) as any
      remoteHost.value = cfg.remote_host || ''
      remoteUser.value = cfg.username || ''
    } else {
      toast.success('口令导入成功！')
      importSyncUri.value = ''
      currentMode.value = 'chained'
    }
  } catch (e: any) {
    toast.error('导入失败: ' + (typeof e === 'string' ? e : e?.message))
  }
}

async function triggerRescue() {
  if (!isTauri()) {
    toast.success('网络已恢复直连！')
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const msg = (await invoke('proxy_rescue')) as string
    toast.success(msg || '网络急救成功，已恢复系统直连！')
  } catch (e: any) {
    toast.error('急救失败：' + (typeof e === 'string' ? e : e?.message))
  }
}
</script>

<template>
  <div class="h-full overflow-y-auto p-6 space-y-6 max-w-4xl mx-auto">
    <!-- 头部说明 -->
    <div class="pb-1">
      <h1 class="text-2xl font-bold tracking-tight text-foreground">设置中心</h1>
      <p class="text-sm text-muted-foreground mt-0.5">管理加速出网方案、域名分流规则与系统网络维护</p>
    </div>

    <!-- 加速出网方案 -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <div class="flex items-center justify-between">
          <CardTitle class="text-base flex items-center gap-2">
            <Zap class="h-4 w-4 text-primary" />
            加速出网方案
          </CardTitle>
          <span class="rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-medium text-primary">
            {{ currentMode === 'direct' ? '个人独立加速' : '远端代理连接' }}
          </span>
        </div>
        <CardDescription>选择适合您的加速出口通道，支持随时切换与多端同步</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <!-- 方案切换卡片（对齐欢迎页） -->
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <button
            type="button"
            @click="currentMode = 'direct'"
            :class="[
              'relative rounded-xl border p-4 text-left transition-all cursor-pointer',
              currentMode === 'direct'
                ? 'border-primary bg-primary/5 ring-1 ring-primary'
                : 'border-border bg-card hover:border-muted-foreground/40',
            ]"
          >
            <Check
              v-if="currentMode === 'direct'"
              class="absolute right-3 top-3 h-4 w-4 text-primary"
            />
            <div class="flex items-center gap-3">
              <div
                :class="[
                  'rounded-lg p-2',
                  currentMode === 'direct' ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground',
                ]"
              >
                <Sparkles class="h-4 w-4" />
              </div>
              <div>
                <div class="text-sm font-semibold flex items-center gap-1.5">
                  方案 A：个人独立加速
                  <span class="rounded bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">推荐</span>
                </div>
                <div class="text-xs text-muted-foreground mt-0.5">直连 Cloudflare / Vercel 双出口，专属通道极速无干扰</div>
              </div>
            </div>
          </button>

          <button
            type="button"
            @click="currentMode = 'chained'"
            :class="[
              'relative rounded-xl border p-4 text-left transition-all cursor-pointer',
              currentMode === 'chained'
                ? 'border-primary bg-primary/5 ring-1 ring-primary'
                : 'border-border bg-card hover:border-muted-foreground/40',
            ]"
          >
            <Check
              v-if="currentMode === 'chained'"
              class="absolute right-3 top-3 h-4 w-4 text-primary"
            />
            <div class="flex items-center gap-3">
              <div
                :class="[
                  'rounded-lg p-2',
                  currentMode === 'chained' ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground',
                ]"
              >
                <Server class="h-4 w-4" />
              </div>
              <div>
                <div class="text-sm font-semibold">方案 B：连接远端代理</div>
                <div class="text-xs text-muted-foreground mt-0.5">连接私有 Linux Server 或局域网其他代理服务</div>
              </div>
            </div>
          </button>
        </div>

        <!-- 方案 A：独立加速详细配置 -->
        <div v-if="currentMode === 'direct'" class="space-y-3.5 pt-1">
          <!-- 授权码配置 -->
          <div class="rounded-xl border border-border/80 bg-muted/20 p-3.5 space-y-3">
            <div class="space-y-1.5">
              <div class="flex items-center justify-between">
                <Label class="text-xs font-medium">更新加速授权码 (Cloudflare Token)</Label>
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
                placeholder="如需更新授权码请在此输入"
                class="font-mono text-xs"
              />
            </div>
            <p class="text-xs text-muted-foreground flex items-center gap-1.5">
              <ShieldCheck class="h-3.5 w-3.5 text-emerald-600 shrink-0" />
              凭据仅保存在本机系统凭据管理器，安全无泄漏
            </p>
          </div>

          <!-- 隧道中继高级配置（WS 端点 + 令牌） -->
          <div class="rounded-xl border border-border/80 bg-muted/20 p-3.5 space-y-3">
            <div class="flex items-center justify-between">
              <div class="text-xs font-semibold flex items-center gap-1.5">
                <Zap class="h-3.5 w-3.5 text-blue-600" />
                隧道中继端点与令牌 (出网通道)
              </div>
              <span v-if="tunnelHasToken" class="text-[11px] text-emerald-600 font-medium">
                本机已保存令牌
              </span>
              <span v-else class="text-[11px] text-muted-foreground">
                未配置令牌
              </span>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div class="space-y-1">
                <Label class="text-xs text-muted-foreground">隧道端点 (wss://)</Label>
                <Input v-model="tunnelUrlInput" placeholder="wss://gate.ponyjob.top/ws" class="font-mono text-xs" />
              </div>
              <div class="space-y-1">
                <Label class="text-xs text-muted-foreground">隧道令牌 (留空沿用已保存)</Label>
                <Input
                  v-model="tunnelTokenInput"
                  type="password"
                  placeholder="粘贴隧道令牌"
                  class="font-mono text-xs"
                />
              </div>
            </div>
            <div class="flex justify-end gap-2 pt-1">
              <Button
                variant="outline"
                size="sm"
                class="text-xs h-8 cursor-pointer"
                :disabled="tunnelSaving"
                @click="clearTunnelTokenAction"
              >
                清除令牌
              </Button>
              <Button
                variant="secondary"
                size="sm"
                class="text-xs h-8 cursor-pointer"
                :disabled="tunnelSaving"
                @click="saveTunnel"
              >
                <RefreshCw v-if="tunnelSaving" class="h-3 w-3 mr-1 animate-spin" />
                {{ tunnelSaving ? '保存中…' : '保存隧道配置' }}
              </Button>
            </div>
          </div>

          <div class="flex justify-end pt-1">
            <Button @click="saveModeConfig" :disabled="isSaving" class="text-xs h-9 font-medium px-4 cursor-pointer">
              <RefreshCw v-if="isSaving" class="h-3.5 w-3.5 mr-1.5 animate-spin" />
              {{ isSaving ? '保存中…' : '保存出网配置' }}
            </Button>
          </div>
        </div>

        <!-- 方案 B：远端代理详细配置 -->
        <div v-if="currentMode === 'chained'" class="space-y-3.5 pt-1">
          <!-- 口令一键导入 -->
          <div class="rounded-xl border border-border/80 bg-muted/20 p-3.5 space-y-2">
            <div class="text-xs font-semibold flex items-center gap-1.5">
              <Share2 class="h-3.5 w-3.5 text-blue-600" />
              口令一键导入 (多端同步)
            </div>
            <div class="flex gap-2">
              <Input
                v-model="importSyncUri"
                placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
                class="text-xs font-mono"
                @keyup.enter="doImportSync"
              />
              <Button @click="doImportSync" class="text-xs h-9 shrink-0 cursor-pointer">一键导入</Button>
            </div>
            <p class="text-[11px] text-muted-foreground">
              由 Linux Server 执行 <code>pproxy user add</code> 或 <code>pproxy sync export</code> 导出
            </p>
          </div>

          <!-- 手动参数配置 -->
          <div class="rounded-xl border border-border/80 bg-muted/20 p-3.5 space-y-3">
            <div class="text-xs font-semibold">手动配置服务器参数</div>
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
              <div class="sm:col-span-3 space-y-1">
                <Label class="text-xs text-muted-foreground">服务器地址 (IP 或域名 : 端口)</Label>
                <Input v-model="remoteHost" placeholder="例如 192.168.1.100:8899" class="text-xs font-mono" />
              </div>
              <div class="space-y-1">
                <Label class="text-xs text-muted-foreground">用户名</Label>
                <Input v-model="remoteUser" placeholder="用户名" class="text-xs" />
              </div>
              <div class="sm:col-span-2 space-y-1">
                <Label class="text-xs text-muted-foreground">密码</Label>
                <Input v-model="remotePass" type="password" placeholder="密码" class="text-xs" />
              </div>
            </div>
          </div>

          <div class="flex justify-end pt-1">
            <Button @click="saveModeConfig" :disabled="isSaving" class="text-xs h-9 font-medium px-4 cursor-pointer">
              <RefreshCw v-if="isSaving" class="h-3.5 w-3.5 mr-1.5 animate-spin" />
              {{ isSaving ? '保存中…' : '保存并连接' }}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>

    <!-- 自定义加速域名名单（白名单） -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <div class="flex items-center justify-between">
          <CardTitle class="text-base flex items-center gap-2">
            <ShieldCheck class="h-4 w-4 text-emerald-600" />
            智能分流加速名单
          </CardTitle>
          <span class="rounded-full bg-muted px-2.5 py-0.5 text-xs text-muted-foreground font-mono">
            {{ whitelistEntries.length }} 个自定义
          </span>
        </div>
        <CardDescription>
          智能分流模式下生效。添加主域名（如 huggingface.co）将自动覆盖全部子域名；常用海外站点已默认内置。
        </CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="flex gap-2">
          <Input
            v-model="newWhitelistEntry"
            placeholder="输入需要加速的域名或网址，如 huggingface.co"
            class="text-xs font-mono"
            @keyup.enter="addWhitelistEntry()"
          />
          <Button
            size="sm"
            class="text-xs h-9 shrink-0 cursor-pointer"
            :disabled="!newWhitelistEntry.trim() || isAddingDomain"
            @click="addWhitelistEntry()"
          >
            <Plus class="h-3.5 w-3.5 mr-1" />
            {{ isAddingDomain ? '添加中…' : '添加域名' }}
          </Button>
        </div>

        <div v-if="whitelistEntries.length" class="flex flex-wrap gap-1.5 pt-1">
          <span
            v-for="(e, i) in whitelistEntries"
            :key="e"
            class="inline-flex items-center gap-1.5 rounded-full bg-muted/80 hover:bg-muted px-3 py-1 text-xs text-foreground/90 border border-border/50 shadow-xs font-mono transition-colors"
          >
            {{ e }}
            <button
              type="button"
              class="rounded-full text-muted-foreground hover:text-rose-500 hover:bg-rose-500/10 leading-none p-0.5 cursor-pointer ml-0.5 transition-colors"
              title="移除域名"
              :aria-label="`移除域名 ${e}`"
              @click="removeWhitelistEntry(i)"
            >
              ×
            </button>
          </span>
        </div>
        <div v-else class="rounded-lg border border-dashed border-border py-4 text-center text-xs text-muted-foreground">
          暂无自定义域名，可在上方输入框添加
        </div>
      </CardContent>
    </Card>

    <!-- 启动与系统偏好 -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <div class="flex items-center justify-between">
          <CardTitle class="text-base flex items-center gap-2">
            <Sparkles class="h-4 w-4 text-primary" />
            启动与系统偏好
          </CardTitle>
        </div>
        <CardDescription>
          管理软件启动时的默认行为
        </CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <div class="flex items-center justify-between rounded-xl border border-border/80 bg-muted/20 p-3.5">
          <div class="space-y-0.5 pr-4">
            <div class="text-xs font-semibold text-foreground">启动即默认开启代理</div>
            <div class="text-xs text-muted-foreground">软件启动时自动接管系统代理（默认开启智能分流模式）</div>
          </div>
          <Switch
            :model-value="autoProxyEnabled"
            @update:model-value="handleAutoProxyToggle"
            :disabled="autoProxySaving"
          />
        </div>
      </CardContent>
    </Card>

    <!-- 系统与维护：网络急救箱与软件更新并排 -->
    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
      <!-- 网络急救箱 -->
      <Card class="border-amber-500/30 bg-amber-500/[0.03] shadow-sm flex flex-col justify-between">
        <CardHeader class="pb-2">
          <CardTitle class="text-sm flex items-center gap-2 text-amber-700 dark:text-amber-400">
            <LifeBuoy class="h-4 w-4 shrink-0" />
            网络急救箱 (Windows)
          </CardTitle>
          <CardDescription class="text-xs">
            如果软件异常退出导致电脑无法上网，一键清除所有系统代理残留并恢复直连。
          </CardDescription>
        </CardHeader>
        <CardContent class="pt-2">
          <Button
            variant="outline"
            size="sm"
            @click="triggerRescue"
            class="w-full border-amber-500/40 text-amber-700 dark:text-amber-400 hover:bg-amber-500/10 text-xs h-8.5 cursor-pointer"
          >
            <LifeBuoy class="h-3.5 w-3.5 mr-1.5" />
            一键恢复系统网络直连
          </Button>
        </CardContent>
      </Card>

      <!-- 软件更新 -->
      <Card class="border-border shadow-sm flex flex-col justify-between">
        <CardHeader class="pb-2">
          <div class="flex items-center justify-between">
            <CardTitle class="text-sm flex items-center gap-2">
              <Download class="h-4 w-4 text-emerald-600 shrink-0" />
              软件更新
            </CardTitle>
            <span v-if="downloading" class="text-xs font-mono font-medium text-emerald-600">
              {{ downloadProgress }}%
            </span>
          </div>
          <CardDescription class="text-xs">
            <span v-if="downloading" class="text-emerald-600 font-medium">
              正在下载更新安装包…
            </span>
            <span v-else-if="downloaded" class="text-emerald-600 font-medium">
              下载完成，正在启动安装程序…
            </span>
            <span v-else-if="checking" class="text-primary font-medium">
              正在连接更新源检查新版本…
            </span>
            <span v-else-if="updateAvailable" class="text-emerald-600 font-medium">
              发现新版本 {{ updateVersion }}，可立即升级
            </span>
            <span v-else>
              当前已是最新版本，保持最新以获得最佳体验
            </span>
          </CardDescription>
        </CardHeader>
        <CardContent class="pt-2 space-y-3">
          <!-- 检查中：不定长进度条（Loading） -->
          <div v-if="checking" class="space-y-1.5 py-1">
            <div class="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>检查更新中…</span>
              <RefreshCw class="h-3 w-3 animate-spin text-primary" />
            </div>
            <Progress indeterminate class="h-1.5" />
          </div>

          <!-- 下载中/已下载：真实进度条 -->
          <div v-else-if="downloading || downloaded" class="space-y-1.5 py-1">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">
                {{ downloaded ? '下载完成，即将启动安装器…' : '正在下载更新…' }}
              </span>
              <span class="font-mono text-emerald-600 font-medium">{{ downloadProgress }}%</span>
            </div>
            <Progress
              :model-value="downloadProgress"
              class="h-1.5"
              indicator-class="bg-emerald-600"
            />
          </div>

          <!-- 错误提示 -->
          <div
            v-if="updateError && !checking && !downloading"
            class="rounded-lg bg-rose-500/10 border border-rose-500/20 px-2.5 py-1.5 text-[11px] text-rose-600 dark:text-rose-400"
          >
            检查更新失败：{{ updateError }}
          </div>

          <!-- 操作按钮 -->
          <div>
            <Button
              v-if="downloading || downloaded"
              disabled
              size="sm"
              class="w-full text-xs h-8.5 bg-emerald-600 text-white opacity-90 cursor-not-allowed"
            >
              <Check v-if="downloaded" class="h-3.5 w-3.5 mr-1.5" />
              <Download v-else class="h-3.5 w-3.5 mr-1.5 animate-bounce" />
              {{ downloaded ? '即将启动安装器…' : `正在下载 (${downloadProgress}%)` }}
            </Button>
            <Button
              v-else-if="updateAvailable"
              @click="downloadAndInstall"
              size="sm"
              class="w-full text-xs h-8.5 cursor-pointer bg-emerald-600 hover:bg-emerald-700 text-white"
            >
              <Download class="h-3.5 w-3.5 mr-1.5" />
              立即升级至 {{ updateVersion }}
            </Button>
            <Button
              v-else
              variant="outline"
              size="sm"
              @click="checkForUpdate"
              :disabled="checking"
              class="w-full text-xs h-8.5 cursor-pointer"
            >
              <RefreshCw v-if="checking" class="h-3 w-3 mr-1.5 animate-spin" />
              {{ checking ? '正在检查…' : '检查新版本' }}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</template>

