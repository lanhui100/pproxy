<script setup lang="ts">
import { onMounted, ref } from 'vue'
import {
  CheckCircle2,
  ExternalLink,
  LifeBuoy,
  Power,
  RefreshCw,
  Server,
  ShieldCheck,
  Sparkles,
  Zap,
} from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useToast } from '@/composables/useToast'
import { isTauri } from '@/lib/config'
import { openExternalUrl } from '@/lib/urls'

const toast = useToast()

// 运行状态
const isRunning = ref(false)
const proxyMode = ref<'whitelist' | 'global'>('whitelist')
const isConfigured = ref(false)
const configInfo = ref<{
  mode_type: string
  configured: boolean
  worker_url?: string
  remote_host?: string
  username?: string
  has_secret?: boolean
}>({
  mode_type: 'direct',
  configured: false,
})

// 新手向导状态
const setupTab = ref<'direct' | 'chained'>('direct')
const cfToken = ref('')
const syncUriInput = ref('')
const remoteHost = ref('')
const remoteUser = ref('')
const remotePass = ref('')
const isSubmitting = ref(false)

// 常用 AI 服务连通性测试
const testResults = ref<
  Array<{ name: string; host: string; status: 'idle' | 'testing' | 'ok' | 'fail'; latency?: number }>
>([
  { name: 'ChatGPT / OpenAI', host: 'chatgpt.com', status: 'idle' },
  { name: 'Claude / Anthropic', host: 'claude.ai', status: 'idle' },
  { name: 'Google Gemini', host: 'gemini.google.com', status: 'idle' },
  { name: 'GitHub', host: 'github.com', status: 'idle' },
])

onMounted(async () => {
  await refreshStatus()
  if (isRunning.value) {
    runSiteTests()
  }
})

async function refreshStatus() {
  if (!isTauri()) {
    isConfigured.value = true
    isRunning.value = true
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const status = (await invoke('proxy_status')) as { engine_running: boolean; mode: 'whitelist' | 'global' }
    isRunning.value = status.engine_running
    proxyMode.value = status.mode || 'whitelist'

    const cfg = (await invoke('proxy_get_current_config')) as typeof configInfo.value
    configInfo.value = cfg
    isConfigured.value = cfg.configured
  } catch (e) {
    console.error('Failed to get status:', e)
  }
}

async function toggleProxy() {
  if (!isTauri()) {
    isRunning.value = !isRunning.value
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    if (isRunning.value) {
      await invoke('proxy_disable')
      isRunning.value = false
      toast.success('已关闭加速')
    } else {
      await invoke('proxy_enable')
      // 后端启用成功只是「本地监听 + 系统代理接管」；真实出网由后端拨测保证。
      // 这里复核 status 再置位，杜绝「显示已开启但连不上」的假状态。
      const st = (await invoke('proxy_status')) as { engine_running: boolean; mode?: 'whitelist' | 'global' }
      isRunning.value = st.engine_running
      if (st.mode) proxyMode.value = st.mode
      if (!st.engine_running) {
        toast.error('开启失败', '出网拨测未通过，系统代理未启用。请检查方案 A 授权码 / 方案 B 远端地址后重试')
        return
      }
      toast.success('智能加速已开启！')
      runSiteTests()
    }
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '操作失败')
  }
}

async function setProxyMode(mode: 'whitelist' | 'global') {
  if (!isTauri()) {
    proxyMode.value = mode
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('proxy_mode_set', { mode })
    proxyMode.value = mode
    toast.success(mode === 'whitelist' ? '已切换至智能分流模式' : '已切换至全局加速模式')
  } catch (e: any) {
    toast.error('切换模式失败')
  }
}

async function runSiteTests() {
  for (const item of testResults.value) {
    item.status = 'testing'
  }
  if (!isTauri()) {
    setTimeout(() => {
      for (const item of testResults.value) {
        item.status = 'ok'
        item.latency = Math.floor(Math.random() * 80) + 120
      }
    }, 800)
    return
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const results = (await invoke('proxy_test_sites')) as Array<{ site: string; ok: boolean; ms: number }>
    for (const r of results) {
      const match = testResults.value.find(
        (t) => r.site.toLowerCase().includes(t.host.toLowerCase()) || t.host.toLowerCase().includes(r.site.toLowerCase())
      )
      if (match) {
        match.status = r.ok ? 'ok' : 'fail'
        match.latency = r.ms
      }
    }
    // 后端返回的站点若未命中本地列表（列表漂移），不得永久停留「测速中」，兜底为失败
    for (const item of testResults.value) {
      if (item.status === 'testing') {
        item.status = 'fail'
        item.latency = undefined
      }
    }
  } catch (e) {
    // 测速失败要如实展示，绝不伪装成 ok（曾经的假绿会误导用户以为链路畅通）
    for (const item of testResults.value) {
      item.status = 'fail'
      item.latency = undefined
    }
    toast.error('测速失败', String(e))
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
    isRunning.value = false
    toast.success(msg || '网络急救成功，已恢复系统直连！')
  } catch (e: any) {
    toast.error('急救失败：' + (typeof e === 'string' ? e : e?.message))
  }
}

