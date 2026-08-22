<script setup lang="ts">
// Settings（spec §4/§6）：后端地址 + admin token（凭据库）+ 连接测试 +
// 服务端监控配置只读展示。401 在此页豁免全局跳转（防死循环）。
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { api, errorMessage, isUnauthorized, setBaseUrlProvider, setTokenProvider, type MonitorConfigResp } from '@/api/client'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

import {
  clearAdminToken,
  isTauri,
  loadBackendUrl,
  loadPollIntervalMin,
  saveAdminToken,
  saveBackendUrl,
  savePollIntervalMin,
} from '@/lib/config'

const router = useRouter()

const url = ref(loadBackendUrl())
const tokenInput = ref('')
const pollMin = ref(loadPollIntervalMin())
const saved = ref(false)
const testResult = ref('')
const testing = ref(false)
const monitorConfig = ref<MonitorConfigResp | null>(null)
const monitorError = ref('')

onMounted(async () => {
  // 装配 client 提供者：地址/token 即时生效（其余页面随后请求即走新配置）
  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => (await tokenFromStore()) ?? (tokenInput.value || null))
  void refreshMonitorConfig()
})

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

/** 连接测试：skipAuthRedirect 豁免全局跳转；失败文案三分支（F10）。 */
async function testConnection(): Promise<void> {
  testing.value = true
  testResult.value = ''
  const prevUrl = url.value
  saveBackendUrl(prevUrl)
  setBaseUrlProvider(() => prevUrl)
  await saveAdminToken(tokenInput.value)
  setTokenProvider(async () => (tokenInput.value || null))
  try {
    const h = await api.health({ skipAuthRedirect: true })
    testResult.value = `✓ 连接成功（status=${h.status}, db=${h.db}）`
  } catch (e) {
    if (isUnauthorized(e)) testResult.value = '✗ 已连通但鉴权失败：请检查 admin token'
    else if ((e as { kind?: string }).kind === 'network') testResult.value = '✗ 无法连接：地址不可达或服务未运行'
    else testResult.value = `✗ ${errorMessage(e)}`
  } finally {
    testing.value = false
  }
}

async function save(): Promise<void> {
  saved.value = false
  saveBackendUrl(url.value)
  if (tokenInput.value) await saveAdminToken(tokenInput.value)
  savePollIntervalMin(pollMin.value)
  setBaseUrlProvider(() => url.value)
  setTokenProvider(async () => tokenInput.value || null)
  saved.value = true
  setTimeout(() => (saved.value = false), 2000)
}

async function refreshMonitorConfig(): Promise<void> {
  monitorError.value = ''
  try {
    monitorConfig.value = await api.monitorConfig()
  } catch (e) {
    monitorError.value = errorMessage(e)
    if (isUnauthorized(e)) router.push('/settings') // 本页自身：仅刷新展示
  }
}

async function forgetToken(): Promise<void> {
  await clearAdminToken()
  tokenInput.value = ''
  testResult.value = '已清除本地凭据'
}
</script>

<template>
  <div class="max-w-xl space-y-6">
    <h1 class="text-xl font-semibold">Settings</h1>

    <Card>
      <CardHeader><CardTitle class="text-sm">后端连接</CardTitle></CardHeader>
      <CardContent class="space-y-3">
        <div class="space-y-1">
          <Label for="cfg-url">管理面地址</Label>
          <Input id="cfg-url" v-model="url" placeholder="http://<TAILNET_IP>:8900" />
        </div>
        <div class="space-y-1">
          <Label for="cfg-token">admin token</Label>
          <Input id="cfg-token" v-model="tokenInput" type="password" placeholder="留空 = 沿用已保存凭据" />
          <p class="text-xs text-muted-foreground">
            存储位置：{{ isTauri() ? 'Windows 凭据管理器' : '浏览器 localStorage（dev）' }}
            · <button class="underline" @click="forgetToken">清除</button>
          </p>
        </div>
        <div class="flex gap-2">
          <Button :disabled="testing" @click="testConnection">{{ testing ? '测试中…' : '测试连接' }}</Button>
          <Button variant="outline" :disabled="!url" @click="save">保存</Button>
        </div>
        <p v-if="testResult" class="text-sm">{{ testResult }}</p>
        <p v-if="saved" class="text-sm text-emerald-700">已保存 ✓</p>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle class="text-sm">告警通知</CardTitle></CardHeader>
      <CardContent class="space-y-3">
        <div class="space-y-1">
          <Label for="cfg-poll">轮询间隔（分钟）</Label>
          <Input id="cfg-poll" v-model.number="pollMin" type="number" min="1" class="w-32" />
        </div>
        <Button variant="outline" @click="save">保存</Button>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle class="text-sm">服务端监控配置（只读）</CardTitle></CardHeader>
      <CardContent>
        <p v-if="monitorError" class="text-sm text-muted-foreground">{{ monitorError }}</p>
        <dl v-else-if="monitorConfig" class="space-y-1 text-sm">
          <div class="flex justify-between"><dt>告警阈值</dt><dd>{{ monitorConfig.threshold_pct }}%</dd></div>
          <div class="flex justify-between"><dt>配额轮询间隔</dt><dd>{{ monitorConfig.poll_interval_sec }}s</dd></div>
        </dl>
        <p v-else class="text-sm text-muted-foreground">加载中…</p>
      </CardContent>
    </Card>
  </div>
</template>
