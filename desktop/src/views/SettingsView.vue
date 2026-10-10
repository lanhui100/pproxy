<script setup lang="ts">
import { nextTick, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import {
  Check,
  ClipboardCheck,
  Copy,
  Download,
  FolderOpen,
  LifeBuoy,
  Pencil,
  Plus,
  QrCode,
  RefreshCw,
  Trash2,
  X,
} from '@lucide/vue'
import { Button } from '@/components/ui/button'
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
  currentVersion,
  initCurrentVersion,
} from '@/composables/useUpdater'
import {
  clearTunnelToken,
  importConnectCode,
  isTauri,
  loadAutoProxyConfig,
  loadTunnelConfig,
  parseGateInput,
  saveAutoProxyConfig,
  saveTunnelToken,
} from '@/lib/config'
import { setAppConfigured } from '@/composables/useAppConfig'
import InfoTip from '@/components/common/InfoTip.vue'
import { cleanDomainInput } from '@/lib/urls'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

const toast = useToast()
const router = useRouter()

// ---- Tooltip 简明说明（去技术黑话） ----
const GATE_INPUT_TIP = '用于接入服务端或授权集群。支持粘贴接入令牌、授权码或连接口令。'
const SYNC_URI_TIP = '粘贴 pproxy-sync:// 或 pproxy:// 口令，一键同步节点配置与凭据。'
const REMOTE_PASS_TIP = '自建服务器认证密码，仅保存在本机系统凭据库。'

// ---- 输入框 Ref 引用（用于进入编辑态时自动聚焦） ----
const gateInputRef = ref<HTMLInputElement | null>(null)
const importSyncInputRef = ref<HTMLInputElement | null>(null)
const remoteHostInputRef = ref<HTMLInputElement | null>(null)
const whitelistInputRef = ref<HTMLInputElement | null>(null)

// ---- 自定义加速名单（白名单）----
const whitelistEntries = ref<string[]>([])
const newWhitelistEntry = ref('')
const isAddingWhitelist = ref(false)
const isAddingDomain = ref(false)

// ---- 破坏性操作二次确认状态 ----
const confirmingClearTunnel = ref(false)

function closeAllEditing(): void {
  isEditingGate.value = false
  isImportingSync.value = false
  isEditingRemote.value = false
  isAddingWhitelist.value = false
  confirmingClearTunnel.value = false
}

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

function startAddWhitelist(): void {
  closeAllEditing()
  newWhitelistEntry.value = ''
  isAddingWhitelist.value = true
  void nextTick(() => whitelistInputRef.value?.focus())
}

function cancelAddWhitelist(): void {
  newWhitelistEntry.value = ''
  isAddingWhitelist.value = false
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
    newWhitelistEntry.value = ''
    isAddingWhitelist.value = false
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

// ---- 隧道通道与令牌 ----
const tunnelHasToken = ref(false)
const tunnelCredError = ref<string | null>(null)
const tunnelFingerprint = ref<string | null>(null)
// P0：常态隧道行扩展展示（分叉徽标 / 数据目录 / cleared 时间线）
const tunnelFpFallback = ref<string | null>(null)
const tunnelFpKeyring = ref<string | null>(null)
const tunnelCredWinner = ref<string | null>(null)
const tunnelDataDir = ref<string | null>(null)
const tunnelDataDirTmpFallback = ref(false)
const tunnelCredSource = ref<string | null>(null)
const tunnelCredWriteTs = ref<number | null>(null)

async function refreshTunnel(): Promise<void> {
  try {
    const c = await loadTunnelConfig()
    tunnelHasToken.value = c.hasToken
    tunnelCredError.value = c.credError ?? null
    tunnelFingerprint.value = c.fingerprint ?? null
    tunnelFpFallback.value = c.fpFallback ?? null
    tunnelFpKeyring.value = c.fpKeyring ?? null
    tunnelCredWinner.value = c.credWinner ?? null
    tunnelDataDir.value = c.dataDir ?? null
    tunnelDataDirTmpFallback.value = c.dataDirTmpFallback === true
    tunnelCredSource.value = c.credMeta?.source ?? null
    tunnelCredWriteTs.value = typeof c.credMeta?.last_write_ts === 'number' ? c.credMeta.last_write_ts : null
  } catch { /* 首次启动无配置 */ }
}

function formatClearedTime(ts: number | null): string {
  if (ts === null || ts === undefined) return ''
  try {
    const d = new Date(ts > 1e12 ? ts : ts * 1000)
    return d.toLocaleString()
  } catch {
    return String(ts)
  }
}

const gateInput = ref('')
const isEditingGate = ref(false)
const gateSaving = ref(false)

function startEditGate(): void {
  closeAllEditing()
  gateInput.value = ''
  isEditingGate.value = true
  void nextTick(() => gateInputRef.value?.focus())
}

function cancelEditGate(): void {
  gateInput.value = ''
  isEditingGate.value = false
}

async function submitGateInput(): Promise<void> {
  if (gateSaving.value) return
  const raw = gateInput.value.trim()
  if (!raw) {
    toast.error('请粘贴连接口令或授权码')
    return
  }
  const parsed = parseGateInput(raw)
  if (raw.startsWith('pony-gate://') && parsed?.kind !== 'code') {
    toast.error('口令格式不正确', '口令已损坏，请向管理员重新索取')
    return
  }
  gateSaving.value = true
  try {
    if (parsed?.kind === 'code') {
      const res = await importConnectCode(raw)
      if (parsed.official === false) {
        toast.info('已导入，端点非官方域名，请确认来源可信', res.url)
      } else {
        toast.success('连接口令已导入', '端点与令牌即时生效')
      }
    } else {
      await saveTunnelToken(raw)
      toast.success('隧道令牌已保存', '即时生效，无需重启')
    }
    gateInput.value = ''
    isEditingGate.value = false
    await refreshTunnel()
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message ?? '未知错误'))
  } finally {
    gateSaving.value = false
  }
}