// ---- 向导提交 ----
async function submitDirectSetup() {
  if (!cfToken.value.trim()) {
    toast.error('请输入 Cloudflare API Token')
    return
  }
  isSubmitting.value = true
  try {
    if (isTauri()) {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('proxy_mode_switch', {
        modeType: 'direct',
        config: {
          worker_url: 'https://edge.ponyjob.top',
          proxy_secret: cfToken.value.trim(),
        },
      })
    }
    toast.success('配置成功！已准备就绪。')
    isConfigured.value = true
    await refreshStatus()
    await toggleProxy()
  } catch (e: any) {
    toast.error('配置失败：' + (typeof e === 'string' ? e : e?.message))
  } finally {
    isSubmitting.value = false
  }
}

async function submitImportOrChained() {
  if (syncUriInput.value.trim()) {
    // 口令一键导入
    isSubmitting.value = true
    try {
      if (isTauri()) {
        const { invoke } = await import('@tauri-apps/api/core')
        const res = (await invoke('proxy_import_sync', {
          syncUri: syncUriInput.value.trim(),
        })) as any
        toast.success(res.message || '导入成功！')
      } else {
        toast.success('口令导入成功！')
      }
      isConfigured.value = true
      await refreshStatus()
      await toggleProxy()
    } catch (e: any) {
      toast.error('导入失败：' + (typeof e === 'string' ? e : e?.message))
    } finally {
      isSubmitting.value = false
    }
  } else if (remoteHost.value.trim()) {
    // 手动远端输入
    isSubmitting.value = true
    try {
      if (isTauri()) {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('proxy_mode_switch', {
          modeType: 'chained',
          config: {
            remote_host: remoteHost.value.trim(),
            username: remoteUser.value.trim(),
            password: remotePass.value.trim(),
          },
        })
      }
      toast.success('远端代理已连接！')
      isConfigured.value = true
      await refreshStatus()
      await toggleProxy()
    } catch (e: any) {
      toast.error('连接失败：' + (typeof e === 'string' ? e : e?.message))
    } finally {
      isSubmitting.value = false
    }
  } else {
    toast.error('请粘贴一键口令，或填写服务器地址')
  }
}
</script>

