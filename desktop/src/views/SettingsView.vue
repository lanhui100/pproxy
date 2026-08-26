<script setup lang="ts">
// 设置 · 连接向导（M7 SPEC §3.3/§3.8）：一屏三步卡（①②③小圆点打勾视觉）
// + 单一原子动作「测试并保存」（P0 契约）+ 告警档位 chips（点击即生效）
// + 服务端监控配置并入告警卡一行只读小字 + 软件更新卡保留。
// 401 在此页豁免全局跳转（本页即 401 落点，防死循环）：health 探测带 skipAuthRedirect；
// App 壳经 history.state.authInvalidHint 送来的落地提示在本页消费后立即清除。
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { ComponentPublicInstance } from 'vue'
import { RouterLink } from 'vue-router'

import { getVersion } from '@tauri-apps/api/app'
import { Download, LoaderCircle, RefreshCw } from '@lucide/vue'

import {
  api,
  isUnauthorized,
  setBaseUrlProvider,
  setTokenProvider,
  type MonitorConfigResp,
} from '@/api/client'
import Chip from '@/components/common/Chip.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  checkForUpdate,
  downloadAndInstall,
  downloaded,
  downloadProgress,
  downloading,
  checking,
  updateAvailable,
  updateError,
  updateNotes,
  updateVersion,
} from '@/composables/useUpdater'
import { provisionTunnel } from '@/composables/useTunnelProvision'
import StatusDot from '@/components/common/StatusDot.vue'
import { useToast } from '@/composables/useToast'
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
import { normalizeBaseUrl, normalizeToken } from '@/lib/normalize'
import { deriveDataPlane } from '@/lib/urls'

const toast = useToast()
const appVersion = ref('')

const url = ref(loadBackendUrl())
const dataPlaneInput = ref(loadDataPlaneUrl())
const tokenInput = ref('')
const testResult = ref('')
const testing = ref(false)
const monitorConfig = ref<MonitorConfigResp | null>(null)

// ---- 401 落地提示（F1 经 router state 投递，一次性消费）----
const authHint = ref(false)
const tokenFlash = ref(false) // token 输入框红色高亮（aria-invalid 样式），开始输入即解除
const tokenInputRef = ref<ComponentPublicInstance | null>(null)

/** 是否已有可用存量凭据（仅探测布尔，不把明文带进组件状态）。 */
const hasStoredToken = ref(false)

// 「清除已存凭据」二次确认弹窗开关（UX-7②：危险操作先确认）
const confirmForget = ref(false)
// 本次会话内连接成功标记（UX-3：成功后展示唯一下一步行动链接）
const connectedThisSession = ref(false)

// ---- 隧道中继（默认自动：网关下发即装；高级折叠保留手动覆盖）----
const tunnelUrlInput = ref('')
const tunnelTokenInput = ref('')
const tunnelHasToken = ref(false)
const tunnelSaving = ref(false)
const confirmForgetToken = ref(false)
// 自动配置状态：idle=进行中 / ok=已就绪 / none=网关未下发 / manual=需手动
const tunnelAuto = ref<'idle' | 'ok' | 'none' | 'manual'>('idle')
const tunnelAutoUrl = ref('')

async function refreshTunnel(): Promise<void> {
  try {
    const c = await loadTunnelConfig()
    tunnelUrlInput.value = c.url
    tunnelHasToken.value = c.hasToken
    tunnelAutoUrl.value = c.url
  } catch {
    /* 首次启动无配置，保持空表单 */
  }
}

/** 自动配置：本地缺失时向网关拉取下发值并静默装配（失败不打扰）。 */
async function autoProvisionTunnel(): Promise<void> {
  try {
    const r = await provisionTunnel()
    if (r === 'ready') tunnelAuto.value = 'ok'
    else if (r === 'unavailable') tunnelAuto.value = 'none'
    else tunnelAuto.value = 'manual'
  } catch {
    tunnelAuto.value = 'manual' // 网络失败等：静默降级为可手动
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
    toast.success('隧道配置已保存，下次启用代理时生效')
  } catch (e) {
    toast.error(errText(e))
  } finally {
    tunnelSaving.value = false
  }
}

