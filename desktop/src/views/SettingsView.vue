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

import {
  api,
  isUnauthorized,
  setBaseUrlProvider,
  setTokenProvider,
  type MonitorConfigResp,
} from '@/api/client'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  checkForUpdate,
  downloadAndInstall,
  downloaded,
  downloadProgress,
  downloading,
  updateAvailable,
  updateError,
  updateNotes,
  updateVersion,
} from '@/composables/useUpdater'
import { useToast } from '@/composables/useToast'
import {
  clearAdminToken,
  isTauri,
  loadBackendUrl,
  loadDataPlaneUrl,
  pollIntervalMin,
  saveAdminToken,
  saveBackendUrl,
  saveDataPlaneUrl,
  savePollIntervalMin,
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
      const msg = '✓ 已连通，但本机凭据保存失败：请重试或检查系统凭据库'
      testResult.value = msg
      toast.error(msg)
    }
    settled = true // 成功与持久化半失败均不还原 providers
  } catch (e) {
    // g) 失败三分支文案（沿用现状）；零写入
    if (isUnauthorized(e)) testResult.value = '✗ 已连通但鉴权失败：请检查 admin token'
    else if ((e as { kind?: string }).kind === 'network') testResult.value = '✗ 无法连接：地址不可达或服务未运行'
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
 * SEC-5：config.ts 的 clearAdminToken 吞错（冻结文件不可改），视图层如实化反馈——
 * 不再宣称「已清除」，改为提示清除请求已发出 + 权限不足时的人工兜底路径。
 */
async function forgetToken(): Promise<void> {
  await clearAdminToken()
  tokenInput.value = ''
  hasStoredToken.value = false
  confirmForget.value = false
  toast.info('清除请求已完成；若系统提示权限不足，请在系统凭据管理器中手动删除「pony-desktop」条目')
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
  if (testResult.value.startsWith('✓')) return 'text-emerald-700'
  if (testResult.value.startsWith('✗')) return 'text-red-700'
  return 'text-muted-foreground'
})
</script>

