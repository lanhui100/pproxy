<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { ComponentPublicInstance } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { Check, ChevronDown, Copy, Download, Key, LoaderCircle, Plus, RefreshCw, Trash2 } from '@lucide/vue'

import {
  api,
  isUnauthorized,
  setBaseUrlProvider,
  setTokenProvider,
  type MonitorConfigResp,
  type TokenDto,
} from '@/api/client'
import Chip from '@/components/common/Chip.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useSessionSecret } from '@/composables/useSessionSecret'
import { copySecret } from '@/composables/useSecretCopy'
import { useToast } from '@/composables/useToast'
import { provisionTunnel } from '@/composables/useTunnelProvision'
import {
  checkForUpdate,
  downloadAndInstall,
  checking,
  updateAvailable,
  updateError,
  updateVersion,
} from '@/composables/useUpdater'
import {
  clearAdminToken,
  clearTunnelToken,
  isTauri,
  isValidTunnelUrl,
  loadBackendUrl,
  loadDataPlaneUrl,
  loadTunnelConfig,
  pollIntervalMin,
  saveAdminToken,
  saveBackendUrl,
  saveDataPlaneUrl,
  savePollIntervalMin,
  saveTunnelConfig,
} from '@/lib/config'
import { errText } from '@/lib/errors'
import { fmtDate, fmtRelative } from '@/lib/format'
import { normalizeBaseUrl, normalizeToken } from '@/lib/normalize'
import { tokenStatusLabel } from '@/lib/statusLabels'
import { deriveDataPlane } from '@/lib/urls'

const toast = useToast()
const { setSessionSecret } = useSessionSecret()
const appVersion = ref('')

const url = ref(loadBackendUrl())
const dataPlaneInput = ref(loadDataPlaneUrl())
const tokenInput = ref('')
const testResult = ref('')
const testing = ref(false)
const monitorConfig = ref<MonitorConfigResp | null>(null)

// 401 提示
const authHint = ref(false)
const tokenFlash = ref(false)
const tokenInputRef = ref<ComponentPublicInstance | null>(null)
const hasStoredToken = ref(false)
const confirmForget = ref(false)

// 隧道中继
const tunnelUrlInput = ref('')
const tunnelTokenInput = ref('')
const tunnelHasToken = ref(false)
const tunnelSaving = ref(false)
const confirmForgetTunnelToken = ref(false)
const tunnelAuto = ref<'idle' | 'ok' | 'none' | 'manual'>('idle')
const tunnelAutoUrl = ref('')

// 设备密钥列表与新建
const tokens = ref<TokenDto[]>([])
const tokensLoading = ref(false)
const showCreateToken = ref(false)
const createTokenName = ref('')
const createTokenError = ref('')
const creatingToken = ref(false)
const newlyCreatedToken = ref<string | null>(null)

// 行内就地二次确认删除
const pendingDeleteTokenId = ref<number | null>(null)
let deleteTimer: ReturnType<typeof setTimeout> | null = null

// 仅展示有效活跃密钥，排除已撤销废弃项
const activeTokens = computed(() => tokens.value.filter((t) => t.status === 'active'))

async function refreshTokens(): Promise<void> {
  tokensLoading.value = true
  try {
    tokens.value = (await api.listTokens()).tokens
  } catch {
    /* 未连通时静默 */
  } finally {
    tokensLoading.value = false
  }
}

function triggerDeleteToken(t: TokenDto): void {
  if (pendingDeleteTokenId.value === t.id) {
    // 第二次点击：执行真正删除并从列表清理
    void executeDeleteToken(t.id)
  } else {
    // 第一次点击：转为确认删除图标，启动 4 秒自动还原定时器
    pendingDeleteTokenId.value = t.id
    if (deleteTimer) clearTimeout(deleteTimer)
    deleteTimer = setTimeout(() => {
      pendingDeleteTokenId.value = null
    }, 4000)
  }
}

async function executeDeleteToken(id: number): Promise<void> {
  if (deleteTimer) clearTimeout(deleteTimer)
  pendingDeleteTokenId.value = null
  try {
    await api.revokeToken(id)
    // 彻底从列表中物理清理
    tokens.value = tokens.value.filter((t) => t.id !== id)
    toast.success('密钥已删除并清理')
  } catch (e) {
    toast.error('删除密钥失败', errText(e))
  }
}

