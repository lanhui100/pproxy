<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref } from 'vue'
import {
  Check,
  Copy,
  Download,
  LifeBuoy,
  Pencil,
  Plus,
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
  isValidTunnelUrl,
  loadAutoProxyConfig,
  loadTunnelConfig,
  parseGateInput,
  saveAutoProxyConfig,
  saveTunnelConfig,
  saveTunnelToken,
  tunnelSelfCheck,
  type TunnelSelfCheck,
} from '@/lib/config'
import InfoTip from '@/components/common/InfoTip.vue'
import { buildAccessUrlDev, cleanDomainInput, type AccessUrlResult } from '@/lib/urls'

const toast = useToast()

// ---- Tooltip 简明说明（去技术黑话） ----
const GATE_INPUT_TIP = '用于开通出口通道。支持粘贴 pony-gate:// 口令或授权码。由服务管理员提供。'
const TUNNEL_URL_TIP = '出网通道接入点地址，默认已优化，通常保持默认即可。'
const SYNC_URI_TIP = '粘贴 pproxy-sync:// 或 pproxy:// 口令，一键同步节点配置与凭据。'
const REMOTE_PASS_TIP = '自建服务器认证密码，仅保存在本机系统凭据库。'
const API_TOKEN_TIP = '用于调用本地 API 代理的访问密钥（形如 pony_xxx）。由服务提供方提供。'
const ACCESS_URL_TIP = '输入模型 base_url（如 api.openai.com/v1），自动推导路由并生成接入地址。'

// ---- 输入框 Ref 引用（用于进入编辑态时自动聚焦） ----
const gateInputRef = ref<HTMLInputElement | null>(null)
const tunnelUrlInputRef = ref<HTMLInputElement | null>(null)
const importSyncInputRef = ref<HTMLInputElement | null>(null)
const remoteHostInputRef = ref<HTMLInputElement | null>(null)
const whitelistInputRef = ref<HTMLInputElement | null>(null)
const apiTokenInputRef = ref<HTMLInputElement | null>(null)

// ---- 自定义加速名单（白名单）----
const whitelistEntries = ref<string[]>([])
const newWhitelistEntry = ref('')
const isAddingWhitelist = ref(false)
const isAddingDomain = ref(false)

// ---- 破坏性操作二次确认状态 ----
const confirmingClearTunnel = ref(false)
const confirmingClearApiToken = ref(false)

function closeAllEditing(): void {
  isEditingGate.value = false
  isEditingTunnelUrl.value = false
  isImportingSync.value = false
  isEditingRemote.value = false
  isAddingWhitelist.value = false
  isEditingApiToken.value = false
  confirmingClearTunnel.value = false
  confirmingClearApiToken.value = false
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

// ---- API 反代地址生成 ----
const providerBaseUrl = ref('')
const apiProxyTokenInput = ref('')
const apiProxyTokenSaved = ref<string | null>(null)
const isEditingApiToken = ref(false)
const accessGenerating = ref(false)
const accessResult = ref<AccessUrlResult | null>(null)
const accessCopied = ref<'local' | 'public' | null>(null)
let accessCopyTimer: ReturnType<typeof setTimeout> | null = null

onUnmounted(() => {
  if (accessCopyTimer) {
    clearTimeout(accessCopyTimer)
    accessCopyTimer = null
  }
})

async function loadApiProxyToken(): Promise<void> {
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      const t = (await invoke('proxy_api_token_get')) as string | null
      apiProxyTokenSaved.value = t
      if (t) apiProxyTokenInput.value = t
    } else {
      const t = localStorage.getItem('pony-dev-api-token')
      apiProxyTokenSaved.value = t
      if (t) apiProxyTokenInput.value = t
    }
  } catch (e) {
    console.warn('Failed to load api proxy token:', e)
  }
}

function startEditApiToken(): void {
  closeAllEditing()
  apiProxyTokenInput.value = apiProxyTokenSaved.value || ''
  isEditingApiToken.value = true
  void nextTick(() => apiTokenInputRef.value?.focus())
}