async function clearTunnelTokenAction(): Promise<void> {
  try {
    await clearTunnelToken()
    if (isTauri()) {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('proxy_disable')
        await invoke('app_config_set', { patch: { configured: false } })
      } catch {}
    } else {
      localStorage.removeItem('pony-dev-tunnel-token')
      localStorage.removeItem('pony-app-config')
    }
    gateInput.value = ''
    isEditingGate.value = false
    tunnelHasToken.value = false
    tunnelFingerprint.value = null
    confirmingClearTunnel.value = false
    setAppConfigured(false)
    toast.success('已清除接入令牌，已返回接入页面')
    await router.push('/')
  } catch (e: any) {
    confirmingClearTunnel.value = false
    toast.error('清除失败: ' + (typeof e === 'string' ? e : e?.message ?? '未知错误'))
  }
}

// ---- 加速模式与远端代理 ----
const currentMode = ref<'direct' | 'chained'>('direct')
const remoteHost = ref('')
const remoteUser = ref('')
const remotePass = ref('')
const editRemoteHost = ref('')
const editRemoteUser = ref('')
const editRemotePass = ref('')
const isEditingRemote = ref(false)
const isSaving = ref(false)

const importSyncUri = ref('')
const importSyncPassphrase = ref('')
const isImportingSync = ref(false)

function startEditRemote(): void {
  closeAllEditing()
  editRemoteHost.value = remoteHost.value
  editRemoteUser.value = remoteUser.value
  editRemotePass.value = ''
  isEditingRemote.value = true
  void nextTick(() => remoteHostInputRef.value?.focus())
}

function cancelEditRemote(): void {
  editRemoteHost.value = remoteHost.value
  editRemoteUser.value = remoteUser.value
  editRemotePass.value = ''
  isEditingRemote.value = false
}

async function saveRemoteConfig(): Promise<void> {
  if (isSaving.value) return
  const host = editRemoteHost.value.trim()
  if (!host) {
    toast.error('服务器地址不能为空', '请输入 IP:端口 或 域名:端口')
    return
  }
  isSaving.value = true
  try {
    const chainedConfig: Record<string, string> = {
      remote_host: host,
      username: editRemoteUser.value.trim(),
    }
    // 关键防御：仅当用户明确键入了新密码时才更新密码；留空表示保留当前保存密码
    if (editRemotePass.value.trim()) {
      chainedConfig.password = editRemotePass.value.trim()
      remotePass.value = editRemotePass.value.trim()
    }
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_mode_switch', {
        modeType: 'chained',
        config: chainedConfig,
      })
    }
    remoteHost.value = host
    remoteUser.value = editRemoteUser.value.trim()
    isEditingRemote.value = false
    toast.success('远端代理配置已保存生效')
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message))
  } finally {
    isSaving.value = false
  }
}

async function switchMode(mode: 'direct' | 'chained'): Promise<void> {
  if (currentMode.value === mode) return
  currentMode.value = mode
  closeAllEditing()
  if (isTauri()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const chainedConfig: Record<string, string> = {
        remote_host: remoteHost.value.trim(),
        username: remoteUser.value.trim(),
      }
      // 关键防御：仅当 remotePass 有值时才发送 password 字段，绝不把空串送入凭据覆盖
      if (remotePass.value.trim()) {
        chainedConfig.password = remotePass.value.trim()
      }
      await invoke('proxy_mode_switch', {
        modeType: mode,
        config: mode === 'chained' ? chainedConfig : null,
      })
      toast.success(mode === 'direct' ? '已切换至专属加速' : '已切换至远端代理')
    } catch (e: any) {
      toast.error('模式切换失败: ' + (typeof e === 'string' ? e : e?.message))
    }
  }
}

function startImportSync(): void {
  closeAllEditing()
  importSyncUri.value = ''
  isImportingSync.value = true
  void nextTick(() => importSyncInputRef.value?.focus())
}

function cancelImportSync(): void {
  importSyncUri.value = ''
  isImportingSync.value = false
}

