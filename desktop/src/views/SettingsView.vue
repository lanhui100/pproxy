<script setup lang="ts">
import { onMounted, ref } from 'vue'
import {
  Download,
  ExternalLink,
  LifeBuoy,
  RefreshCw,
  Server,
  Share2,
  Sparkles,
  Zap,
} from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useToast } from '@/composables/useToast'
import {
  checkForUpdate,
  downloadAndInstall,
  downloadProgress,
  downloading,
  checking,
  updateAvailable,
  updateVersion,
} from '@/composables/useUpdater'
import {
  clearTunnelToken,
  isTauri,
  isValidTunnelUrl,
  loadTunnelConfig,
  saveTunnelConfig,
} from '@/lib/config'
import { openExternalUrl } from '@/lib/urls'

const toast = useToast()

// ---- 隧道中继（方案 A 出网通道：WS 端点 + 令牌；曾因无配置入口导致令牌无法修复）----
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

onMounted(async () => {
  void refreshTunnel()

  if (isTauri()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const cfg = (await invoke('proxy_get_current_config')) as any
      currentMode.value = cfg.mode_type === 'chained' ? 'chained' : 'direct'
      remoteHost.value = cfg.remote_host || ''
      remoteUser.value = cfg.username || ''
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
    } else {
      toast.success('口令导入成功！')
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
    <div>
      <h1 class="text-2xl font-bold tracking-tight text-foreground">设置中心</h1>
      <p class="text-sm text-muted-foreground mt-0.5">管理您的加速模式、多端配置同步与网络急救</p>
    </div>

    <!-- 隧道中继（方案 A 出网通道：WS 端点 + 令牌） -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <CardTitle class="text-base flex items-center gap-2">
          <Zap class="h-4 w-4 text-blue-600" />
          隧道中继（出网通道）
        </CardTitle>
        <CardDescription>
          方案 A 的 WS 隧道端点与令牌；保存后重新开启代理时生效。
          <span v-if="tunnelHasToken" class="text-emerald-600">本机已保存令牌，留空则沿用。</span>
        </CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="space-y-1">
          <Label class="text-xs font-medium">隧道端点（wss://）</Label>
          <Input v-model="tunnelUrlInput" placeholder="wss://gate.ponyjob.top/ws" class="font-mono text-sm" />
        </div>
        <div class="space-y-1">
          <Label class="text-xs font-medium">隧道令牌</Label>
          <Input
            v-model="tunnelTokenInput"
            type="password"
            placeholder="粘贴隧道令牌（与 gate 的 TUNNEL_TOKEN_HASH 对应）"
            class="font-mono text-sm"
          />
        </div>
        <div class="pt-1 flex justify-end gap-2">
          <Button variant="outline" size="sm" class="text-xs h-9" :disabled="tunnelSaving" @click="clearTunnelTokenAction">
            清除令牌
          </Button>
          <Button size="sm" class="text-xs h-9" :disabled="tunnelSaving" @click="saveTunnel">
            {{ tunnelSaving ? '保存中…' : '保存隧道配置' }}
          </Button>
        </div>
      </CardContent>
    </Card>

    <!-- 加速模式配置 -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <CardTitle class="text-base flex items-center gap-2">
          <Zap class="h-4 w-4 text-primary" />
          加速出网模式
        </CardTitle>
        <CardDescription>选择适合您的加速方案，支持随时切换</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <div class="grid grid-cols-2 gap-3">
          <button
            @click="currentMode = 'direct'"
            :class="[
              'p-4 rounded-xl border text-left transition-all',
              currentMode === 'direct'
                ? 'border-primary bg-primary/5'
                : 'border-border bg-card hover:border-muted-foreground/30',
            ]"
          >
            <div class="font-medium text-sm flex items-center gap-1.5">
              <Sparkles class="h-4 w-4 text-primary" />
              方案 A：个人独立加速
            </div>
            <div class="text-xs text-muted-foreground mt-1">独立 Cloudflare 出口，专属通道极速无干扰</div>
          </button>

          <button
            @click="currentMode = 'chained'"
            :class="[
              'p-4 rounded-xl border text-left transition-all',
              currentMode === 'chained'
                ? 'border-primary bg-primary/5'
                : 'border-border bg-card hover:border-muted-foreground/30',
            ]"
          >
            <div class="font-medium text-sm flex items-center gap-1.5">
              <Server class="h-4 w-4 text-blue-600" />
              方案 B：连接远端代理
            </div>
            <div class="text-xs text-muted-foreground mt-1">连接私有 Linux Server 或局域网其他代理</div>
          </button>
        </div>

        <div v-if="currentMode === 'direct'" class="space-y-3 pt-2">
          <div class="flex items-center justify-between">
            <Label class="text-xs font-medium">更新 Cloudflare API Token</Label>
            <button
              type="button"
              @click="openExternalUrl('https://dash.cloudflare.com/profile/api-tokens')"
              class="text-xs text-primary hover:underline flex items-center gap-1 cursor-pointer bg-transparent border-0 p-0"
            >
              获取 Token <ExternalLink class="h-3 w-3" />
            </button>
          </div>
          <Input v-model="cfToken" type="password" placeholder="如需更新 Token 请在此输入" class="text-sm" />
        </div>

        <div v-if="currentMode === 'chained'" class="grid grid-cols-2 gap-3 pt-2">
          <div class="col-span-2 space-y-1">
            <Label class="text-xs">代理服务器地址 (IP 或域名 : 端口)</Label>
            <Input v-model="remoteHost" placeholder="例如 192.168.1.100:8899" class="text-sm" />
          </div>
          <div class="space-y-1">
            <Label class="text-xs">用户名</Label>
            <Input v-model="remoteUser" placeholder="用户名" class="text-sm" />
          </div>
          <div class="space-y-1">
            <Label class="text-xs">密码</Label>
            <Input v-model="remotePass" type="password" placeholder="密码" class="text-sm" />
          </div>
        </div>

        <div class="pt-2 flex justify-end">
          <Button @click="saveModeConfig" :disabled="isSaving" class="text-xs h-9">
            <RefreshCw v-if="isSaving" class="h-3.5 w-3.5 mr-1.5 animate-spin" />
            {{ isSaving ? '保存中...' : '保存配置' }}
          </Button>
        </div>
      </CardContent>
    </Card>

    <!-- 跨端一键导入与同步 -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <CardTitle class="text-base flex items-center gap-2">
          <Share2 class="h-4 w-4 text-blue-600" />
          多端配置导入
        </CardTitle>
        <CardDescription>支持一键粘贴来自手机或 Linux Server 的同步口令</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="flex gap-2">
          <Input
            v-model="importSyncUri"
            placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
            class="text-xs font-mono"
          />
          <Button @click="doImportSync" class="text-xs h-9 shrink-0">一键导入</Button>
        </div>
      </CardContent>
    </Card>

    <!-- 网络急救箱 -->
    <Card class="border-amber-500/30 bg-amber-500/5 shadow-sm">
      <CardHeader class="pb-3">
        <CardTitle class="text-base flex items-center gap-2 text-amber-700 dark:text-amber-400">
          <LifeBuoy class="h-4 w-4" />
          网络急救箱 (Windows 专属)
        </CardTitle>
        <CardDescription>
          如果软件异常退出导致电脑无法上网，或需要彻底恢复系统直连，点击下方按钮即可一键修复。
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Button
          variant="outline"
          @click="triggerRescue"
          class="border-amber-500/40 text-amber-700 dark:text-amber-400 hover:bg-amber-500/10 text-xs h-9"
        >
          <LifeBuoy class="h-3.5 w-3.5 mr-1.5" />
          一键清除所有代理残留并恢复网络
        </Button>
      </CardContent>
    </Card>

    <!-- 软件更新 -->
    <Card class="border-border shadow-sm">
      <CardHeader class="pb-3">
        <CardTitle class="text-base flex items-center gap-2">
          <Download class="h-4 w-4 text-emerald-600" />
          软件更新
        </CardTitle>
      </CardHeader>
      <CardContent class="flex items-center justify-between">
        <div class="text-xs text-muted-foreground">
          <span v-if="updateAvailable" class="text-emerald-600 font-medium">发现新版本 {{ updateVersion }}</span>
          <span v-else>当前已是最新版本</span>
        </div>
        <Button
          v-if="updateAvailable"
          @click="downloadAndInstall"
          :disabled="downloading"
          size="sm"
          class="text-xs h-8"
        >
          {{ downloading ? `下载中 ${downloadProgress}%` : '立即升级' }}
        </Button>
        <Button v-else variant="outline" size="sm" @click="checkForUpdate" :disabled="checking" class="text-xs h-8">
          <RefreshCw v-if="checking" class="h-3 w-3 mr-1 animate-spin" />
          检查更新
        </Button>
      </CardContent>
    </Card>
  </div>
</template>