async function forgetTunnelToken(): Promise<void> {
  if (await clearTunnelToken()) {
    tunnelHasToken.value = false
    tunnelAuto.value = 'manual'
    toast.success('已清除隧道密钥')
  } else {
    toast.error('隧道密钥清除失败：请在系统凭据管理器中删除「pony-desktop / tunnel_token」条目')
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
  // 装配 client 提供者：地址/token 即时生效（其余页面随后请求即走新配置）
  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
  void refreshMonitorConfig()

  // 消费 F1 的 401 落地标记：顶部警示条 + 红色高亮并聚焦 token 输入框，随后清除标记
  //（仅删除本键、保留 vue-router 自身导航状态键，避免破坏后退/位置追踪）
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
  // 隧道自动配置：进设置页即尝试（网关已连时静默装配）
  void refreshTunnel().then(autoProvisionTunnel)
})

// 用户开始输入新 token 即解除红色高亮（程序化写回同样触发，无副作用）
watch(tokenInput, () => (tokenFlash.value = false))

async function tokenFromStore(): Promise<string | null> {
  if (!isTauri() && typeof localStorage !== 'undefined') {
    return localStorage.getItem('pony-dev-admin-token')
  }
  // Tauri 环境经 invoke 回取（不把明文带进本组件状态，仅注入 client）
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<string | null>('credential_get')
  } catch {
    return null
  }
}

// ---- 向导步骤状态（逐步打勾视觉）----

const adminNorm = computed(() => normalizeBaseUrl(url.value))
const step1Done = computed(() => adminNorm.value !== '')
// 步骤②选填：填了算完成；留空但管理面可推导也算完成
const step2Done = computed(
  () => dataPlaneInput.value.trim() !== '' || deriveDataPlane(adminNorm.value) !== null,
)
const step3Done = computed(() => tokenInput.value !== '' || hasStoredToken.value)

function stepDotDone(n: 1 | 2 | 3): boolean {
  return n === 1 ? step1Done.value : n === 2 ? step2Done.value : step3Done.value
}

/** 步骤②动态预览：输入为空且管理面合法时给出推导结果（推导失败显示占位符）。 */
const derivedPreviewText = computed(() => {
  if (!adminNorm.value || dataPlaneInput.value.trim() !== '') return ''
  return deriveDataPlane(adminNorm.value) ?? '—'
})

/** 步骤②保存值（P0 规则）：空 → 存推导值（推导失败才存空串）；非空 → normalize 去尾斜杠。 */
function dataPlaneToSave(): string {
  const raw = dataPlaneInput.value.trim()
  if (!raw) return deriveDataPlane(adminNorm.value) ?? ''
  return normalizeBaseUrl(raw)
}

/**
 * 单一原子动作「测试并保存」（P0 契约，算法不得偏离）：
 * a) 规范化 url/token 并写回输入框；b) 记录 prevUrl=当前 baseUrlProvider 值
 * （mount 时已装配为读取本 ref），同时快照 localStorage 旧 backendUrl/dataPlaneUrl
 * 供持久化失败回滚（ENG-9/SEC-3）；c) 运行时 swap providers →
 * d) api.health({skipAuthRedirect:true})；
 * e) 成功：saveBackendUrl + saveDataPlaneUrl(步骤②结果) + token 非空才 saveAdminToken，
 *    再固化最终闭包，testResult 固定成功文案 + toast「已连接并保存」；
 *    —— 三步落盘包独立 try/catch：keyring 写入失败不再误入网络失败分支
 *    （否则 URL 两 key 已落盘而 providers 已回滚，磁盘/运行态分叉）；
 * f) 失败：finally 中恢复原 providers（对齐 visibilitychange 并发窗口）——
 *    恢复必须放 finally，任何网络失败退出路径都还原；成功路径已在 try 内固化；
 * g) 网络失败文案三分支沿用现状；localStorage 与 keyring 在该路径零写入；
 * h) 持久化失败（ENG-9/SEC-3）：回滚 localStorage 快照（providers 保持新值，
 *    已连通事实成立），独立文案「✓ 已连通，但本机凭据保存失败…」+ toast.error。
 */