async function doImportSync(): Promise<void> {
  const uri = importSyncUri.value.trim()
  if (!uri) {
    toast.error('请先粘贴同步口令')
    return
  }
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      const res = (await invoke('proxy_import_sync', {
        syncUri: uri,
        passphrase: importSyncPassphrase.value.trim() || null,
      })) as any
      toast.success(res.message || '导入成功')
      importSyncUri.value = ''
      isImportingSync.value = false
      // Must-fix C：pony-gate:// 经 proxy_import_sync 路由到 tunnel_connect_code_import，
      // 其返回值无 mode 字段——缺失时重读真实配置，禁止落到 'chained' 造成分叉
      {
        const m = (res as any).mode
        if (m === 'direct' || m === 'chained') {
          currentMode.value = m
        } else {
          try {
            const { invoke: inv } = await import('@tauri-apps/api/core')
            const cfg = (await inv('proxy_get_current_config')) as any
            if (cfg.mode_type === 'direct' || cfg.mode_type === 'chained') currentMode.value = cfg.mode_type
          } catch { /* 保持不动 */ }
        }
      }
      if (res.tunnel_retained_fp8) {
        toast.info('同步口令未携带隧道令牌', `已保留本机旧令牌（${res.tunnel_retained_fp8}），如 401 请重贴授权码`)
      }
      const cfg = (await invoke('proxy_get_current_config')) as any
      remoteHost.value = cfg.remote_host || ''
      remoteUser.value = cfg.username || ''
    } else {
      toast.success('口令导入成功')
      importSyncUri.value = ''
      isImportingSync.value = false
      currentMode.value = 'chained'
    }
  } catch (e: any) {
    toast.error('导入失败: ' + (typeof e === 'string' ? e : e?.message))
  }
}

// 启动偏好
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
  const previous = autoProxyEnabled.value
  autoProxyEnabled.value = val
  autoProxySaving.value = true
  try {
    await saveAutoProxyConfig({ auto_proxy: val })
    toast.success(val ? '已开启启动自动代理' : '已关闭启动自动代理')
  } catch (e: any) {
    autoProxyEnabled.value = previous
    toast.error('保存失败', typeof e === 'string' ? e : e?.message)
  } finally {
    autoProxySaving.value = false
  }
}

// 网络急救
async function triggerRescue(): Promise<void> {
  if (!isTauri()) {
    toast.success('网络已恢复直连！')
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const msg = (await invoke('proxy_rescue')) as string
    toast.success(msg || '网络急救成功，已恢复系统直连')
  } catch (e: any) {
    toast.error('急救失败: ' + (typeof e === 'string' ? e : e?.message))
  }
}

// 软件更新
async function handleCheckUpdate(): Promise<void> {
  if (!isTauri()) {
    toast.info('开发模式无需更新', '当前处于开发环境')
    return
  }
  await checkForUpdate()
  if (updateError.value) {
    toast.error('检查更新失败', updateError.value)
  } else if (!updateAvailable.value) {
    toast.success('已是最新版本', '当前版本已是最新')
  }
}

// ---- 诊断与日志导出 ----
const diagnosticCopied = ref(false)

async function copyDiagnosticInfo(): Promise<void> {
  try {
    const diag: Record<string, unknown> = {
      timestamp: new Date().toISOString(),
      platform: isTauri() ? 'tauri-desktop' : 'web-browser',
      mode: currentMode.value,
      hasToken: tunnelHasToken.value,
      fingerprint: tunnelFingerprint.value,
      credError: tunnelCredError.value,
      tunnelSource: tunnelCredSource.value,
      dataDir: tunnelDataDir.value,
      autoProxy: autoProxyEnabled.value,
      whitelistCount: whitelistEntries.value.length,
    }
    await navigator.clipboard.writeText(JSON.stringify(diag, null, 2))
    diagnosticCopied.value = true
    toast.success('诊断信息已复制', '可直接粘贴发给运维技术支持')
    setTimeout(() => { diagnosticCopied.value = false }, 2500)
  } catch {
    toast.error('复制失败，请重试')
  }
}

async function openLogDir(): Promise<void> {
  if (!isTauri()) {
    toast.info('浏览器开发环境', '无本地日志目录')
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const path = await invoke<string>('proxy_open_log_dir')
    toast.success('已打开日志目录', path)
  } catch (e: any) {
    toast.error('打开失败', typeof e === 'string' ? e : e?.message)
  }
}

// ---- Clash Meta 客户端配置与二维码 ----
interface ClashConfigData {
  lan_ip: string
  port: number
  has_token: boolean
  token: string | null
  is_tunnel?: boolean
  tunnel_host?: string | null
  subscription_url: string
  clash_scheme_url?: string
  yaml: string
  qr_svg: string
}

const clashConfig = ref<ClashConfigData | null>(null)
const loadingClash = ref(false)
const showClashDialog = ref(false)
const clashUrlCopied = ref(false)
const clashSchemeCopied = ref(false)

async function fetchClashConfig(): Promise<void> {
  loadingClash.value = true
  if (!isTauri()) {
    clashConfig.value = {
      lan_ip: '192.168.1.100',
      port: 8899,
      has_token: true,
      token: 'pony_dev_mock',
      subscription_url: 'http://192.168.1.100:8899/clash.yaml',
      yaml: '# Dev mock Clash configuration\nmixed-port: 7890\nproxies:\n  - name: Pony-Proxy\n    type: http\n    server: 192.168.1.100\n    port: 8899',
      qr_svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200"><rect width="200" height="200" fill="#f0f0f0"/><text x="100" y="105" text-anchor="middle" font-size="12" fill="#666">二维码开发模拟</text></svg>',
    }
    loadingClash.value = false
    return
  }

  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const res = await invoke<ClashConfigData>('proxy_clash_config_get', {})
    clashConfig.value = res
  } catch (e: any) {
    toast.error('获取 Clash 配置失败', typeof e === 'string' ? e : e?.message)
  } finally {
    loadingClash.value = false
  }
}