async function refreshTunnel(): Promise<void> {
  try {
    const c = await loadTunnelConfig()
    tunnelUrlInput.value = c.url
    tunnelHasToken.value = c.hasToken
    tunnelAutoUrl.value = c.url
  } catch {
    /* 首次无配置保持空 */
  }
}

async function autoProvisionTunnel(): Promise<void> {
  try {
    const r = await provisionTunnel()
    if (r === 'ready') tunnelAuto.value = 'ok'
    else if (r === 'unavailable') tunnelAuto.value = 'none'
    else tunnelAuto.value = 'manual'
  } catch {
    tunnelAuto.value = 'manual'
  }
  await refreshTunnel()
}

async function saveTunnel(): Promise<void> {
  if (tunnelSaving.value) return
  if (!isValidTunnelUrl(tunnelUrlInput.value)) {
    toast.error('隧道端点必须以 wss:// 或 ws:// 开头且不含空白')
    return
  }
  tunnelSaving.value = true
  try {
    await saveTunnelConfig(tunnelUrlInput.value, tunnelTokenInput.value)
    tunnelTokenInput.value = ''
    tunnelAuto.value = 'ok'
    await refreshTunnel()
    toast.success('隧道配置已保存')
  } catch (e) {
    toast.error(errText(e))
  } finally {
    tunnelSaving.value = false
  }
}

async function forgetTunnelToken(): Promise<void> {
  confirmForgetTunnelToken.value = false
  try {
    await clearTunnelToken()
    tunnelHasToken.value = false
    tunnelAuto.value = 'manual'
    toast.success('已清除隧道密钥')
  } catch {
    toast.error('隧道密钥清除失败')
  }
}

onMounted(async () => {
  if (isTauri()) {
    try {
      appVersion.value = await getVersion()
    } catch {
      appVersion.value = ''
    }
  }

  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
  void refreshMonitorConfig()
  void refreshTokens()

  if (typeof window !== 'undefined' && window.history.state?.authInvalidHint) {
    authHint.value = true
    const st: Record<string, unknown> = { ...window.history.state }
    delete st.authInvalidHint
    window.history.replaceState(st, '')
    tokenFlash.value = true
    await nextTick()
    ;(tokenInputRef.value?.$el as HTMLInputElement | undefined)?.focus()
  }

  void tokenFromStore().then((t) => (hasStoredToken.value = Boolean(t)))
  void refreshTunnel().then(autoProvisionTunnel)
})

watch(tokenInput, () => (tokenFlash.value = false))

async function tokenFromStore(): Promise<string | null> {
  if (!isTauri() && typeof localStorage !== 'undefined') {
    return localStorage.getItem('pony-dev-admin-token')
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<string | null>('credential_get')
  } catch {
    return null
  }
}

const adminNorm = computed(() => normalizeBaseUrl(url.value))

function dataPlaneToSave(): string {
  const raw = dataPlaneInput.value.trim()
  if (!raw) return deriveDataPlane(adminNorm.value) ?? ''
  return normalizeBaseUrl(raw)
}

async function testAndSave(): Promise<void> {
  if (testing.value) return
  testing.value = true
  testResult.value = ''

  const prevUrl = url.value
  url.value = normalizeBaseUrl(url.value)
  const token = normalizeToken(tokenInput.value)
  tokenInput.value = token
  const persistSnapshot = { backend: loadBackendUrl(), dataPlane: loadDataPlaneUrl() }

  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => token || (await tokenFromStore()))

  try {
    await api.health({ skipAuthRedirect: true })
    authHint.value = false
    tokenFlash.value = false
    void refreshMonitorConfig()
    void autoProvisionTunnel()
    void refreshTokens()

    try {
      saveBackendUrl(url.value)
      saveDataPlaneUrl(dataPlaneToSave())
      if (token) await saveAdminToken(token)
      setBaseUrlProvider(() => url.value)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
      if (token) hasStoredToken.value = true
      testResult.value = '✓ 连接成功（网关工作正常）'
      toast.success('已连接并保存配置')
    } catch {
      saveBackendUrl(persistSnapshot.backend)
      saveDataPlaneUrl(persistSnapshot.dataPlane)
      setBaseUrlProvider(() => url.value)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
      const msg = '✓ 已连通，但本地凭据保存失败，请检查系统权限'
      testResult.value = msg
      toast.error(msg)
    }
  } catch (e) {
    saveBackendUrl(prevUrl)
    setBaseUrlProvider(() => prevUrl)
    setTokenProvider(tokenFromStore)
    if (isUnauthorized(e)) {
      authHint.value = true
      tokenFlash.value = true
      testResult.value = '✗ 管理员密钥错误'
      toast.error('管理员密钥错误')
    } else {
      const msg = '✗ 无法连接到网关，请检查地址是否正确'
      testResult.value = msg
      toast.error(msg, errText(e))
    }
  } finally {
    testing.value = false
  }
}