function cancelEditApiToken(): void {
  apiProxyTokenInput.value = apiProxyTokenSaved.value || ''
  isEditingApiToken.value = false
}

async function saveApiProxyToken(): Promise<void> {
  const raw = apiProxyTokenInput.value.trim()
  if (!raw) {
    await clearApiTokenAction()
    return
  }

  if (raw.startsWith('gate_')) {
    toast.error('令牌类型错误', 'gate_ 开头为出海隧道码，API 反代请使用 pony_xxx 令牌')
    return
  }
  if (!raw.startsWith('pony_')) {
    toast.error('格式不规范', '反代令牌必须以 pony_ 开头（例如 pony_31abc...）')
    return
  }

  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_api_token_set', { token: raw })
    } else {
      localStorage.setItem('pony-dev-api-token', raw)
    }
    apiProxyTokenSaved.value = raw
    isEditingApiToken.value = false
    toast.success('反代令牌已保存生效')
  } catch (e: any) {
    toast.error('保存失败', typeof e === 'string' ? e : e?.message)
  }
}

async function clearApiTokenAction(): Promise<void> {
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_api_token_set', { token: '' })
    } else {
      localStorage.removeItem('pony-dev-api-token')
    }
    apiProxyTokenSaved.value = null
    apiProxyTokenInput.value = ''
    isEditingApiToken.value = false
    confirmingClearApiToken.value = false
    toast.success('已清空反代令牌')
  } catch (e: any) {
    confirmingClearApiToken.value = false
    toast.error('清空失败', typeof e === 'string' ? e : e?.message)
  }
}

async function generateAccessUrls(): Promise<void> {
  if (accessGenerating.value) return
  const raw = providerBaseUrl.value.trim()
  if (!raw) {
    toast.error('请先输入模型提供商的 base_url', '例如 https://api.anthropic.com 或 api.openai.com/v1')
    return
  }
  const tokenCandidate = apiProxyTokenSaved.value || apiProxyTokenInput.value.trim()
  if (tokenCandidate && tokenCandidate.startsWith('gate_')) {
    toast.error('提示：不可使用 gate_ 出海隧道码', 'API 反代需要形如 pony_xxx 的令牌')
    return
  }

  accessGenerating.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      accessResult.value = (await invoke('proxy_access_url_generate', {
        baseUrl: raw,
        customToken: tokenCandidate || null,
      })) as AccessUrlResult
      if (accessResult.value.has_token && tokenCandidate) {
        apiProxyTokenSaved.value = tokenCandidate
      }
    } else {
      accessResult.value = buildAccessUrlDev(raw, tokenCandidate)
      if (tokenCandidate) {
        localStorage.setItem('pony-dev-api-token', tokenCandidate)
        apiProxyTokenSaved.value = tokenCandidate
      }
    }
    accessCopied.value = null
    if (!accessResult.value.has_token) {
      toast.info('已生成，但未配置反代令牌', '令牌段为 <token> 占位，配置 pony_xxx 令牌后自动填充')
    }
  } catch (e: any) {
    accessResult.value = null
    toast.error('生成失败', typeof e === 'string' ? e : e?.message ?? '未知错误')
  } finally {
    accessGenerating.value = false
  }
}

async function copyAccessUrl(kind: 'local' | 'public'): Promise<void> {
  const text = kind === 'local' ? accessResult.value?.local_url : accessResult.value?.public_url
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
    if (accessCopyTimer) {
      clearTimeout(accessCopyTimer)
      accessCopyTimer = null
    }
    accessCopied.value = kind
    toast.success(kind === 'local' ? '本机地址已复制' : '公网地址已复制')
    accessCopyTimer = setTimeout(() => {
      accessCopied.value = null
      accessCopyTimer = null
    }, 2000)
  } catch {
    toast.error('复制失败，请手动选择复制')
  }
}