async function openClashModal(): Promise<void> {
  await fetchClashConfig()
  showClashDialog.value = true
}

async function copyClashUrl(): Promise<void> {
  if (!clashConfig.value?.subscription_url) return
  try {
    await navigator.clipboard.writeText(clashConfig.value.subscription_url)
    clashUrlCopied.value = true
    toast.success('已复制订阅链接', clashConfig.value.subscription_url)
    setTimeout(() => { clashUrlCopied.value = false }, 2500)
  } catch {
    toast.error('复制失败')
  }
}

async function copyClashScheme(): Promise<void> {
  if (!clashConfig.value?.clash_scheme_url) return
  try {
    await navigator.clipboard.writeText(clashConfig.value.clash_scheme_url)
    clashSchemeCopied.value = true
    toast.success('已复制一键导入链接', clashConfig.value.clash_scheme_url)
    setTimeout(() => { clashSchemeCopied.value = false }, 2500)
  } catch {
    toast.error('复制失败')
  }
}

async function exportClashFile(): Promise<void> {
  if (!isTauri()) {
    toast.info('开发模式', '已模拟保存 ~/.pony/clash.yaml')
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const savedPath = await invoke<string>('proxy_clash_export', {})
    toast.success('配置已保存至本机', savedPath)
  } catch (e: any) {
    toast.error('保存失败', typeof e === 'string' ? e : e?.message)
  }
}