async function forgetToken(): Promise<void> {
  confirmForget.value = false
  try {
    await clearAdminToken()
    hasStoredToken.value = false
    tokenInput.value = ''
    toast.success('已清除本地保存的管理员密钥')
  } catch {
    toast.error('清除失败，请检查系统安全凭据访问权限')
  }
}

// 轮询档位
const POLL_TIERS = [
  { value: 60, label: '1 小时' },
  { value: 180, label: '3 小时' },
  { value: 360, label: '6 小时' },
  { value: 720, label: '12 小时' },
  { value: 1440, label: '24 小时' },
]

function applyPollTier(v: number): void {
  savePollIntervalMin(v)
  toast.success('已更新告警偏好')
}

// 软件更新
async function onCheckUpdate(): Promise<void> {
  await checkForUpdate()
  if (updateAvailable.value) {
    toast.info(`发现新版本 ${updateVersion.value}`)
  } else if (!updateError.value) {
    toast.success('当前已是最新版本')
  } else {
    toast.error('检查更新失败', updateError.value)
  }
}

async function onDownloadUpdate(): Promise<void> {
  await downloadAndInstall()
  if (updateError.value) {
    toast.error('下载更新失败', updateError.value)
  } else {
    toast.success('更新已下载，应用即将重启生效')
  }
}

async function refreshMonitorConfig(): Promise<void> {
  try {
    monitorConfig.value = await api.monitorConfig({ skipAuthRedirect: true })
  } catch {
    /* 静默 */
  }
}

const pollHoursText = computed(() => {
  const sec = monitorConfig.value?.poll_interval_sec ?? 0
  if (!sec) return ''
  const h = sec / 3600
  return Number.isInteger(h) ? String(h) : h.toFixed(1)
})

const resultClass = computed(() => {
  if (testResult.value.startsWith('✓')) return 'text-ok font-medium'
  if (testResult.value.startsWith('✗')) return 'text-bad font-medium'
  return 'text-muted-foreground'
})

// 创建设备密钥（名字规范化为 ^[a-zA-Z0-9._-]{1,64}$）
async function doCreateToken(): Promise<void> {
  createTokenError.value = ''
  const cleanName = createTokenName.value.trim().replace(/[^a-zA-Z0-9._-]/g, '_')
  if (!cleanName) {
    createTokenError.value = '请输入合法的密钥名称（仅允许字母、数字、下划线、减号）'
    return
  }
  creatingToken.value = true
  try {
    const res = await api.createToken({
      name: cleanName,
    })
    setSessionSecret(res.token, res.name)
    newlyCreatedToken.value = res.token
    createTokenName.value = ''
    await refreshTokens()
    await copySecret(res.token, {
      onCopied: () => toast.success('新密钥已生成并复制到剪贴板（60 秒自清）'),
    })
  } catch (e) {
    createTokenError.value = errText(e)
  } finally {
    creatingToken.value = false
  }
}

async function copyNewToken(): Promise<void> {
  if (!newlyCreatedToken.value) return
  await copySecret(newlyCreatedToken.value, {
    onCopied: () => toast.success('已复制到剪贴板（60 秒自清）'),
  })
}

function closeCreateModal(): void {
  showCreateToken.value = false
  newlyCreatedToken.value = null
  createTokenName.value = ''
  createTokenError.value = ''
}
</script>

