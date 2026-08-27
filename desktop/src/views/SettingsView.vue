<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { ComponentPublicInstance } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { Check, Copy, Download, Key, LoaderCircle, Plus, RefreshCw, Trash2 } from '@lucide/vue'

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
const revokeTarget = ref<TokenDto | null>(null)
const revokingToken = ref(false)

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
  if (await clearTunnelToken()) {
    tunnelHasToken.value = false
    tunnelAuto.value = 'manual'
    toast.success('已清除隧道密钥')
  } else {
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
  let settled = false

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
    settled = true
  } catch (e) {
    if (isUnauthorized(e)) testResult.value = '✗ 网关已连上但密钥不对：请核对管理员密钥'
    else if ((e as { kind?: string }).kind === 'network') testResult.value = '✗ 无法连接：请检查网关地址是否正确'
    else testResult.value = `✗ ${errText(e)}`
  } finally {
    testing.value = false
    if (!settled) {
      setBaseUrlProvider(() => prevUrl)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
    }
  }
}

async function forgetToken(): Promise<void> {
  const cleared = await clearAdminToken()
  confirmForget.value = false
  if (cleared) {
    tokenInput.value = ''
    hasStoredToken.value = false
    toast.success('已清除本机保存的管理员密钥')
  } else {
    toast.error('密钥清除失败')
  }
}

// 告警档位
const POLL_TIERS = [
  { value: 5, label: '标准（5 分钟）' },
  { value: 15, label: '安静（15 分钟）' },
  { value: 0, label: '手动（不自动通知）' },
] as const

function applyPollTier(value: number): void {
  savePollIntervalMin(value)
}

// 软件更新
async function onCheckUpdate(): Promise<void> {
  await checkForUpdate()
  if (updateError.value) {
    toast.error('检查更新失败', updateError.value)
  }
}

async function onDownloadUpdate(): Promise<void> {
  await downloadAndInstall()
  if (updateError.value) {
    toast.error('下载更新失败', updateError.value)
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

// 创建设备密钥
async function doCreateToken(): Promise<void> {
  createTokenError.value = ''
  if (!createTokenName.value.trim()) {
    createTokenError.value = '请输入密钥备注名称'
    return
  }
  creatingToken.value = true
  try {
    const res = await api.createToken({
      name: createTokenName.value.trim(),
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

async function doRevokeToken(): Promise<void> {
  if (!revokeTarget.value) return
  revokingToken.value = true
  try {
    await api.revokeToken(revokeTarget.value.id)
    revokeTarget.value = null
    await refreshTokens()
    toast.success('密钥已撤销')
  } catch (e) {
    toast.error(errText(e))
  } finally {
    revokingToken.value = false
  }
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

    <!-- 卡二：本机接入凭据与密钥管理 Section -->
    <Card>
      <CardHeader>
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
      <CardContent class="space-y-3">
        <!-- 密钥列表 -->
        <div v-if="tokens.length" class="space-y-2">
          <div
            v-for="t in tokens"
            :key="t.id"
            class="flex items-center justify-between p-2.5 rounded-lg border border-border/50 bg-muted/20 text-xs"
          >
            <div class="space-y-0.5 min-w-0">
              <div class="flex items-center gap-2">
                <span class="font-medium truncate">{{ t.name === '__admin__' ? '系统管理员（内置）' : t.name }}</span>
                <StatusDot v-bind="tokenStatusLabel(t.status)" class="text-[10px]" />
              </div>
              <p class="text-[11px] text-muted-foreground">
                创建: {{ fmtDate(t.created_at * 1000) }} · 活跃: {{ t.last_used_at ? fmtRelative(t.last_used_at * 1000) : '从未' }}
              </p>
            </div>
            <div class="flex items-center gap-1 shrink-0">
              <Button
                v-if="t.name !== '__admin__'"
                variant="ghost"
                size="xs"
                class="text-bad hover:bg-bad-soft cursor-pointer"
                @click="revokeTarget = t"
              >
                <Trash2 class="size-3" />
              </Button>
            </div>
          </div>
        </div>
        <p v-else class="text-xs text-muted-foreground">连接网关后自动加载密钥列表</p>
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
          <DialogTitle>{{ newlyCreatedToken ? '接入密钥已生成' : '新建接入凭据' }}</DialogTitle>
        </DialogHeader>

        <!-- 状态 A：输入名称创建 -->
        <div v-if="!newlyCreatedToken" class="space-y-3 py-2">
          <div class="space-y-1">
            <Label for="token-name-input" class="text-xs">备注名称</Label>
            <Input
              id="token-name-input"
              v-model="createTokenName"
              placeholder="例如：主力电脑、公司开发机"
              class="text-xs"
              @keyup.enter="doCreateToken"
            />
          </div>
          <p v-if="createTokenError" class="text-xs text-bad bg-bad-soft p-2 rounded-md">{{ createTokenError }}</p>
        </div>

        <!-- 状态 B：生成成功明文展示 -->
        <div v-else class="space-y-3 py-2">
          <p class="text-xs text-muted-foreground">密钥仅显示一次，请复制并妥善保管：</p>
          <div class="flex items-center gap-2">
            <code class="flex-1 break-all rounded-md bg-muted p-2 font-mono text-xs text-foreground select-all">
              {{ newlyCreatedToken }}
            </code>
            <Button size="xs" variant="outline" @click="copyNewToken">
              <Copy class="size-3 mr-1" />
              复制
            </Button>
          </div>
          <p class="text-[11px] text-ok">💡 已复制到剪贴板（60 秒自清保护）并暂存于本次运行内存中。</p>
        </div>

        <DialogFooter>
          <template v-if="!newlyCreatedToken">
            <Button variant="outline" size="sm" @click="closeCreateModal">取消</Button>
            <Button size="sm" :disabled="creatingToken || !createTokenName.trim()" @click="doCreateToken">
              <LoaderCircle v-if="creatingToken" class="size-3 animate-spin mr-1" />
              生成密钥
            </Button>
          </template>
          <template v-else>
            <Button size="sm" @click="closeCreateModal">
              <Check class="size-3 mr-1" />
              我已保存并完成
            </Button>
          </template>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 撤销确认 -->
    <ConfirmDialog
      :open="revokeTarget !== null"
      :title="`撤销凭据「${revokeTarget?.name ?? ''}」？`"
      description="使用该密钥的客户端将立即无法访问，且无法恢复。"
      confirm-text="确认撤销"
      destructive
      :busy="revokingToken"
      @update:open="(v) => !v && (revokeTarget = null)"
      @confirm="doRevokeToken"
    />

    <!-- 清除凭据确认 -->
    <ConfirmDialog
      :open="confirmForget"
      title="清除本机管理员密钥？"
      description="清除后需要重新输入管理员密钥才能管理网关。"
      confirm-text="清除"
      destructive
      @update:open="(v) => !v && (confirmForget = false)"
      @confirm="forgetToken"
    />

    <!-- 清除隧道令牌确认 -->
    <ConfirmDialog
      :open="confirmForgetTunnelToken"
      title="清除隧道密钥？"
      description="清除后将停止使用该隧道密钥进行中继出网。"
      confirm-text="清除"
      destructive
      @update:open="(v) => !v && (confirmForgetTunnelToken = false)"
      @confirm="forgetTunnelToken"
    />
  </div>
</template>