onMounted(async () => {
  void initCurrentVersion()
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
</script>

<template>
  <div class="space-y-8 max-w-3xl mx-auto pb-6">
    <!-- 顶栏标题 -->
    <div class="pb-1">
      <h1 class="text-xl font-bold tracking-tight text-foreground">设置</h1>
      <p class="text-xs text-muted-foreground mt-0.5">管理网络连接、分流规则与系统维护</p>
    </div>

    <!-- 加速模式 -->
    <section class="space-y-2.5">
      <div>
        <h2 class="text-sm font-bold text-foreground">加速模式</h2>
        <p class="text-xs text-muted-foreground mt-0.5">选择出网连接通道，支持随时切换</p>
      </div>

      <div class="rounded-xl bg-muted/60 dark:bg-muted/25 border border-border/20 p-4 space-y-4">
        <!-- 模式切换：极简分段按钮，无边框、无方案A/B编号 -->
        <div class="inline-flex rounded-lg bg-muted/80 p-1 gap-1" role="group" aria-label="加速模式">
          <button
            type="button"
            @click="switchMode('direct')"
            :aria-pressed="currentMode === 'direct'"
            :class="[
              'px-4 py-1.5 rounded-md text-xs font-medium transition-all duration-150 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none',
              currentMode === 'direct'
                ? 'bg-background text-foreground shadow-xs font-semibold'
                : 'text-muted-foreground hover:text-foreground',
            ]"
          >
            专属加速
          </button>
          <button
            type="button"
            @click="switchMode('chained')"
            :aria-pressed="currentMode === 'chained'"
            :class="[
              'px-4 py-1.5 rounded-md text-xs font-medium transition-all duration-150 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none',
              currentMode === 'chained'
                ? 'bg-background text-foreground shadow-xs font-semibold'
                : 'text-muted-foreground hover:text-foreground',
            ]"
          >
            远端代理
          </button>
        </div>

        <!-- 专属加速通道配置 -->
        <div v-if="currentMode === 'direct'" class="space-y-3 pt-1">
          <!-- dev 水印：浏览器便利通道恒绿提示 -->
          <div v-if="!isTauri()" class="rounded-lg bg-amber-500/10 border border-amber-500/30 px-3 py-1.5 text-[11px] text-amber-600 dark:text-amber-400">
            开发模拟·自检恒绿
          </div>
          <!-- 接入令牌（极简化，隧道端点由令牌内嵌/系统内置自动获取，不展示冗余端点） -->
          <div class="space-y-2">
            <!-- 常态展示：非输入态 -->
            <div v-if="!isEditingGate" class="flex items-center justify-between gap-4 py-1">
              <div class="space-y-0.5 min-w-0">
                <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                  接入令牌
                  <InfoTip :text="GATE_INPUT_TIP" />
                </div>
                <div class="text-[11px] text-muted-foreground flex items-center gap-2 flex-wrap">
                  <span v-if="tunnelCredError" class="text-rose-500 font-mono">{{ tunnelCredError }}</span>
                  <span v-else-if="tunnelFingerprint" class="font-mono">指纹 sha256:{{ tunnelFingerprint }}…</span>
                  <span v-else-if="tunnelHasToken" class="text-emerald-600 dark:text-emerald-400 font-medium">已接入</span>
                  <span v-else class="text-muted-foreground/80">未配置</span>
                  <span v-if="tunnelCredSource && tunnelCredSource.includes('cleared')" class="font-mono">
                    已清除{{ tunnelCredWriteTs !== null ? `（${formatClearedTime(tunnelCredWriteTs)}）` : '' }}
                  </span>
                </div>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <!-- 清除/删除接入令牌：带二次确认，删除后立即回退到极简接入页面 -->
                <div v-if="tunnelHasToken" class="inline-flex items-center">
                  <div v-if="confirmingClearTunnel" class="flex items-center gap-1.5 bg-background px-2 py-0.5 rounded-md border border-rose-500/30 text-[11px]">
                    <span class="text-rose-500 font-medium">确定删除令牌？</span>
                    <button
                      type="button"
                      @click="clearTunnelTokenAction"
                      class="text-rose-600 font-medium hover:underline cursor-pointer focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                    >
                      删除
                    </button>
                    <button
                      type="button"
                      @click="confirmingClearTunnel = false"
                      class="text-muted-foreground hover:underline cursor-pointer ml-0.5 focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                    >
                      取消
                    </button>
                  </div>
                  <button
                    v-else
                    type="button"
                    @click="confirmingClearTunnel = true"
                    title="删除接入令牌"
                    aria-label="删除接入令牌"
                    class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-rose-500 hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                  >
                    <Trash2 class="size-3.5" />
                  </button>
                </div>

                <!-- 编辑/更换令牌按钮 -->
                <button
                  type="button"
                  @click="startEditGate"
                  title="更换接入令牌"
                  aria-label="更换接入令牌"
                  class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <Pencil class="size-3.5" />
                </button>
              </div>
            </div>

            <!-- 编辑态：带自动聚焦、Enter 保存、Esc 取消 -->
            <div v-else class="space-y-2 py-1">
              <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                输入接入令牌
                <InfoTip :text="GATE_INPUT_TIP" />
              </div>
              <div class="flex items-center gap-2">
                <Input
                  ref="gateInputRef"
                  v-model="gateInput"
                  type="password"
                  placeholder="粘贴接入令牌 / 授权码"
                  class="font-mono text-xs h-8 bg-background"
                  @keyup.enter="submitGateInput"
                  @keydown.esc="cancelEditGate"
                />
                <button
                  type="button"
                  @click="submitGateInput"
                  :disabled="gateSaving || !gateInput.trim()"
                  title="保存 (Enter)"
                  aria-label="保存"
                  class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer disabled:opacity-50 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <RefreshCw v-if="gateSaving" class="size-3.5 animate-spin" />
                  <Check v-else class="size-3.5" />
                </button>
                <button
                  type="button"
                  @click="cancelEditGate"
                  title="取消 (Esc)"
                  aria-label="取消"
                  class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <X class="size-3.5" />
                </button>
              </div>
            </div>
          </div>
        </div>

        <!-- 远端代理通道配置 -->
        <div v-else class="space-y-3 pt-1">
          <!-- 同步口令导入 -->
          <div>
            <div v-if="!isImportingSync" class="flex items-center justify-between gap-4 py-1">
              <div class="space-y-0.5 min-w-0">
                <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                  同步口令
                  <InfoTip :text="SYNC_URI_TIP" />
                </div>
                <div class="text-[11px] text-muted-foreground">
                  支持快速导入远端节点与认证信息
                </div>
              </div>
              <button
                type="button"
                @click="startImportSync"
                title="导入同步口令"
                aria-label="导入同步口令"
                class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Download class="size-3.5" />
              </button>
            </div>

            <div v-else class="space-y-2 py-1">
              <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                导入同步口令
                <InfoTip :text="SYNC_URI_TIP" />
              </div>
              <div class="flex items-center gap-2">
                <Input
                  ref="importSyncInputRef"
                  v-model="importSyncUri"
                  placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
                  class="font-mono text-xs h-8 bg-background"
                  @keyup.enter="doImportSync"
                  @keydown.esc="cancelImportSync"
                />
                <Input
                  v-model="importSyncPassphrase"
                  type="password"
                  placeholder="同步口令（可选，pproxy-sync:// 导出时生成）"
                  class="font-mono text-xs h-8 bg-background"
                  @keyup.enter="doImportSync"
                  @keydown.esc="cancelImportSync"
                />
                <button
                  type="button"
                  @click="doImportSync"
                  :disabled="!importSyncUri.trim()"
                  title="导入 (Enter)"
                  aria-label="导入"
                  class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer disabled:opacity-50 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <Check class="size-3.5" />
                </button>
                <button
                  type="button"
                  @click="cancelImportSync"
                  title="取消 (Esc)"
                  aria-label="取消"
                  class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <X class="size-3.5" />
                </button>
              </div>
            </div>
          </div>

          <!-- 服务器参数配置 -->
          <div class="border-t border-border/30 pt-3">
            <div v-if="!isEditingRemote" class="flex items-center justify-between gap-4 py-1">
              <div class="space-y-1 min-w-0">
                <div class="text-xs font-medium text-foreground">服务器配置</div>
                <div class="text-[11px] text-muted-foreground font-mono space-y-0.5">
                  <div>地址：{{ remoteHost || '未设置' }}</div>
                  <div>用户：{{ remoteUser || '未设置' }} · 密码：{{ remotePass ? '••••••••' : '未设置' }}</div>
                </div>
              </div>
              <button
                type="button"
                @click="startEditRemote"
                title="修改服务器配置"
                aria-label="修改服务器配置"
                class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Pencil class="size-3.5" />
              </button>
            </div>

            <div v-else class="space-y-2.5 py-1">
              <div class="flex items-center justify-between">
                <span class="text-xs font-medium text-foreground">修改服务器配置</span>
                <div class="flex items-center gap-1">
                  <button
                    type="button"
                    @click="saveRemoteConfig"
                    :disabled="isSaving"
                    title="保存配置 (Enter)"
                    aria-label="保存配置"
                    class="h-7 w-7 rounded-md inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer disabled:opacity-50 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                  >
                    <RefreshCw v-if="isSaving" class="size-3.5 animate-spin" />
                    <Check v-else class="size-3.5" />
                  </button>
                  <button
                    type="button"
                    @click="cancelEditRemote"
                    title="取消 (Esc)"
                    aria-label="取消"
                    class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                  >
                    <X class="size-3.5" />
                  </button>
                </div>
              </div>
              <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
                <div class="sm:col-span-3 space-y-1">
                  <Label for="remote-host-input" class="text-[11px] text-muted-foreground">服务器地址</Label>
                  <Input
                    id="remote-host-input"
                    ref="remoteHostInputRef"
                    v-model="editRemoteHost"
                    placeholder="例如 192.168.1.100:8899"
                    class="text-xs font-mono h-8 bg-background"
                    @keyup.enter="saveRemoteConfig"
                    @keydown.esc="cancelEditRemote"
                  />
                </div>
                <div class="space-y-1">
                  <Label for="remote-user-input" class="text-[11px] text-muted-foreground">用户名</Label>
                  <Input
                    id="remote-user-input"
                    v-model="editRemoteUser"
                    placeholder="用户名"
                    class="text-xs h-8 bg-background"
                    @keyup.enter="saveRemoteConfig"
                    @keydown.esc="cancelEditRemote"
                  />
                </div>
                <div class="sm:col-span-2 space-y-1">
                  <Label for="remote-pass-input" class="text-[11px] text-muted-foreground flex items-center gap-1">
                    代理密码
                    <InfoTip :text="REMOTE_PASS_TIP" />
                  </Label>
                  <Input
                    id="remote-pass-input"
                    v-model="editRemotePass"
                    type="password"
                    placeholder="留空表示保留当前密码"
                    class="text-xs h-8 bg-background"
                    @keyup.enter="saveRemoteConfig"
                    @keydown.esc="cancelEditRemote"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 加速名单 -->
    <section class="space-y-3">
      <div class="flex items-center justify-between">
        <div>
          <h2 class="text-sm font-bold text-foreground">加速名单</h2>
          <p class="text-xs text-muted-foreground mt-0.5">智能分流模式下加速的域名，支持二级域名自动覆盖；常用站点已内置</p>
        </div>
        <div class="flex items-center gap-2 shrink-0">
          <span class="text-[11px] text-muted-foreground font-mono">
            {{ whitelistEntries.length }} 个自定义
          </span>
          <button
            type="button"
            @click="startAddWhitelist"
            title="添加域名"
            aria-label="添加域名"
            class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
          >
            <Plus class="size-3.5" />
          </button>
        </div>
      </div>

      <!-- 添加输入行（非常态） -->
      <div v-if="isAddingWhitelist" class="flex items-center gap-2 max-w-sm">
        <Input
          ref="whitelistInputRef"
          v-model="newWhitelistEntry"
          placeholder="输入域名，如 huggingface.co"
          class="font-mono text-xs h-8 bg-background"
          @keyup.enter="addWhitelistEntry()"
          @keydown.esc="cancelAddWhitelist"
        />
        <button
          type="button"
          @click="addWhitelistEntry()"
          :disabled="!newWhitelistEntry.trim() || isAddingDomain"
          title="确认添加 (Enter)"
          aria-label="确认添加"
          class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer disabled:opacity-50 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
        >
          <RefreshCw v-if="isAddingDomain" class="size-3.5 animate-spin" />
          <Check v-else class="size-3.5" />
        </button>
        <button
          type="button"
          @click="cancelAddWhitelist"
          title="取消 (Esc)"
          aria-label="取消"
          class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
        >
          <X class="size-3.5" />
        </button>
      </div>

      <!-- 域名徽标罗列（直接铺展，无长方形背景卡片） -->
      <div v-if="whitelistEntries.length" class="flex flex-wrap gap-2 pt-0.5">
        <span
          v-for="(e, i) in whitelistEntries"
          :key="e"
          class="inline-flex items-center gap-1.5 rounded-lg bg-muted/80 hover:bg-muted dark:bg-muted/60 dark:hover:bg-muted px-2.5 py-1 text-xs text-foreground font-mono transition-colors"
        >
          {{ e }}
          <button
            type="button"
            class="size-4 inline-flex items-center justify-center -mr-0.5 rounded hover:bg-muted-foreground/20 text-muted-foreground hover:text-foreground cursor-pointer transition-colors focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
            title="移除域名"
            :aria-label="`移除域名 ${e}`"
            @click="removeWhitelistEntry(i)"
          >
            <X class="size-3" />
          </button>
        </span>
      </div>
      <div v-else class="py-1 text-xs text-muted-foreground">
        暂无自定义域名，可点击右上角加号添加
      </div>
    </section>

    <!-- 客户端与移动端生态 (Clash Meta 等) -->
    <section class="space-y-2.5">
      <div>
        <h2 class="text-sm font-bold text-foreground flex items-center gap-1.5">
          客户端与移动端接入
          <InfoTip text="生成适用于手机 Clash Meta、Flclash、小火箭及第三方客户端的代理配置与订阅二维码。" />
        </h2>
        <p class="text-xs text-muted-foreground mt-0.5">一键生成标准 Clash Meta / Mihomo 配置文件与扫码订阅</p>
      </div>

      <div class="rounded-xl bg-muted/60 dark:bg-muted/25 border border-border/20 p-4 space-y-4">
        <div class="flex items-center justify-between gap-4">
          <div class="space-y-0.5">
            <div class="text-xs font-medium text-foreground">Clash Meta / Mihomo 移动端与桌面配置</div>
            <div class="text-[11px] text-muted-foreground">内置智能分流与免流保护规则，支持扫码导入与本地导出</div>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              @click="exportClashFile"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <Download class="size-3.5 mr-1" />
              导出本地文件
            </Button>
            <Button
              variant="default"
              size="sm"
              @click="openClashModal"
              :disabled="loadingClash"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <QrCode class="size-3.5 mr-1" />
              二维码与配置
            </Button>
          </div>
        </div>
      </div>
    </section>

    <!-- 启动偏好 -->
    <section class="space-y-2.5">
      <div>
        <h2 class="text-sm font-bold text-foreground">启动偏好</h2>
        <p class="text-xs text-muted-foreground mt-0.5">管理应用启动时的默认网络行为</p>
      </div>

      <div class="rounded-xl bg-muted/60 dark:bg-muted/25 border border-border/20 p-4">
        <div class="flex items-center justify-between gap-4">
          <label for="auto-proxy-switch" class="space-y-0.5 cursor-pointer">
            <div class="text-xs font-medium text-foreground">开机自动开启代理</div>
            <div class="text-[11px] text-muted-foreground">软件启动时自动接管系统代理</div>
          </label>
          <Switch
            id="auto-proxy-switch"
            aria-label="开机自动开启代理"
            :model-value="autoProxyEnabled"
            @update:model-value="handleAutoProxyToggle"
            :disabled="autoProxySaving"
          />
        </div>
      </div>
    </section>

    <!-- 系统维护 -->
    <section class="space-y-2.5">
      <div>
        <h2 class="text-sm font-bold text-foreground">系统维护</h2>
        <p class="text-xs text-muted-foreground mt-0.5">网络状态急救与客户端版本管理</p>
      </div>

      <div class="rounded-xl bg-muted/60 dark:bg-muted/25 border border-border/20 p-4 space-y-4">
        <!-- 网络急救 -->
        <div class="flex items-center justify-between gap-4">
          <div class="space-y-0.5">
            <div class="text-xs font-medium text-foreground">网络急救</div>
            <div class="text-[11px] text-muted-foreground">异常退出导致无法联网时，一键清除代理残留并恢复直连</div>
          </div>
          <Button
            variant="secondary"
            size="sm"
            @click="triggerRescue"
            class="text-xs h-8 cursor-pointer shrink-0 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
          >
            <LifeBuoy class="size-3.5 mr-1 text-muted-foreground" />
            恢复直连
          </Button>
        </div>

        <!-- 软件更新 -->
        <div class="border-t border-border/30 pt-3 flex items-center justify-between gap-4 relative overflow-hidden">
          <div
            v-if="checking || downloading || downloaded"
            class="absolute top-0 inset-x-0 h-0.5 overflow-hidden bg-muted"
          >
            <Progress
              v-if="checking"
              indeterminate
              class="h-0.5 rounded-none bg-transparent"
            />
            <div
              v-else
              class="h-full bg-foreground transition-all duration-300 ease-out"
              :style="{ width: `${downloadProgress}%` }"
            />
          </div>

          <div class="space-y-0.5">
            <div class="text-xs font-medium text-foreground flex items-center gap-2">
              软件更新
              <span class="text-[11px] font-mono text-muted-foreground">v{{ currentVersion }}</span>
            </div>
            <div class="text-[11px] text-muted-foreground">
              <span v-if="downloading">下载更新包中 ({{ downloadProgress }}%)…</span>
              <span v-else-if="downloaded">下载完成，即将启动安装…</span>
              <span v-else-if="checking">正在检查新版本…</span>
              <span v-else-if="updateError" class="text-rose-500">检查失败：{{ updateError }}</span>
              <span v-else-if="updateAvailable">发现新版本 v{{ updateVersion }}</span>
              <span v-else>已是最新版本</span>
            </div>
          </div>

          <div class="shrink-0">
            <Button
              v-if="downloading || downloaded"
              disabled
              size="sm"
              variant="secondary"
              class="text-xs h-8 cursor-not-allowed"
            >
              <Download class="size-3.5 mr-1 animate-bounce" />
              {{ downloaded ? '安装重启中…' : `${downloadProgress}%` }}
            </Button>
            <Button
              v-else-if="updateAvailable"
              size="sm"
              @click="downloadAndInstall"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <Download class="size-3.5 mr-1" />
              升级至 v{{ updateVersion }}
            </Button>
            <Button
              v-else
              variant="secondary"
              size="sm"
              @click="handleCheckUpdate"
              :disabled="checking"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <RefreshCw v-if="checking" class="size-3.5 mr-1 animate-spin" />
              {{ checking ? '检查中…' : '检查更新' }}
            </Button>
          </div>
        </div>

        <!-- 诊断与日志 -->
        <div class="border-t border-border/30 pt-3 flex items-center justify-between gap-4">
          <div class="space-y-0.5">
            <div class="text-xs font-medium text-foreground">诊断与日志</div>
            <div class="text-[11px] text-muted-foreground">打开应用数据与日志目录，便于快速导出故障诊断材料</div>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              @click="copyDiagnosticInfo"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <ClipboardCheck v-if="diagnosticCopied" class="size-3.5 mr-1 text-emerald-600" />
              <Copy v-else class="size-3.5 mr-1" />
              {{ diagnosticCopied ? '已复制诊断' : '复制诊断信息' }}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              @click="openLogDir"
              class="text-xs h-8 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
            >
              <FolderOpen class="size-3.5 mr-1" />
              打开日志目录
            </Button>
          </div>
        </div>
      </div>
    </section>

    <!-- Clash Meta 配置与二维码弹窗 -->
    <Dialog :open="showClashDialog" @update:open="showClashDialog = $event">
      <DialogContent class="sm:max-w-md p-6">
        <DialogHeader>
          <DialogTitle>Clash Meta / 移动端配置导入</DialogTitle>
          <DialogDescription>
            使用手机 Clash Meta、Flclash 或小火箭扫描二维码，或复制订阅链接进行导入。
          </DialogDescription>
        </DialogHeader>

        <div v-if="clashConfig" class="space-y-4 pt-1">
          <!-- 二维码展示区（去外围边框与内阴影，纯净居中） -->
          <div class="flex flex-col items-center justify-center p-4 bg-white rounded-xl">
            <div
              class="w-48 h-48 flex items-center justify-center overflow-hidden"
              v-html="clashConfig.qr_svg"
            />
            <div class="flex items-center gap-1.5 mt-2">
              <span
                v-if="clashConfig.is_tunnel"
                class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-emerald-100 text-emerald-800"
              >
                公网隧道漫游模式
              </span>
              <span
                v-else
                class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-amber-100 text-amber-800"
              >
                局域网直连模式
              </span>
              <span class="text-[11px] text-zinc-500">
                {{ clashConfig.is_tunnel ? '支持手机公网蜂窝及任意 Wi-Fi 扫码即用' : '手机需与电脑连接同一局域网 Wi-Fi' }}
              </span>
            </div>
          </div>

          <!-- 订阅链接复制（tabindex -1 且聚焦时无 outline/ring，不主动聚焦） -->
          <div class="space-y-1.5">
            <Label class="text-xs text-muted-foreground">订阅 URL</Label>
            <div class="flex items-center gap-2">
              <input
                readonly
                tabindex="-1"
                :value="clashConfig.subscription_url"
                class="font-mono text-xs h-8 bg-muted/50 rounded-lg px-2.5 py-1 text-foreground w-full min-w-0 border-0 outline-none focus:outline-none focus:ring-0 select-all"
              />
              <Button
                variant="secondary"
                size="sm"
                @click="copyClashUrl"
                class="h-8 shrink-0 text-xs"
              >
                <Check v-if="clashUrlCopied" class="size-3.5 mr-1 text-emerald-600" />
                <Copy v-else class="size-3.5 mr-1" />
                {{ clashUrlCopied ? '已复制' : '复制' }}
              </Button>
            </div>
          </div>

          <!-- 一键唤起 / DeepLink 复制 -->
          <div v-if="clashConfig.clash_scheme_url" class="space-y-1.5 pt-1">
            <Label class="text-xs text-muted-foreground">一键导入链接 (DeepLink)</Label>
            <div class="flex items-center gap-2">
              <input
                readonly
                tabindex="-1"
                :value="clashConfig.clash_scheme_url"
                class="font-mono text-xs h-8 bg-muted/50 rounded-lg px-2.5 py-1 text-foreground w-full min-w-0 border-0 outline-none focus:outline-none focus:ring-0 select-all"
              />
              <Button
                variant="secondary"
                size="sm"
                @click="copyClashScheme"
                class="h-8 shrink-0 text-xs"
              >
                <Check v-if="clashSchemeCopied" class="size-3.5 mr-1 text-emerald-600" />
                <Copy v-else class="size-3.5 mr-1" />
                {{ clashSchemeCopied ? '已复制' : '复制' }}
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  </div>
</template>