<template>
  <div class="h-full overflow-y-auto p-6 space-y-6 max-w-4xl mx-auto">
    <!-- 未配置向导卡片（零代码小白专属） -->
    <div v-if="!isConfigured" class="space-y-6">
      <div class="text-center py-4">
        <h1 class="text-2xl font-bold tracking-tight text-foreground">欢迎使用 Pony Proxy</h1>
        <p class="text-sm text-muted-foreground mt-1">请选择适合您的加速连接方案，1 分钟内即可完成配置</p>
      </div>

      <div class="grid grid-cols-2 gap-4">
        <button
          @click="setupTab = 'direct'"
          :class="[
            'p-5 text-left rounded-xl border-2 transition-all flex flex-col justify-between',
            setupTab === 'direct'
              ? 'border-primary bg-primary/5 shadow-sm'
              : 'border-border bg-card hover:border-muted-foreground/30',
          ]"
        >
          <div class="flex items-center gap-3">
            <div class="p-2.5 rounded-lg bg-primary/10 text-primary">
              <Sparkles class="h-5 w-5" />
            </div>
            <div>
              <div class="font-semibold text-base">方案 A：个人独立加速 (推荐)</div>
              <div class="text-xs text-muted-foreground mt-0.5">本机自给自足，速度快、专属独立通道</div>
            </div>
          </div>
          <div class="mt-4 text-xs text-primary font-medium flex items-center gap-1">
            仅需一键授权出口 <CheckCircle2 class="h-3.5 w-3.5" />
          </div>
        </button>

        <button
          @click="setupTab = 'chained'"
          :class="[
            'p-5 text-left rounded-xl border-2 transition-all flex flex-col justify-between',
            setupTab === 'chained'
              ? 'border-primary bg-primary/5 shadow-sm'
              : 'border-border bg-card hover:border-muted-foreground/30',
          ]"
        >
          <div class="flex items-center gap-3">
            <div class="p-2.5 rounded-lg bg-blue-500/10 text-blue-600">
              <Server class="h-5 w-5" />
            </div>
            <div>
              <div class="font-semibold text-base">方案 B：连接远端代理 / 跨端导入</div>
              <div class="text-xs text-muted-foreground mt-0.5">连接自己的 Linux Server 或粘贴分享口令</div>
            </div>
          </div>
          <div class="mt-4 text-xs text-blue-600 font-medium flex items-center gap-1">
            支持一键粘贴连接口令 <Zap class="h-3.5 w-3.5" />
          </div>
        </button>
      </div>

      <!-- 方案 A 表单 -->
      <Card v-if="setupTab === 'direct'" class="border-border shadow-sm">
        <CardContent class="p-6 space-y-4">
          <div class="flex items-center justify-between">
            <Label class="text-sm font-medium">Cloudflare API Token 授权码</Label>
            <button
              type="button"
              @click="openExternalUrl('https://dash.cloudflare.com/profile/api-tokens')"
              class="text-xs text-primary hover:underline flex items-center gap-1 cursor-pointer bg-transparent border-0 p-0"
            >
              点击直达获取 Token <ExternalLink class="h-3 w-3" />
            </button>
          </div>
          <Input
            v-model="cfToken"
            type="password"
            placeholder="粘贴您的 Cloudflare API Token"
            class="font-mono text-sm"
          />
          <p class="text-xs text-muted-foreground">
            💡 提示：用于自动在云端部署个人加速节点，凭据将安全保存在本机 Windows 凭据管理器中，绝不上报。
          </p>
          <div class="pt-2">
            <Button
              @click="submitDirectSetup"
              :disabled="isSubmitting || !cfToken.trim()"
              class="w-full h-11 text-sm font-semibold"
            >
              <Zap v-if="!isSubmitting" class="h-4 w-4 mr-2" />
              <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
              {{ isSubmitting ? '正在初始化加速节点...' : '一键开启个人独立加速' }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- 方案 B 表单 -->
      <Card v-if="setupTab === 'chained'" class="border-border shadow-sm">
        <CardContent class="p-6 space-y-5">
          <div class="space-y-2">
            <Label class="text-sm font-medium">方式 1：粘贴一键连接口令 (最快捷)</Label>
            <Input
              v-model="syncUriInput"
              placeholder="粘贴 pproxy-sync:// 或 pproxy:// 口令"
              class="font-mono text-xs"
            />
            <p class="text-xs text-muted-foreground">
              可直接粘贴从 Linux Server（运行 <code>pproxy user add</code> 或 <code>pproxy sync export</code>）导出的口令。
            </p>
          </div>

          <div class="relative flex items-center py-2">
            <div class="flex-grow border-t border-border"></div>
            <span class="flex-shrink mx-4 text-xs text-muted-foreground uppercase">或者手动填写参数</span>
            <div class="flex-grow border-t border-border"></div>
          </div>

          <div class="grid grid-cols-2 gap-4">
            <div class="col-span-2 space-y-1.5">
              <Label class="text-xs">代理服务器地址 (如 192.168.1.100:8899)</Label>
              <Input v-model="remoteHost" placeholder="IP 或域名 : 端口" class="text-sm" />
            </div>
            <div class="space-y-1.5">
              <Label class="text-xs">用户名</Label>
              <Input v-model="remoteUser" placeholder="用户名" class="text-sm" />
            </div>
            <div class="space-y-1.5">
              <Label class="text-xs">密码</Label>
              <Input v-model="remotePass" type="password" placeholder="密码" class="text-sm" />
            </div>
          </div>

          <div class="pt-2">
            <Button
              @click="submitImportOrChained"
              :disabled="isSubmitting || (!syncUriInput.trim() && !remoteHost.trim())"
              class="w-full h-11 text-sm font-semibold"
            >
              <Server v-if="!isSubmitting" class="h-4 w-4 mr-2" />
              <RefreshCw v-else class="h-4 w-4 mr-2 animate-spin" />
              {{ isSubmitting ? '正在验证连接...' : '连接远端代理并开启' }}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>

    <!-- 已配置：极简傻瓜主界面 -->
    <div v-else class="space-y-6">
      <!-- 核心大开关卡片 -->
      <Card class="overflow-hidden border-border shadow-sm">
        <div
          :class="[
            'p-8 flex items-center justify-between transition-colors',
            isRunning ? 'bg-emerald-500/10' : 'bg-muted/40',
          ]"
        >
          <div class="space-y-1">
            <div class="flex items-center gap-2.5">
              <span
                :class="[
                  'h-3.5 w-3.5 rounded-full inline-block animate-pulse',
                  isRunning ? 'bg-emerald-500 shadow-sm shadow-emerald-500/50' : 'bg-muted-foreground/40',
                ]"
              ></span>
              <h2 class="text-2xl font-bold tracking-tight">
                {{ isRunning ? '智能加速已开启' : '加速已停止' }}
              </h2>
            </div>
            <p class="text-sm text-muted-foreground">
              {{
                isRunning
                  ? proxyMode === 'whitelist'
                    ? '🎯 智能分流中：国内网络直连，海外 AI 服务极速出网'
                    : '🌐 全局加速中：全部网络流量已接管加速'
                  : '点击右侧按钮即可一键恢复加速'
              }}
            </p>
          </div>

          <Button
            @click="toggleProxy"
            :class="[
              'h-16 px-8 rounded-2xl text-base font-bold transition-all shadow-md flex items-center gap-3',
              isRunning
                ? 'bg-emerald-600 hover:bg-emerald-700 text-white shadow-emerald-600/25'
                : 'bg-primary hover:bg-primary/90 text-primary-foreground',
            ]"
          >
            <Power class="h-6 w-6" />
            {{ isRunning ? '已开启' : '一键开启' }}
          </Button>
        </div>

        <!-- 模式切换与急救栏 -->
        <div class="bg-card px-8 py-4 border-t border-border flex items-center justify-between">
          <div class="flex items-center gap-2">
            <span class="text-xs font-medium text-muted-foreground mr-1">加速模式:</span>
            <button
              @click="setProxyMode('whitelist')"
              :class="[
                'px-3 py-1.5 rounded-lg text-xs font-medium transition-all',
                proxyMode === 'whitelist'
                  ? 'bg-primary text-primary-foreground shadow-sm'
                  : 'bg-muted hover:bg-muted/80 text-muted-foreground',
              ]"
            >
              智能分流 (推荐)
            </button>
            <button
              @click="setProxyMode('global')"
              :class="[
                'px-3 py-1.5 rounded-lg text-xs font-medium transition-all',
                proxyMode === 'global'
                  ? 'bg-primary text-primary-foreground shadow-sm'
                  : 'bg-muted hover:bg-muted/80 text-muted-foreground',
              ]"
            >
              全局加速
            </button>
          </div>

          <!-- 网络急救箱 -->
          <button
            @click="triggerRescue"
            title="如果电脑无法上网或代理异常，点击此按钮可一键恢复网络直连"
            class="text-xs text-amber-600 hover:text-amber-700 hover:bg-amber-500/10 px-3 py-1.5 rounded-lg transition-colors flex items-center gap-1.5 font-medium"
          >
            <LifeBuoy class="h-4 w-4" />
            网络急救箱 (一键还原)
          </button>
        </div>
      </Card>

      <!-- AI 常用服务实时状态卡片 -->
      <Card class="border-border shadow-sm">
        <CardContent class="p-6 space-y-4">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <ShieldCheck class="h-5 w-5 text-primary" />
              <h3 class="font-semibold text-sm">常用 AI 与海外服务实时连通性</h3>
            </div>
            <Button variant="ghost" size="sm" @click="runSiteTests" class="h-8 text-xs gap-1">
              <RefreshCw class="h-3.5 w-3.5" />
              重新测速
            </Button>
          </div>

          <div class="grid grid-cols-2 gap-3 pt-1">
            <div
              v-for="site in testResults"
              :key="site.name"
              class="p-3.5 rounded-xl border border-border/70 bg-card flex items-center justify-between"
            >
              <div class="space-y-0.5">
                <div class="text-sm font-medium">{{ site.name }}</div>
                <div class="text-xs text-muted-foreground">{{ site.host }}</div>
              </div>
              <div class="flex items-center gap-2">
                <span
                  v-if="site.status === 'ok'"
                  class="text-xs font-mono font-medium px-2 py-0.5 rounded-full bg-emerald-500/10 text-emerald-600 flex items-center gap-1"
                >
                  <span class="h-1.5 w-1.5 rounded-full bg-emerald-500 inline-block"></span>
                  {{ site.latency }}ms
                </span>
                <span
                  v-else-if="site.status === 'testing'"
                  class="text-xs text-muted-foreground animate-pulse"
                >
                  测速中...
                </span>
                <span
                  v-else-if="site.status === 'fail'"
                  class="text-xs font-medium px-2 py-0.5 rounded-full bg-rose-500/10 text-rose-600"
                >
                  无法连接
                </span>
                <span v-else class="text-xs text-muted-foreground">—</span>
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
