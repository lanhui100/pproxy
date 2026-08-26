<script setup lang="ts">
// Proxy 页（M6 spec §4）：白名单 CRUD + 系统代理总开关。
// 纯 Tauri invoke；browser dev 下功能禁用（按钮仍可见但不发请求）。
import { computed, onMounted, ref } from 'vue'

import { Loader2 } from '@lucide/vue'

import EmptyState from '@/components/common/EmptyState.vue'
import InfoTip from '@/components/common/InfoTip.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'

import { Switch } from '@/components/ui/switch'

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}
async function tauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error('浏览器预览模式下不可用，请在桌面应用内使用')
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

const entries = ref<string[]>([])
const newEntry = ref('')
const enabled = ref(false)
const toggling = ref(false)
const error = ref('')
interface SiteResult { site: string; ok: boolean; ms: number; error: string }
const siteResults = ref<SiteResult[]>([])
const testing = ref(false)

async function refresh(): Promise<void> {
  try {
    if (!isTauri()) return
    entries.value = await tauri<string[]>('proxy_whitelist_get')
    error.value = ''
  } catch (e) {
    error.value = String(e)
  }
}

async function addEntry(): Promise<void> {
  const v = newEntry.value.trim().replace(/\.$/, '').toLowerCase()
  if (!v || entries.value.includes(v)) return
  const next = [...entries.value, v]
  try {
    await tauri('proxy_whitelist_set', { entries: next })
    entries.value = next
    newEntry.value = ''
  } catch (e) {
    error.value = String(e)
  }
}

async function removeEntry(i: number): Promise<void> {
  const next = entries.value.filter((_, idx) => idx !== i)
  try {
    await tauri('proxy_whitelist_set', { entries: next })
    entries.value = next
  } catch (e) {
    error.value = String(e)
  }
}

async function toggleProxy(on: boolean): Promise<void> {
  error.value = ''
  toggling.value = true
  try {
    await tauri(on ? 'proxy_enable' : 'proxy_disable')
    const st = await tauri<{ engine_running: boolean }>('proxy_status')
    enabled.value = st.engine_running // 以后端真实状态为准
  } catch (e) {
    error.value = errorMessage(e)
    enabled.value = false
  } finally {
    toggling.value = false
  }
}

async function testSites(): Promise<void> {
  testing.value = true
  try {
    siteResults.value = await tauri<SiteResult[]>('proxy_test_sites')
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    testing.value = false
  }
}

function errorMessage(e: unknown): string {
  return String(e).replace(/^"|"$/g, '')
}

const inApp = computed(() => isTauri())

onMounted(refresh)
</script>

<template>
  <div class="max-w-2xl">
    <PageHeader title="代理加速" subtitle="让常用网站走加速通道，其余流量保持原样">
      <template #actions>
        <div class="flex items-center gap-2">
          <Switch :model-value="enabled" :disabled="toggling" @update:model-value="toggleProxy" />
          <span v-if="toggling"><Loader2 class="size-4 animate-spin text-muted-foreground" /></span>
          <span v-else class="text-sm font-medium">{{ enabled ? '已开启' : '已关闭' }}</span>
        </div>
      </template>
    </PageHeader>

    <p v-if="error" class="mb-6 rounded-lg bg-bad-soft px-3 py-2 text-sm break-all text-bad">{{ error }}</p>

    <p v-if="!inApp" class="mb-6 rounded-lg bg-warn-soft px-3 py-2 text-sm text-warn">
      浏览器预览下看不到真实状态，请在桌面应用里使用本页
    </p>

    <!-- 连通性体检 -->
    <Card>
      <CardHeader>
        <CardTitle class="text-sm">连通性体检</CardTitle>
        <CardDescription>检测几个常用网站当前能不能正常访问。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-3">
        <Button variant="outline" size="sm" :disabled="!enabled || testing" @click="testSites">
          {{ testing ? '正在测试…' : '测试常用站点' }}
        </Button>
        <p v-if="!enabled" class="text-xs leading-5 text-muted-foreground">先打开上面的开关再测</p>
        <ul v-if="siteResults.length" class="space-y-1.5 pt-1">
          <li v-for="r in siteResults" :key="r.site" class="flex items-center gap-2 text-sm">
            <StatusDot tone="ok" :label="`${r.site} · ${r.ms}ms`" />
            <span v-if="!r.ok" class="min-w-0 flex-1 truncate text-xs text-muted-foreground">{{ r.error }}</span>
          </li>
        </ul>
      </CardContent>
    </Card>

    <!-- 直连名单 -->
    <Card class="mt-4">
      <CardHeader>
        <CardTitle class="flex items-center gap-1 text-sm">
          直连名单
          <InfoTip text="名单里的网站不走加速通道，直接连接。填 example.com 会连同它的所有子域一起匹配" />
        </CardTitle>
        <CardDescription>这些网站的流量不经加速，适合国内本来就能访问的站点。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <div class="flex gap-2">
          <Input
            v-model="newEntry"
            placeholder="输入域名，如 example.com"
            class="bg-muted/70"
            @keyup.enter="addEntry"
          />
          <Button :disabled="!newEntry.trim()" @click="addEntry">添加</Button>
        </div>
        <EmptyState
          v-if="entries.length === 0"
          title="名单是空的"
          description="不需要加速的网站可以加到这里，例如公司内网或国内站点。"
        />
        <template v-else>
          <!-- 标签流：条目多时比表格更易扫读，点 × 即移除 -->
          <div class="flex flex-wrap gap-2">
            <span
              v-for="(e, i) in entries"
              :key="e"
              class="inline-flex items-center gap-1 rounded-full bg-muted/70 py-1 pr-1.5 pl-3 text-sm"
            >
              {{ e }}
              <button
                type="button"
                class="rounded-full p-0.5 leading-none text-muted-foreground/70 transition-colors hover:text-bad"
                :aria-label="`移除 ${e}`"
                @click="removeEntry(i)"
              >
                ×
              </button>
            </span>
          </div>
        </template>
      </CardContent>
    </Card>
  </div>
</template>