// ---- 隧道通道与令牌 ----
const tunnelUrlInput = ref('')
const tunnelUrlEditInput = ref('')
const isEditingTunnelUrl = ref(false)
const tunnelHasToken = ref(false)
const tunnelCredError = ref<string | null>(null)
const tunnelFingerprint = ref<string | null>(null)
const tunnelSaving = ref(false)

async function refreshTunnel(): Promise<void> {
  try {
    const c = await loadTunnelConfig()
    tunnelUrlInput.value = c.url
    tunnelUrlEditInput.value = c.url
    tunnelHasToken.value = c.hasToken
    tunnelCredError.value = c.credError ?? null
    tunnelFingerprint.value = c.fingerprint ?? null
  } catch { /* 首次启动无配置 */ }
}

function startEditTunnelUrl(): void {
  closeAllEditing()
  tunnelUrlEditInput.value = tunnelUrlInput.value
  isEditingTunnelUrl.value = true
  void nextTick(() => tunnelUrlInputRef.value?.focus())
}

function cancelEditTunnelUrl(): void {
  tunnelUrlEditInput.value = tunnelUrlInput.value
  isEditingTunnelUrl.value = false
}

async function saveTunnel(): Promise<void> {
  if (tunnelSaving.value) return
  const val = tunnelUrlEditInput.value.trim()
  if (!isValidTunnelUrl(val)) {
    toast.error('隧道端点必须以 wss:// 或 ws:// 开头且不含空白')
    return
  }
  tunnelSaving.value = true
  try {
    await saveTunnelConfig(val, '')
    tunnelUrlInput.value = val
    isEditingTunnelUrl.value = false
    await refreshTunnel()
    toast.success('隧道端点已保存，重新开启代理后生效')
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message))
  } finally {
    tunnelSaving.value = false
  }
}

const gateInput = ref('')
const isEditingGate = ref(false)
const gateSaving = ref(false)
const selfCheck = ref<TunnelSelfCheck | null>(null)
const selfChecking = ref(false)

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
    selfCheck.value = null // 写后旧探测失效（旧 token 结果不得展示在新 token 下）
    await refreshTunnel()
  } catch (e: any) {
    toast.error('保存失败: ' + (typeof e === 'string' ? e : e?.message ?? '未知错误'))
  } finally {
    gateSaving.value = false
  }
}

async function runSelfCheck(): Promise<void> {
  if (selfChecking.value) return
  if (!tunnelHasToken.value) {
    toast.info('尚未配置隧道令牌', '请先配置并保存隧道令牌后再进行自检')
    return
  }
  selfChecking.value = true
  try {
    selfCheck.value = await tunnelSelfCheck()
  } catch (e: any) {
    toast.error('自检失败: ' + (typeof e === 'string' ? e : e?.message ?? '未知错误'))
  } finally {
    selfChecking.value = false
  }
}