<template>
  <div class="max-w-2xl space-y-6">
    <PageHeader title="设置" subtitle="网关连接、本机接入凭据与偏好" />

    <!-- 401 警示条 -->
    <div
      v-if="authHint"
      class="flex items-start gap-2 rounded-lg bg-bad-soft px-3 py-2 text-xs text-bad"
      role="alert"
    >
      登录凭据已失效，请重新输入管理员密钥并保存。
    </div>

    <!-- 卡一：网关连接 -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm font-semibold">网关连接</CardTitle>
        <CardDescription class="text-xs">填写服务器网关入口，客户端与代理服务均自动对齐。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <div class="space-y-1">
          <Label for="cfg-url" class="text-xs">网关管理地址</Label>
          <Input id="cfg-url" v-model="url" placeholder="http://100.x.x.x:8900" class="font-mono text-xs" />
          <p class="text-[11px] leading-4 text-muted-foreground">
            如 http://100.x.x.x:8900；需确保本机可连通该 IP。
          </p>
        </div>

        <div class="space-y-1">
          <Label for="cfg-token" class="text-xs">管理员密钥</Label>
          <Input
            id="cfg-token"
            ref="tokenInputRef"
            v-model="tokenInput"
            type="password"
            placeholder="留空沿用本机已保存的密钥"
            :aria-invalid="tokenFlash ? 'true' : undefined"
            autocomplete="off"
            class="text-xs"
          />
          <p class="text-[11px] leading-4 text-muted-foreground flex items-center justify-between">
            <span>存于本机系统安全凭据库中。</span>
            <button
              v-if="hasStoredToken"
              class="underline hover:text-foreground cursor-pointer"
              @click="confirmForget = true"
            >
              清除已存密钥
            </button>
            <span v-else class="text-muted-foreground/60">当前未保存密钥</span>
          </p>
        </div>

        <!-- 数据面 Base URL（可选高级项） -->
        <details class="rounded-lg bg-muted/40 p-2.5">
          <summary class="cursor-pointer font-medium text-xs">高级：自定义数据面 Base URL</summary>
          <div class="mt-2 space-y-1">
            <Label for="cfg-data-plane" class="text-[11px]">数据面 Base URL（留空自动推导为 8899 端口）</Label>
            <Input
              id="cfg-data-plane"
              v-model="dataPlaneInput"
              placeholder="默认自动推导，如 http://100.x.x.x:8899"
              class="h-8 text-xs font-mono"
            />
          </div>
        </details>

        <!-- 单一测试并保存动作 -->
        <div class="pt-1 space-y-2">
          <Button size="sm" :disabled="testing || !url.trim()" @click="testAndSave">
            <LoaderCircle v-if="testing" class="size-3.5 animate-spin mr-1.5" />
            {{ testing ? '正在连接…' : '测试并保存连接' }}
          </Button>
          <p v-if="testResult" class="text-xs" :class="resultClass">{{ testResult }}</p>
        </div>
      </CardContent>
    </Card>

    <!-- 卡二：本机接入凭据与密钥管理 Section（折叠展示，支持二次确认删除与物理清理） -->
    <Card>
      <CardHeader class="pb-3">
        <div class="flex items-center justify-between">
          <div class="space-y-0.5">
            <CardTitle class="text-sm font-semibold flex items-center gap-1.5">
              <Key class="size-4 text-primary" />
              接入凭据管理
            </CardTitle>
            <CardDescription class="text-xs">
              用于大模型客户端与外部设备连接此代理网关的鉴权钥匙。
            </CardDescription>
          </div>
          <Button size="xs" variant="outline" @click="showCreateToken = true">
            <Plus class="size-3 mr-1" />
            新建密钥
          </Button>
        </div>
      </CardHeader>
      <CardContent class="space-y-3 pt-0">
        <!-- 折叠面板：已创建密钥列表 -->
        <details class="group rounded-lg bg-muted/40 p-3">
          <summary class="flex cursor-pointer items-center justify-between font-medium text-xs text-muted-foreground select-none list-none">
            <div class="flex items-center gap-1.5">
              <span>已创建密钥列表</span>
              <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground font-mono">
                {{ activeTokens.length }} 个
              </span>
              <InfoTip text="创建专线或外部设备接入时使用的专属 Token，删除后将彻底从网关注销。" />
            </div>
            <ChevronDown class="size-4 text-muted-foreground transition-transform duration-200 group-open:rotate-180" />
          </summary>

          <div class="mt-3 space-y-2 pt-1">
            <div v-if="activeTokens.length" class="space-y-2">
              <div
                v-for="t in activeTokens"
                :key="t.id"
                class="flex items-center justify-between p-2.5 rounded-md bg-background text-xs shadow-xs"
              >
                <div class="space-y-0.5 min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-medium font-mono truncate">{{ t.name === '__admin__' ? '系统管理员（内置）' : t.name }}</span>
                    <StatusDot v-bind="tokenStatusLabel(t.status)" class="text-[10px]" />
                  </div>
                  <p class="text-[11px] text-muted-foreground">
                    创建: {{ fmtDate(t.created_at * 1000) }} · 活跃: {{ t.last_used_at ? fmtRelative(t.last_used_at * 1000) : '从未' }}
                  </p>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                  <!-- 行内二次确认删除按钮：首次点击转确认态，再次点击彻底删除并清理 -->
                  <template v-if="t.name !== '__admin__'">
                    <button
                      v-if="pendingDeleteTokenId === t.id"
                      type="button"
                      class="inline-flex items-center gap-1 rounded bg-bad px-2 py-1 text-[11px] font-medium text-white shadow-xs transition hover:bg-bad/90 cursor-pointer animate-pulse"
                      title="点击确认彻底删除"
                      @click="triggerDeleteToken(t)"
                    >
                      <Check class="size-3" />
                      确认删除
                    </button>
                    <button
                      v-else
                      type="button"
                      class="p-1.5 rounded-md text-muted-foreground hover:text-bad hover:bg-bad-soft transition cursor-pointer"
                      title="删除密钥"
                      @click="triggerDeleteToken(t)"
                    >
                      <Trash2 class="size-3.5" />
                    </button>
                  </template>
                </div>
              </div>
            </div>
            <p v-else class="text-xs text-muted-foreground py-1">暂无活跃密钥，点击右上角可新建</p>
          </div>
        </details>
      </CardContent>
    </Card>

    <!-- 卡三：隧道中继配置 -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm font-semibold flex items-center gap-1.5">
          隧道中继
          <InfoTip text="代理加速名单网站的流量出口，由网关自动配置，无需手动干预。" />
        </CardTitle>
        <CardDescription class="text-xs">代理流量的出口中继通道。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3 text-xs">
        <div class="flex items-center gap-2">
          <template v-if="tunnelAuto === 'ok'">
            <StatusDot tone="ok" label="已自动就绪" />
            <span class="truncate font-mono text-muted-foreground">{{ tunnelAutoUrl }}</span>
          </template>
          <template v-else-if="tunnelAuto === 'idle'">
            <span class="inline-flex items-center gap-1.5 text-muted-foreground animate-pulse">
              正在同步网关配置…
            </span>
          </template>
          <template v-else>
            <StatusDot tone="muted" label="未配置" />
            <span class="text-muted-foreground">可在下方手动填写</span>
          </template>
        </div>

        <details class="rounded-lg bg-muted/40 p-2.5">
          <summary class="cursor-pointer font-medium text-xs">高级：手动中继端点</summary>
          <div class="mt-2.5 space-y-2">
            <div class="space-y-1">
              <Label for="tunnel-url" class="text-xs">隧道端点 (WebSocket)</Label>
              <Input id="tunnel-url" v-model="tunnelUrlInput" placeholder="wss://..." class="h-8 text-xs font-mono" />
            </div>
            <div class="space-y-1">
              <Label for="tunnel-token" class="text-xs">隧道密钥</Label>
              <Input id="tunnel-token" v-model="tunnelTokenInput" type="password" placeholder="留空沿用" class="h-8 text-xs" />
            </div>
            <div class="flex items-center gap-2 pt-1">
              <Button size="xs" :disabled="tunnelSaving" @click="saveTunnel">保存手动端点</Button>
              <Button v-if="tunnelHasToken" size="xs" variant="ghost" class="text-bad" @click="confirmForgetTunnelToken = true">清除密钥</Button>
            </div>
          </div>
        </details>
      </CardContent>
    </Card>

    <!-- 卡四：告警通知与软件更新 -->
    <div class="grid gap-4 md:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle class="text-sm font-semibold">告警通知</CardTitle>
          <CardDescription class="text-xs">上游配额触顶时提醒偏好。</CardDescription>
        </CardHeader>
        <CardContent class="space-y-2">
          <div class="flex flex-wrap gap-1.5">
            <Chip
              v-for="tier in POLL_TIERS"
              :key="tier.value"
              :selected="pollIntervalMin === tier.value"
              class="text-xs"
              @click="applyPollTier(tier.value)"
            >
              {{ tier.label }}
            </Chip>
          </div>
          <p v-if="monitorConfig && pollHoursText" class="text-[11px] text-muted-foreground pt-1">
            服务端阈值 {{ monitorConfig.threshold_pct }}%，每 {{ pollHoursText }} 小时轮询
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <div class="flex items-center justify-between">
            <CardTitle class="text-sm font-semibold">软件版本</CardTitle>
            <Button
              v-if="updateAvailable"
              size="xs"
              @click="onDownloadUpdate"
            >
              <Download class="size-3 mr-1" />
              升级
            </Button>
            <Button
              v-else
              variant="outline"
              size="xs"
              :disabled="checking"
              @click="onCheckUpdate"
            >
              <RefreshCw class="size-3 mr-1" :class="{ 'animate-spin': checking }" />
              检查
            </Button>
          </div>
        </CardHeader>
        <CardContent class="space-y-1 text-xs">
          <div class="flex justify-between">
            <span class="text-muted-foreground">当前版本</span>
            <span class="font-medium">{{ appVersion || '—' }}</span>
          </div>
          <p v-if="updateAvailable" class="text-ok font-medium">有新版本 {{ updateVersion }} 可更新</p>
          <p v-else class="text-muted-foreground">{{ checking ? '正在检查…' : '已是最新版本' }}</p>
        </CardContent>
      </Card>
    </div>

    <!-- 创建新密钥弹窗 -->
    <Dialog :open="showCreateToken" @update:open="(v) => (!v && closeCreateModal())">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle class="text-sm font-semibold flex items-center gap-1.5">
            <Plus class="size-4 text-primary" />
            新建接入密钥
          </DialogTitle>
        </DialogHeader>

        <div class="space-y-3 py-1">
          <div v-if="!newlyCreatedToken" class="space-y-3">
            <div class="space-y-1">
              <Label for="new-token-name" class="text-xs">密钥备注标识</Label>
              <Input
                id="new-token-name"
                v-model="createTokenName"
                placeholder="如 cursor_macbook 或 claude_cli"
                class="h-8 text-xs"
                @keyup.enter="doCreateToken"
              />
              <p class="text-[11px] text-muted-foreground">仅允许英文字母、数字、下划线与减号。</p>
            </div>
            <p v-if="createTokenError" class="text-xs text-bad">{{ createTokenError }}</p>
          </div>

          <!-- 创建成功后展示明文（仅此一次） -->
          <div v-else class="space-y-2 rounded-lg bg-ok-soft/30 p-3">
            <div class="text-xs font-semibold text-ok flex items-center gap-1">
              <Check class="size-3.5" />
              密钥创建成功
            </div>
            <p class="text-[11px] text-muted-foreground leading-relaxed">
              明文仅展示此一次，已自动复制到剪贴板并缓存至当前会话。
            </p>
            <div class="flex items-center gap-2">
              <code class="flex-1 rounded bg-background p-2 text-xs font-mono break-all select-all">
                {{ newlyCreatedToken }}
              </code>
              <Button size="xs" variant="outline" @click="copyNewToken">
                <Copy class="size-3" />
              </Button>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button v-if="!newlyCreatedToken" variant="outline" size="xs" @click="closeCreateModal">取消</Button>
          <Button
            v-if="!newlyCreatedToken"
            size="xs"
            :disabled="creatingToken || !createTokenName.trim()"
            @click="doCreateToken"
          >
            <LoaderCircle v-if="creatingToken" class="size-3 animate-spin mr-1" />
            立即创建
          </Button>
          <Button v-else size="xs" @click="closeCreateModal">完成</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 清除已存管理员密钥确认 -->
    <ConfirmDialog
      :open="confirmForget"
      title="清除已存管理员密钥？"
      description="清除后需重新输入并保存才能继续管理网关。密钥不会被服务器注销。"
      confirm-text="确认清除"
      destructive
      @update:open="(v) => !v && (confirmForget = false)"
      @confirm="forgetToken"
    />

    <!-- 清除隧道密钥确认 -->
    <ConfirmDialog
      :open="confirmForgetTunnelToken"
      title="清除已存隧道密钥？"
      description="清除后隧道中继将无法使用该密钥连接。"
      confirm-text="确认清除"
      destructive
      @update:open="(v) => !v && (confirmForgetTunnelToken = false)"
      @confirm="forgetTunnelToken"
    />
  </div>
</template>