async function testAndSave(): Promise<void> {
  if (testing.value) return
  testing.value = true
  testResult.value = ''

  const prevUrl = url.value // b)
  url.value = normalizeBaseUrl(url.value) // a) 写回让用户看到规范化结果
  const token = normalizeToken(tokenInput.value)
  tokenInput.value = token
  // b2) 持久化快照：落盘失败时按此恢复磁盘（ENG-9/SEC-3）
  const persistSnapshot = { backend: loadBackendUrl(), dataPlane: loadDataPlaneUrl() }
  let settled = false // providers 是否已固化到新值（true 则 finally 不再还原）

  // c) 运行时 swap
  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => token || (await tokenFromStore()))

  try {
    await api.health({ skipAuthRedirect: true }) // d) 401 只回给本页展示，不触发全局跳转
    authHint.value = false
    tokenFlash.value = false
    void refreshMonitorConfig() // 已连通即补拉监控配置，点亮告警卡只读小字
    void autoProvisionTunnel() // 已连通即自动装配隧道（本地缺失时）

    // e) 成功：先落盘再固化为最终闭包。持久化段独立 try/catch（ENG-9/SEC-3）
    try {
      saveBackendUrl(url.value)
      saveDataPlaneUrl(dataPlaneToSave())
      if (token) await saveAdminToken(token)
      setBaseUrlProvider(() => url.value)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
      if (token) hasStoredToken.value = true
      connectedThisSession.value = true
      testResult.value = '✓ 连接成功（服务运行正常）'
      toast.success('已连接并保存')
    } catch {
      // h) 持久化失败：磁盘回滚快照；providers 保持新值（已连通事实成立）
      saveBackendUrl(persistSnapshot.backend)
      saveDataPlaneUrl(persistSnapshot.dataPlane)
      setBaseUrlProvider(() => url.value)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
      const msg = '✓ 已连通，但保存到本机时失败：请重试或检查系统凭据库'
      testResult.value = msg
      toast.error(msg)
    }
    settled = true // 成功与持久化半失败均不还原 providers
  } catch (e) {
    // g) 失败三分支文案（沿用现状）；零写入
    if (isUnauthorized(e)) testResult.value = '✗ 网关已连上但密钥不对：请核对管理员密钥'
    else if ((e as { kind?: string }).kind === 'network') testResult.value = '✗ 连不上：地址可能写错了，或服务没有在运行'
    else testResult.value = `✗ ${errText(e)}`
  } finally {
    testing.value = false
    // f) 恢复必须在 finally：恢复到点击前的地址快照，而非当前输入框值
    if (!settled) {
      setBaseUrlProvider(() => prevUrl)
      setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
    }
  }
}

/**
 * 清除已存凭据（UX-7②：经 ConfirmDialog 确认后才执行）。
 * SEC-5 收口：clearAdminToken 返回真实结果（config.ts boolean），成功才清 hasStoredToken
 * （步骤③打勾不失真）；失败保留状态并给出人工兜底路径。
 */
async function forgetToken(): Promise<void> {
  const cleared = await clearAdminToken()
  confirmForget.value = false
  if (cleared) {
    tokenInput.value = ''
    hasStoredToken.value = false
    toast.success('已清除本机保存的管理员密钥')
    return
  }
  toast.error('密钥清除失败：请在系统凭据管理器中手动删除「pony-desktop」条目后重试')
}

// ---- 告警档位（点击即持久化并热生效，无独立保存钮）----

const POLL_TIERS = [
  { value: 5, label: '标准（5 分钟）' },
  { value: 15, label: '安静（15 分钟）' },
  { value: 0, label: '手动（不自动通知）' },
] as const

function applyPollTier(value: number): void {
  // 点击即生效：模块级响应式 ref 更新 + 持久化（composable watch 即时 stop/start）
  savePollIntervalMin(value)
}

// ---- 软件更新（纯图标按钮；错误走 toast 且带一键复制，版本信息保持）----

async function onCheckUpdate(): Promise<void> {
  await checkForUpdate()
  if (updateError.value) {
    toast.error('检查更新失败，点「复制」可导出详情', updateError.value)
  }
}

async function onDownloadUpdate(): Promise<void> {
  await downloadAndInstall()
  if (updateError.value) {
    // updateAvailable 保持 true，下方版本信息不丢；toast 给可复制的错误详情
    toast.error('下载更新失败，点「复制」可导出详情', updateError.value)
  }
}