async function clearTunnelTokenAction(): Promise<void> {
  const cleared = await clearTunnelToken()
  if (cleared) {
    gateInput.value = ''
    isEditingGate.value = false
    tunnelHasToken.value = false
    tunnelFingerprint.value = null
    confirmingClearTunnel.value = false
    toast.success('已清除隧道令牌')
    return
  }
  confirmingClearTunnel.value = false
  toast.error('清除失败：请在系统凭据管理器中删除「pony-desktop / tunnel_token」')
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

onMounted(async () => {
  void initCurrentVersion()
  void refreshTunnel()
  void refreshWhitelist()
  void refreshAutoProxy()
  void loadApiProxyToken()

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
          <!-- 隧道令牌 -->
          <div class="space-y-2">
            <!-- 常态展示：非输入态 -->
            <div v-if="!isEditingGate" class="flex items-center justify-between gap-4 py-1">
              <div class="space-y-0.5 min-w-0">
                <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                  隧道令牌
                  <InfoTip :text="GATE_INPUT_TIP" />
                </div>
                <div class="text-[11px] text-muted-foreground flex items-center gap-2">
                  <span v-if="tunnelCredError" class="text-rose-500 font-mono">{{ tunnelCredError }}</span>
                  <span v-else-if="tunnelFingerprint" class="font-mono">指纹 sha256:{{ tunnelFingerprint }}…</span>
                  <span v-else-if="tunnelHasToken">已配置</span>
                  <span v-else class="text-muted-foreground/80">未配置</span>
                </div>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <!-- 自检按钮：未配置时禁用并提示，避免未配置触发报红 -->
                <button
                  type="button"
                  @click="runSelfCheck"
                  :disabled="selfChecking || !tunnelHasToken"
                  :title="tunnelHasToken ? '通道自检' : '请先配置隧道令牌'"
                  aria-label="通道自检"
                  class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <RefreshCw class="size-3.5" :class="{ 'animate-spin': selfChecking }" />
                </button>

                <!-- 清除令牌：带二次确认，杜绝误触销毁凭据 -->
                <div v-if="tunnelHasToken" class="inline-flex items-center">
                  <div v-if="confirmingClearTunnel" class="flex items-center gap-1.5 bg-background px-2 py-0.5 rounded-md border border-rose-500/30 text-[11px]">
                    <span class="text-rose-500 font-medium">确定清除？</span>
                    <button
                      type="button"
                      @click="clearTunnelTokenAction"
                      class="text-rose-600 font-medium hover:underline cursor-pointer focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                    >
                      是
                    </button>
                    <button
                      type="button"
                      @click="confirmingClearTunnel = false"
                      class="text-muted-foreground hover:underline cursor-pointer ml-0.5 focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                    >
                      否
                    </button>
                  </div>
                  <button
                    v-else
                    type="button"
                    @click="confirmingClearTunnel = true"
                    title="清除令牌"
                    aria-label="清除令牌"
                    class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-rose-500 hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                  >
                    <Trash2 class="size-3.5" />
                  </button>
                </div>

                <!-- 编辑按钮 -->
                <button
                  type="button"
                  @click="startEditGate"
                  title="修改隧道令牌"
                  aria-label="修改隧道令牌"
                  class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <Pencil class="size-3.5" />
                </button>
              </div>
            </div>

            <!-- 编辑态：带自动聚焦、Enter 保存、Esc 取消 -->
            <div v-else class="space-y-2 py-1">
              <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                修改隧道令牌
                <InfoTip :text="GATE_INPUT_TIP" />
              </div>
              <div class="flex items-center gap-2">
                <Input
                  ref="gateInputRef"
                  v-model="gateInput"
                  type="password"
                  placeholder="粘贴 pony-gate:// 连接口令，或直接粘贴授权码"
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

            <!-- 自检结果展示 -->
            <div v-if="selfCheck" class="rounded-lg bg-background/50 px-3 py-2 space-y-1 text-[11px] font-mono">
              <div v-if="selfCheck.credError" class="text-rose-500">{{ selfCheck.credError }}</div>
              <div v-for="g in selfCheck.gates" :key="g.url" class="flex items-center justify-between">
                <span class="text-muted-foreground">{{ g.name === 'cf' ? '出口C' : '出口V' }}</span>
                <span v-if="g.ok" class="text-foreground">{{ typeof g.ms === 'number' ? `正常 · ${g.ms}ms` : '正常' }}</span>
                <span v-else class="text-rose-500 truncate max-w-56" :title="g.error">失败 · {{ g.error }}</span>
              </div>
              <!-- P0/E4：本地凭据分叉告警（H2 实锤——fallback 与 keyring 不一致） -->
              <div
                v-if="selfCheck.fpFallback && selfCheck.fpKeyring && selfCheck.fpFallback !== selfCheck.fpKeyring"
                class="text-amber-500"
              >
                本地凭据分叉：备份({{ selfCheck.fpFallback }})≠主({{ selfCheck.fpKeyring }})，已按主修复，重贴授权码可彻底统一
              </div>
              <div v-if="selfCheck.credMeta?.fp8" class="text-muted-foreground">
                上次写入：{{ selfCheck.credMeta.source }} · {{ selfCheck.credMeta.fp8 }}
              </div>
            </div>
          </div>

          <!-- 隧道端点 -->
          <div class="border-t border-border/30 pt-3">
            <div v-if="!isEditingTunnelUrl" class="flex items-center justify-between gap-4 py-1">
              <div class="space-y-0.5 min-w-0">
                <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                  隧道端点
                  <InfoTip :text="TUNNEL_URL_TIP" />
                </div>
                <div class="text-[11px] text-muted-foreground font-mono truncate">
                  {{ tunnelUrlInput || '默认官方端点' }}
                </div>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <button
                  type="button"
                  @click="startEditTunnelUrl"
                  title="修改隧道端点"
                  aria-label="修改隧道端点"
                  class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <Pencil class="size-3.5" />
                </button>
              </div>
            </div>

            <div v-else class="space-y-2 py-1">
              <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                修改隧道端点
                <InfoTip :text="TUNNEL_URL_TIP" />
              </div>
              <div class="flex items-center gap-2">
                <Input
                  ref="tunnelUrlInputRef"
                  v-model="tunnelUrlEditInput"
                  placeholder="wss://gate.example.com/ws"
                  class="font-mono text-xs h-8 bg-background"
                  @keyup.enter="saveTunnel"
                  @keydown.esc="cancelEditTunnelUrl"
                />
                <button
                  type="button"
                  @click="saveTunnel"
                  :disabled="tunnelSaving || !tunnelUrlEditInput.trim()"
                  title="保存 (Enter)"
                  aria-label="保存"
                  class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer disabled:opacity-50 focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <RefreshCw v-if="tunnelSaving" class="size-3.5 animate-spin" />
                  <Check v-else class="size-3.5" />
                </button>
                <button
                  type="button"
                  @click="cancelEditTunnelUrl"
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

    <!-- API 反代 -->
    <section class="space-y-2.5">
      <div>
        <h2 class="text-sm font-bold text-foreground flex items-center gap-1.5">
          API 反代
          <InfoTip :text="ACCESS_URL_TIP" />
        </h2>
        <p class="text-xs text-muted-foreground mt-0.5">转换模型服务 base_url 并注入凭据，生成 SDK 接入地址</p>
      </div>

      <div class="rounded-xl bg-muted/60 dark:bg-muted/25 border border-border/20 p-4 space-y-3">
        <!-- 反代令牌配置项 -->
        <div>
          <div v-if="!isEditingApiToken" class="flex items-center justify-between gap-4 py-1">
            <div class="space-y-0.5 min-w-0">
              <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
                反代令牌
                <InfoTip :text="API_TOKEN_TIP" />
              </div>
              <div class="text-[11px] text-muted-foreground font-mono truncate">
                <span v-if="apiProxyTokenSaved">{{ apiProxyTokenSaved.slice(0, 10) }}…{{ apiProxyTokenSaved.slice(-6) }}</span>
                <span v-else>未配置（将使用 &lt;token&gt; 占位）</span>
              </div>
            </div>
            <div class="flex items-center gap-1 shrink-0">
              <!-- 清除令牌带二次确认 -->
              <div v-if="apiProxyTokenSaved" class="inline-flex items-center">
                <div v-if="confirmingClearApiToken" class="flex items-center gap-1.5 bg-background px-2 py-0.5 rounded-md border border-rose-500/30 text-[11px]">
                  <span class="text-rose-500 font-medium">确定清空？</span>
                  <button
                    type="button"
                    @click="clearApiTokenAction"
                    class="text-rose-600 font-medium hover:underline cursor-pointer focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                  >
                    是
                  </button>
                  <button
                    type="button"
                    @click="confirmingClearApiToken = false"
                    class="text-muted-foreground hover:underline cursor-pointer ml-0.5 focus-visible:ring-1 focus-visible:ring-ring/50 outline-none"
                  >
                    否
                  </button>
                </div>
                <button
                  v-else
                  type="button"
                  @click="confirmingClearApiToken = true"
                  title="清空令牌"
                  aria-label="清空令牌"
                  class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-rose-500 hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
                >
                  <Trash2 class="size-3.5" />
                </button>
              </div>
              <button
                type="button"
                @click="startEditApiToken"
                title="修改反代令牌"
                aria-label="修改反代令牌"
                class="h-7 w-7 rounded-md inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Pencil class="size-3.5" />
              </button>
            </div>
          </div>

          <div v-else class="space-y-2 py-1">
            <div class="text-xs font-medium text-foreground flex items-center gap-1.5">
              修改反代令牌
              <InfoTip :text="API_TOKEN_TIP" />
            </div>
            <div class="flex items-center gap-2">
              <Input
                ref="apiTokenInputRef"
                v-model="apiProxyTokenInput"
                placeholder="请输入形如 pony_31abc... 的访问令牌"
                class="font-mono text-xs h-8 bg-background"
                @keyup.enter="saveApiProxyToken"
                @keydown.esc="cancelEditApiToken"
              />
              <button
                type="button"
                @click="saveApiProxyToken"
                title="保存 (Enter)"
                aria-label="保存"
                class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center bg-foreground text-background hover:bg-foreground/90 transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Check class="size-3.5" />
              </button>
              <button
                type="button"
                @click="cancelEditApiToken"
                title="取消 (Esc)"
                aria-label="取消"
                class="h-8 w-8 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <X class="size-3.5" />
              </button>
            </div>
          </div>
        </div>

        <!-- 地址生成行 -->
        <div class="border-t border-border/30 pt-3 space-y-2">
          <div class="flex items-center gap-2">
            <Input
              v-model="providerBaseUrl"
              placeholder="例如 https://api.anthropic.com 或 api.openai.com/v1"
              class="font-mono text-xs h-8 bg-background"
              @keyup.enter="generateAccessUrls"
            />
            <Button
              size="sm"
              variant="secondary"
              class="text-xs h-8 shrink-0 cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              :disabled="accessGenerating || !providerBaseUrl.trim()"
              @click="generateAccessUrls"
            >
              <RefreshCw v-if="accessGenerating" class="size-3.5 mr-1 animate-spin" />
              {{ accessGenerating ? '生成中…' : '生成' }}
            </Button>
          </div>

          <!-- 生成结果展示 -->
          <div v-if="accessResult" class="rounded-lg bg-background/60 p-3 space-y-2 text-xs">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">路由：<code class="font-mono font-medium text-foreground">{{ accessResult.route }}</code></span>
              <span v-if="accessResult.has_token" class="text-muted-foreground font-medium">已注入令牌</span>
              <span v-else class="text-muted-foreground">未配置令牌，需手动替换 &lt;token&gt;</span>
            </div>

            <div class="flex items-center justify-between gap-2">
              <span class="w-10 text-[11px] text-muted-foreground shrink-0">本机</span>
              <code class="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground bg-muted/70 px-2 py-1 rounded" :title="accessResult.local_url">
                {{ accessResult.local_url }}
              </code>
              <button
                type="button"
                @click="copyAccessUrl('local')"
                title="复制本机地址"
                aria-label="复制本机地址"
                class="h-7 w-7 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Check v-if="accessCopied === 'local'" class="size-3.5" />
                <Copy v-else class="size-3.5" />
              </button>
            </div>

            <div class="flex items-center justify-between gap-2">
              <span class="w-10 text-[11px] text-muted-foreground shrink-0">公网</span>
              <code class="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground bg-muted/70 px-2 py-1 rounded" :title="accessResult.public_url">
                {{ accessResult.public_url }}
              </code>
              <button
                type="button"
                @click="copyAccessUrl('public')"
                title="复制公网地址"
                aria-label="复制公网地址"
                class="h-7 w-7 rounded-md shrink-0 inline-flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer focus-visible:ring-2 focus-visible:ring-ring/50 outline-none"
              >
                <Check v-if="accessCopied === 'public'" class="size-3.5" />
                <Copy v-else class="size-3.5" />
              </button>
            </div>
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
      </div>
    </section>
  </div>
</template>