<template>
  <div class="max-w-xl space-y-6">
    <PageHeader title="设置" subtitle="连接网关与通知偏好" />

    <!-- 401 落地警示条（F1 送入，固定文案，不含动态详情） -->
    <div
      v-if="authHint"
      class="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800"
      role="alert"
    >
      登录凭据无效，请在下方重新粘贴 admin token
    </div>

    <!-- 卡一：连接向导（三步打勾 + 单一原子按钮） -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">连接向导</CardTitle>
        <CardDescription>按顺序填好三步，最后一键测试并保存。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-5">
        <!-- 步骤① 管理面地址 -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(1) ? 'bg-emerald-100 text-emerald-700' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(1) ? '✓' : '1' }}
            </span>
            <Label for="cfg-url">管理面地址</Label>
          </div>
          <Input id="cfg-url" v-model="url" placeholder="http://100.x.x.x:8900" class="font-mono text-sm" />
          <p class="text-xs leading-5 text-muted-foreground">
            形如 http://100.x.x.x:8900，需为其他设备可达的 IP（127.0.0.1 仅限本机）
          </p>
        </div>

        <!-- 步骤② 数据面地址（选填） -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(2) ? 'bg-emerald-100 text-emerald-700' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(2) ? '✓' : '2' }}
            </span>
            <Label for="cfg-dataplane">数据面地址（选填）</Label>
          </div>
          <Input
            id="cfg-dataplane"
            v-model="dataPlaneInput"
            placeholder="留空自动推导"
            class="font-mono text-sm"
          />
          <p v-if="derivedPreviewText" class="text-xs text-emerald-700">
            留空将使用推导地址：{{ derivedPreviewText }}
          </p>
          <p class="text-xs leading-5 text-muted-foreground">
            接入设备的 base_url 底座；走公网入口填 https://access.ponyjob.top。管理面无需公网可达
          </p>
        </div>

        <!-- 步骤③ admin token -->
        <div class="space-y-1.5">
          <div class="flex items-center gap-2">
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-medium"
              :class="stepDotDone(3) ? 'bg-emerald-100 text-emerald-700' : 'bg-muted text-muted-foreground'"
              aria-hidden="true"
            >
              {{ stepDotDone(3) ? '✓' : '3' }}
            </span>
            <Label for="cfg-token">admin token</Label>
          </div>
          <Input
            id="cfg-token"
            ref="tokenInputRef"
            v-model="tokenInput"
            type="password"
            placeholder="粘贴服务器上的 admin token（留空沿用已保存凭据）"
            :aria-invalid="tokenFlash ? 'true' : undefined"
            autocomplete="off"
          />
          <ul class="space-y-0.5 text-xs leading-5 text-muted-foreground">
            <li>· 在服务器上查看：部署时注入的环境变量 PPROXY_ADMIN_TOKEN</li>
            <li>· 或执行 journalctl -u pproxy | grep ADMIN_TOKEN，首次启动仅打印一次</li>
            <li>· 或查看服务器 ~/.pony/config.toml 的 admin_token 字段</li>
            <li>· 日志已丢失则设置变量后重启服务端</li>
          </ul>
          <p class="text-xs text-muted-foreground">
            存储位置：{{ isTauri() ? 'Windows 凭据管理器' : '浏览器 localStorage（dev）' }}
            · <button class="underline underline-offset-2 hover:text-foreground" @click="confirmForget = true">清除已存凭据</button>
          </p>
        </div>

        <!-- 单一原子按钮（无独立「保存」「测试」两钮） -->
        <div class="space-y-2 border-t pt-4">
          <div class="flex flex-wrap items-center gap-3">
            <Button :disabled="testing || !url.trim()" @click="testAndSave">
              {{ testing ? '正在测试…' : '测试并保存' }}
            </Button>
            <!-- UX-8：置灰原因就地说明 -->
            <span v-if="!url.trim()" class="text-xs text-muted-foreground">填写管理面地址后可测试</span>
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

    <!-- 卡二：告警通知（档位即点即生效 + 服务端配置一行小字） -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">告警通知</CardTitle>
        <CardDescription>上游配额触及时提醒你，选一档即刻生效。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <div class="flex flex-wrap gap-2">
          <button
            v-for="tier in POLL_TIERS"
            :key="tier.value"
            type="button"
            class="rounded-full border px-3 py-1 text-sm transition-colors duration-150"
            :class="
              pollIntervalMin === tier.value
                ? 'border-primary bg-primary/10 font-medium text-primary'
                : 'border-border text-muted-foreground hover:bg-muted hover:text-foreground'
            "
            :aria-pressed="pollIntervalMin === tier.value"
            @click="applyPollTier(tier.value)"
          >
            {{ tier.label }}
          </button>
        </div>
        <p v-if="monitorConfig && pollHoursText" class="text-xs text-muted-foreground">
          服务端告警阈值 {{ monitorConfig.threshold_pct }}%，配额每 {{ pollHoursText }} 小时轮询一次
        </p>
      </CardContent>
    </Card>

    <!-- 卡三：软件更新（结构保留，文案口语化微调） -->
    <Card>
      <CardHeader><CardTitle class="text-sm">软件更新</CardTitle></CardHeader>
      <CardContent class="space-y-3">
        <div class="flex items-center justify-between text-sm">
          <span>当前版本</span>
          <span class="font-medium">{{ appVersion || '—' }}</span>
        </div>
        <template v-if="updateAvailable">
          <div class="rounded-md border border-emerald-300 bg-emerald-50 px-3 py-2 text-sm dark:border-emerald-700 dark:bg-emerald-950">
            有新版本 <span class="font-semibold">{{ updateVersion }}</span> 可以升级了
            <pre v-if="updateNotes" class="mt-1 whitespace-pre-wrap text-xs text-muted-foreground">{{ updateNotes }}</pre>
          </div>
          <div v-if="downloading" class="text-sm">正在下载… {{ downloadProgress }}%（下载完会自动弹出安装器）</div>
          <Button :disabled="downloading" @click="downloadAndInstall">
            {{ downloaded ? '重启完成更新' : downloading ? `下载中 ${downloadProgress}%` : '下载并安装' }}
          </Button>
        </template>
        <template v-else>
          <p class="text-sm text-muted-foreground">已经是最新版本</p>
          <Button variant="outline" size="sm" @click="checkForUpdate">检查更新</Button>
        </template>
        <p v-if="updateError" class="text-xs text-muted-foreground">检查失败：{{ updateError }}</p>
      </CardContent>
    </Card>

    <!-- 清除已存凭据二次确认（UX-7②：destructive，确认后才调 forgetToken） -->
    <ConfirmDialog
      :open="confirmForget"
      title="清除已存凭据？"
      description="清除后需要重新粘贴 admin token 才能管理网关"
      confirm-text="清除"
      destructive
      @update:open="(v: boolean) => !v && (confirmForget = false)"
      @confirm="forgetToken"
    />
  </div>
</template>