// ---- 服务端监控配置：降级为一行只读小字，加载失败静默省略整行 ----

async function refreshMonitorConfig(): Promise<void> {
  try {
    monitorConfig.value = await api.monitorConfig({ skipAuthRedirect: true })
  } catch {
    // 静默：未连通/401 时不打扰，该行整体省略（连接成功后会自动补拉）
  }
}

/** 规格公式：(poll_interval_sec / 3600)，整数直接展示，否则保留 1 位小数。 */
const pollHoursText = computed(() => {
  const sec = monitorConfig.value?.poll_interval_sec ?? 0
  if (!sec) return ''
  const h = sec / 3600
  return Number.isInteger(h) ? String(h) : h.toFixed(1)
})

// ---- 结果行配色：✓ 绿 / ✗ 红 / 中性提示灰 ----

const resultClass = computed(() => {
  if (testResult.value.startsWith('✓')) return 'text-ok'
  if (testResult.value.startsWith('✗')) return 'text-bad'
  return 'text-muted-foreground'
})
</script>

<template>
  <div class="max-w-xl space-y-6">
    <PageHeader title="设置" subtitle="连接网关与通知偏好" />

    <!-- 401 落地警示条（F1 送入，固定文案，不含动态详情） -->
    <div
      v-if="authHint"
      class="flex items-start gap-2 rounded-lg bg-bad-soft px-3 py-2 text-sm text-bad"
      role="alert"
    >
      登录凭据已失效，请在下方重新粘贴管理员密钥
    </div>

    <!-- 卡一：连接向导（三步打勾 + 单一原子按钮） -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">连接向导</CardTitle>
        <CardDescription>按顺序填好三步，最后一键测试并保存。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-5">
        <!-- 步骤① 网关地址 -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(1) ? 'bg-ok-soft text-ok' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(1) ? '✓' : '1' }}
            </span>
            <Label for="cfg-url">网关地址</Label>
            <InfoTip text="服务器上网关的管理入口，形如 http://192.168.x.x:8900。手机、电脑等设备要能访问到这个 IP" />
          </div>
          <Input id="cfg-url" v-model="url" placeholder="http://100.x.x.x:8900" class="font-mono text-sm" />
          <p class="text-xs leading-5 text-muted-foreground">
            形如 http://100.x.x.x:8900；其他设备要能访问到这个地址（127.0.0.1 仅限本机使用）
          </p>
        </div>

        <!-- 步骤② 接入地址（选填） -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(2) ? 'bg-ok-soft text-ok' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(2) ? '✓' : '2' }}
            </span>
            <Label for="cfg-dataplane">接入地址（选填）</Label>
            <InfoTip text="设备的请求入口。留空会按网关地址自动推导；需要走公网时填 https://access.ponyjob.top" />
          </div>
          <Input
            id="cfg-dataplane"
            v-model="dataPlaneInput"
            placeholder="留空自动推导"
            class="font-mono text-sm"
          />
          <p v-if="derivedPreviewText" class="text-xs leading-5 text-ok">
            留空将自动使用：{{ derivedPreviewText }}
          </p>
        </div>

        <!-- 步骤③ 管理员密钥 -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(3) ? 'bg-ok-soft text-ok' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(3) ? '✓' : '3' }}
            </span>
            <Label for="cfg-token">管理员密钥</Label>
            <InfoTip
              text="部署网关时生成的一串密码，丢了可以在服务器上找回：执行 journalctl -u pproxy | grep ADMIN_TOKEN 查看首次启动日志；或看 ~/.pony/config.toml 里的 admin_token 字段；日志已丢失则设置环境变量后重启服务端"
            />
          </div>
          <Input
            id="cfg-token"
            ref="tokenInputRef"
            v-model="tokenInput"
            type="password"
            placeholder="粘贴服务器上的管理员密钥（留空沿用已保存的）"
            :aria-invalid="tokenFlash ? 'true' : undefined"
            autocomplete="off"
          />
          <p class="text-xs leading-5 text-muted-foreground">
            密钥保存在本机{{ isTauri() ? '系统凭据管理器' : '浏览器（dev 模式）' }}中，不会上传
            · <button class="underline underline-offset-2 hover:text-foreground" @click="confirmForget = true">清除已保存的密钥</button>
          </p>
        </div>

        <!-- 单一原子按钮（无独立「保存」「测试」两钮） -->
        <div class="space-y-2 pt-1">
          <div class="flex flex-wrap items-center gap-3">
            <Button :disabled="testing || !url.trim()" @click="testAndSave">
              {{ testing ? '正在测试…' : '测试并保存' }}
            </Button>
            <!-- UX-8：置灰原因就地说明 -->
            <span v-if="!url.trim()" class="text-xs text-muted-foreground">填写网关地址后可测试</span>
          </div>
          <p v-if="testResult" class="text-sm" :class="resultClass">{{ testResult }}</p>
          <!-- UX-3：本次会话连接成功后的唯一下一步行动链接 -->
          <p v-if="connectedThisSession" class="text-sm">
            <RouterLink to="/routes" class="text-primary underline-offset-2 hover:underline">
              下一步：去添加服务 ›
            </RouterLink>
          </p>
        </div>
      </CardContent>
    </Card>

    <!-- 卡一点五：隧道中继（默认自动配置；高级折叠保留手动覆盖） -->
    <Card>
      <CardHeader>
        <CardTitle class="flex items-center gap-1 text-sm">
          隧道中继
          <InfoTip text="「代理加速」里名单网站的流量出口，由网关自动下发配置，一般无需手动填写" />
        </CardTitle>
        <CardDescription>连接网关后自动配置好，开箱即用。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <!-- 自动配置状态行 -->
        <div class="flex items-center gap-2 text-sm">
          <template v-if="tunnelAuto === 'ok'">
            <StatusDot tone="ok" label="已自动配置" />
            <span class="min-w-0 truncate font-mono text-xs text-muted-foreground">{{ tunnelAutoUrl }}</span>
          </template>
          <template v-else-if="tunnelAuto === 'idle'">
            <span class="inline-flex items-center gap-1.5 text-muted-foreground">
              <LoaderCircle class="size-3.5 animate-spin" />
              正在从网关获取配置…
            </span>
          </template>
          <template v-else>
            <StatusDot tone="muted" label="未自动配置" />
            <span class="text-xs text-muted-foreground">网关未提供下发配置，可在下方手动填写</span>
          </template>
        </div>

        <!-- 高级：手动覆盖 -->
        <details class="rounded-lg bg-muted/50 px-3 py-2">
          <summary class="cursor-pointer text-sm font-medium">高级：手动配置</summary>
          <div class="mt-3 space-y-3">
            <div class="space-y-1.5">
              <Label for="tunnel-url">隧道端点</Label>
              <Input
                id="tunnel-url"
                v-model="tunnelUrlInput"
                placeholder="wss://your-gate.example/ws"
                autocomplete="off"
                spellcheck="false"
              />
            </div>
            <div class="space-y-1.5">
              <Label for="tunnel-token">隧道密钥</Label>
              <Input
                id="tunnel-token"
                v-model="tunnelTokenInput"
                type="password"
                placeholder="留空沿用已保存的密钥"
                autocomplete="off"
              />
              <p class="text-xs leading-5 text-muted-foreground">
                {{ tunnelHasToken ? '已保存密钥（存于本机系统凭据库）' : '尚未保存密钥' }}
                <template v-if="tunnelHasToken">
                  · <button class="underline underline-offset-2 hover:text-foreground" @click="confirmForgetToken = true">清除密钥</button>
                </template>
              </p>
            </div>
            <Button :disabled="tunnelSaving" size="sm" variant="outline" @click="saveTunnel">
              {{ tunnelSaving ? '保存中…' : '保存手动配置' }}
            </Button>
          </div>
        </details>
      </CardContent>
    </Card>

    <!-- 卡二：告警通知（档位即点即生效 + 服务端配置一行小字） -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">告警通知</CardTitle>
        <CardDescription>上游配额触及时提醒你，选一档即刻生效。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="flex flex-wrap gap-2">
          <!-- 档位即点即生效：选中态切换钮统一走 Chip（UX-11） -->
          <Chip
            v-for="tier in POLL_TIERS"
            :key="tier.value"
            :selected="pollIntervalMin === tier.value"
            @click="applyPollTier(tier.value)"
          >
            {{ tier.label }}
          </Chip>
        </div>
        <p v-if="monitorConfig && pollHoursText" class="text-xs text-muted-foreground">
          服务端告警阈值 {{ monitorConfig.threshold_pct }}%，配额每 {{ pollHoursText }} 小时轮询一次
        </p>
      </CardContent>
    </Card>

    <!-- 卡三：软件更新（纯图标按钮 + 环形进度；错误 toast，版本信息保持） -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">软件更新</CardTitle>
        <!-- 动作区：检查/更新纯图标按钮；下载中变为环形进度条 -->
        <CardAction>
          <template v-if="downloading || downloaded">
            <!-- 环形进度：中心百分比 -->
            <div
              class="relative size-8 shrink-0"
              role="progressbar"
              :aria-valuenow="downloadProgress"
              aria-valuemin="0"
              aria-valuemax="100"
              aria-label="下载进度"
            >
              <svg viewBox="0 0 36 36" class="size-8 -rotate-90">
                <circle cx="18" cy="18" r="15.5" fill="none" stroke-width="3.5" class="stroke-muted" />
                <circle
                  cx="18"
                  cy="18"
                  r="15.5"
                  fill="none"
                  stroke-width="3.5"
                  stroke-linecap="round"
                  class="stroke-primary transition-[stroke-dashoffset] duration-150"
                  :stroke-dasharray="97.4"
                  :stroke-dashoffset="97.4 * (1 - downloadProgress / 100)"
                />
              </svg>
              <span class="absolute inset-0 flex items-center justify-center text-[10px] font-medium tabular-nums">
                {{ downloadProgress }}%
              </span>
            </div>
          </template>
          <Button
            v-else-if="updateAvailable"
            variant="outline"
            size="icon-sm"
            title="下载并安装更新"
            aria-label="下载并安装更新"
            @click="onDownloadUpdate"
          >
            <Download />
          </Button>
          <Button
            v-else
            variant="outline"
            size="icon-sm"
            :disabled="checking"
            title="刷新获取更新"
            aria-label="刷新获取更新"
            @click="onCheckUpdate"
          >
            <RefreshCw :class="{ 'animate-spin': checking }" />
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="flex items-center justify-between text-sm">
          <span>当前版本</span>
          <span class="font-medium">{{ appVersion || '—' }}</span>
        </div>
        <template v-if="updateAvailable">
          <!-- 版本信息常驻：下载中/出错都不消失 -->
          <div class="rounded-lg bg-ok-soft px-3 py-2 text-sm text-ok">
            有新版本 <span class="font-semibold">{{ updateVersion }}</span> 可以升级了
            <pre v-if="updateNotes" class="mt-1 whitespace-pre-wrap text-xs opacity-80">{{ updateNotes }}</pre>
          </div>
          <p v-if="downloading && !downloaded" class="text-xs leading-5 text-muted-foreground">
            正在下载，完成后自动安装，请稍候…
          </p>
          <p v-else-if="downloaded" class="text-xs leading-5 text-muted-foreground">
            下载完成，正在重启应用…
          </p>
        </template>
        <p v-else class="text-sm text-muted-foreground">
          {{ checking ? '正在检查更新…' : '已经是最新版本' }}
        </p>
      </CardContent>
    </Card>

    <!-- 清除隧道令牌二次确认 -->
    <ConfirmDialog
      :open="confirmForgetToken"
      title="清除隧道密钥？"
      description="清除后需重新粘贴新密钥才能使用隧道中继"
      confirm-text="清除"
      destructive
      @update:open="(v: boolean) => !v && (confirmForgetToken = false)"
      @confirm="forgetTunnelToken"
    />

    <!-- 清除已存凭据二次确认（UX-7②：destructive，确认后才调 forgetToken） -->
    <ConfirmDialog
      :open="confirmForget"
      title="清除已保存的密钥？"
      description="清除后需要重新粘贴管理员密钥才能管理网关"
      confirm-text="清除"
      destructive
      @update:open="(v: boolean) => !v && (confirmForget = false)"
      @confirm="forgetToken"
    />
  </div>
</template>
