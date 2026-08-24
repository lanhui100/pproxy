<script setup lang="ts">
// 服务路由（M7 SPEC §3.6）：两段式添加（模板网格 + 手动高级折叠）+ 列表
// （上游徽标 / Switch 行内切换 / 连通性测速与失败态线路切换）+ 删除确认。
// 模板常量取 lib/serviceTemplates.ts（target_host 已逐条 web 核验，见该文件注释）。
import { onMounted, ref } from 'vue'

import { LoaderCircle } from '@lucide/vue'

import { api, type RouteDto } from '@/api/client'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useToast } from '@/composables/useToast'
import { errText } from '@/lib/errors'
import { SERVICE_TEMPLATES } from '@/lib/serviceTemplates'
import { upstreamLabel } from '@/lib/statusLabels'

const toast = useToast()

// 上游线路下拉/徽标的中文文案（经 statusLabels 唯一出口，避免模板直出英文）
const LINE_LABELS = {
  worker: upstreamLabel('worker').label,
  vercel: upstreamLabel('vercel').label,
}

// ---- 列表状态 ----
const routes = ref<RouteDto[]>([])
const loading = ref(false)
const loadError = ref('')

async function refresh(): Promise<void> {
  loading.value = true
  loadError.value = ''
  try {
    routes.value = (await api.listRoutes()).routes
  } catch (e) {
    loadError.value = errText(e)
  } finally {
    loading.value = false
  }
}

onMounted(refresh)

// ---- 添加对话框（两段式）----
const showAdd = ref(false)
const adding = ref(false)
const addName = ref('')
const addHost = ref('')
const addUpstream = ref('') // ''=自动决策（缺省，不携带 override_upstream 键）
const addError = ref('')
const selectedTemplate = ref<string | null>(null)

function openAdd(): void {
  addError.value = ''
  resetAddForm()
  showAdd.value = true
}

/** 添加中忽略 Esc/遮罩关闭（UX-7③）：busy 态下 @update:open 的 false 直接忽略。 */
function onAddOpenChange(v: boolean): void {
  if (!v && adding.value) return
  showAdd.value = v
}

function resetAddForm(): void {
  addName.value = ''
  addHost.value = ''
  addUpstream.value = ''
  selectedTemplate.value = null
}

/**
 * 点击模板（UX-9）：仅当名称与 host 均为空时才自动填充；
 * 已有手动输入时不得覆盖，只提示保留。
 */
function pickTemplate(name: string): void {
  const t = SERVICE_TEMPLATES.find((x) => x.name === name)
  if (!t) return
  if (addName.value.trim() || addHost.value.trim()) {
    // 不点亮选中态：模板并未生效，避免「看似已选、表单却是另一套」的误导
    toast.info('已保留你的手动输入，模板未覆盖')
    return
  }
  selectedTemplate.value = t.name
  addName.value = t.name
  addHost.value = t.target_host
  addUpstream.value = '' // 模板一律走自动决策（与服务端 VERCEL_HOSTS 规则一致）
}

async function addRoute(): Promise<void> {
  addError.value = ''
  adding.value = true
  try {
    await api.createRoute({
      name: addName.value,
      target_host: addHost.value,
      ...(addUpstream.value ? { override_upstream: addUpstream.value } : {}),
    })
    showAdd.value = false
    resetAddForm()
    await refresh()
    toast.success('服务已添加')
  } catch (e) {
    // 服务端 SSRF/名称校验文案经 errText 译中文；Dialog 不关、表单不清
    addError.value = errText(e)
  } finally {
    adding.value = false
  }
}

// ---- 状态开关（行内 spinner + 成功后行内更新）----
const togglingName = ref('')

async function toggle(r: RouteDto): Promise<void> {
  togglingName.value = r.name
  try {
    // 三态语义：仅动 enabled，override_upstream 字段整体缺席=不改（F17 分支之一）
    const resp = await api.patchRoute(r.name, { enabled: !r.enabled })
    r.enabled = resp.enabled
  } catch (e) {
    // 失败统一 toast（UX-5 / SPEC §2-5）；行内开关由 resp 未更新自然回弹
    toast.error(errText(e))
  } finally {
    togglingName.value = ''
  }
}

// ---- 连通性测速 ----
type TestState =
  | { phase: 'untested' }
  | { phase: 'testing' }
  | { phase: 'ok'; latency: number | null }
  | { phase: 'fail'; message: string }

const tests = ref<Record<string, TestState>>({})
const switchingName = ref('') // 失败态切换线路进行中

function testOf(name: string): TestState {
  return tests.value[name] ?? { phase: 'untested' }
}

/** 模板展示用收窄助手（vue-tsc 不跨调用保留联合类型收窄）。 */
function okLatency(name: string): number | null {
  const t = testOf(name)
  return t.phase === 'ok' ? t.latency : null
}

function failMessage(name: string): string {
  const t = testOf(name)
  return t.phase === 'fail' ? t.message : ''
}

async function runTest(r: RouteDto): Promise<void> {
  tests.value[r.name] = { phase: 'testing' }
  try {
    const t = await api.testRoute(r.name, { skipAuthRedirect: true })
    tests.value[r.name] = t.ok
      ? { phase: 'ok', latency: t.latency_ms ?? null }
      : { phase: 'fail', message: errText(t.error ?? '') || '未知原因' }
  } catch (e) {
    tests.value[r.name] = { phase: 'fail', message: errText(e) }
  }
}

/** 测速按钮入口。 */
function onTestClick(r: RouteDto): void {
  void runTest(r)
}

/**
 * 失败态切换线路：PATCH override_upstream → 刷新行 → 自动重测一次。
 * 自动决策语义下 override 即 effective，行内直接同步展示值。
 */
async function switchLine(r: RouteDto, upstream: string): Promise<void> {
  switchingName.value = r.name
  tests.value[r.name] = { phase: 'testing' }
  try {
    await api.patchRoute(r.name, { override_upstream: upstream })
    r.override_upstream = upstream
    r.effective_upstream = upstream
    const fresh = routes.value.find((x) => x.name === r.name) ?? r
    await runTest(fresh)
    toast.success(`已切换至${upstream === 'worker' ? LINE_LABELS.worker : LINE_LABELS.vercel}，正在重测`)
  } catch (e) {
    // 失败统一 toast（UX-5）；连通性列同步落失败态，保留切换线路/重测入口
    toast.error(errText(e))
    tests.value[r.name] = { phase: 'fail', message: errText(e) }
  } finally {
    switchingName.value = ''
  }
}

/** 线路选择变化（仅失败态的下拉会触发）。 */
function onLineChange(r: RouteDto, ev: Event): void {
  const v = (ev.target as HTMLSelectElement).value
  ;(ev.target as HTMLSelectElement).value = '' // 用后归位，便于再次选择同一线路
  if (v) void switchLine(r, v)
}

// ---- 删除确认 ----
const deleteTarget = ref<RouteDto | null>(null)
const deleting = ref(false)

function askDelete(r: RouteDto): void {
  deleteTarget.value = r
}

async function doDelete(): Promise<void> {
  if (!deleteTarget.value) return
  deleting.value = true
  try {
    await api.deleteRoute(deleteTarget.value.name)
    deleteTarget.value = null
    await refresh()
  } catch (e) {
    // 失败统一 toast（UX-5 / SPEC §2-5）；Dialog 已随 deleteTarget 置空关闭
    toast.error(errText(e))
    deleteTarget.value = null
  } finally {
    deleting.value = false
  }
}
</script>

<template>
  <div>
    <PageHeader title="服务" subtitle="一键添加常用服务，网关自动选择上游出口">
      <template #actions>
        <Button @click="openAdd">添加服务</Button>
      </template>
    </PageHeader>

    <!-- 加载骨架 -->
    <SkeletonTable v-if="loading && routes.length === 0" :rows="6" />

    <!-- 加载失败（可重试） -->
    <div v-else-if="loadError" class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">
      {{ loadError }}
      <Button variant="outline" size="sm" class="ml-2" @click="refresh">重试</Button>
    </div>

    <!-- 空态（CTA 下一步）：EmptyState 仅渲染具名插槽 #actions -->
    <EmptyState
      v-else-if="routes.length === 0"
      title="还没有服务路由"
      description="从常用服务模板一键添加，或手动填入任意目标域名。"
    >
      <template #actions>
        <Button @click="openAdd">添加服务</Button>
      </template>
    </EmptyState>

    <!-- 列表 -->
    <div v-else class="overflow-hidden rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>名称</TableHead>
            <TableHead>目标</TableHead>
            <TableHead>上游</TableHead>
            <TableHead>状态</TableHead>
            <TableHead>连通性</TableHead>
            <TableHead class="text-right">操作</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="r in routes" :key="r.name">
            <TableCell class="font-medium">{{ r.name }}</TableCell>
            <TableCell class="font-mono text-xs">{{ r.target_host }}</TableCell>
            <TableCell>
              <Badge variant="secondary">{{ upstreamLabel(r.effective_upstream).label }}</Badge>
              <span v-if="r.override_upstream" class="ml-1 align-middle text-xs text-muted-foreground">手动指定</span>
            </TableCell>
            <TableCell>
              <div class="flex items-center gap-1.5">
                <Switch :checked="r.enabled" :disabled="togglingName === r.name" @update:checked="toggle(r)" />
                <LoaderCircle v-if="togglingName === r.name" class="size-3 animate-spin text-muted-foreground" />
              </div>
            </TableCell>
            <TableCell>
              <!-- 未测：灰字 -->
              <span v-if="testOf(r.name).phase === 'untested'" class="text-xs text-muted-foreground">未测</span>
              <!-- 测速中 -->
              <span v-else-if="testOf(r.name).phase === 'testing'" class="text-xs text-muted-foreground">测速中…</span>
              <!-- 正常：绿点 -->
              <StatusDot
                v-else-if="testOf(r.name).phase === 'ok'"
                tone="ok"
                :label="`正常 · ${okLatency(r.name) ?? '?'}ms`"
              />
              <!-- 失败：红点 + 内联动作（切换线路 ▾ / 重测） -->
              <div v-else class="space-y-1">
                <StatusDot tone="error" :label="`失败：${failMessage(r.name)}`" />
                <div class="flex items-center gap-1.5">
                  <select
                    class="h-6 rounded-md border bg-background px-1.5 text-xs outline-none"
                    :disabled="switchingName === r.name"
                    aria-label="切换线路"
                    @change="onLineChange(r, $event)"
                  >
                    <option value="" disabled selected>切换线路 ▾</option>
                    <option value="worker">{{ LINE_LABELS.worker }}</option>
                    <option value="vercel">{{ LINE_LABELS.vercel }}</option>
                  </select>
                  <button
                    class="text-xs underline underline-offset-2 disabled:opacity-50"
                    :disabled="switchingName === r.name || testOf(r.name).phase === 'testing'"
                    @click="onTestClick(r)"
                  >
                    重测
                  </button>
                </div>
              </div>
            </TableCell>
            <TableCell class="space-x-1 text-right">
              <Button
                variant="outline"
                size="sm"
                :disabled="testOf(r.name).phase === 'testing' || switchingName === r.name"
                @click="onTestClick(r)"
              >
                {{ testOf(r.name).phase === 'testing' ? '测速中…' : '测速' }}
              </Button>
              <Button variant="destructive" size="sm" @click="askDelete(r)">删除</Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <!-- 添加对话框（两段式）：错误在内部渲染，失败不关不清；busy 中 Esc/遮罩不可关（UX-7③） -->
    <Dialog :open="showAdd" @update:open="onAddOpenChange">
      <DialogContent class="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>添加服务</DialogTitle>
        </DialogHeader>

        <div class="space-y-4">
          <!-- 上段：模板网格 -->
          <div>
            <Label>常用服务</Label>
            <div class="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-3">
              <button
                v-for="tpl in SERVICE_TEMPLATES"
                :key="tpl.name"
                type="button"
                class="rounded-lg border px-3 py-2 text-left transition-colors"
                :class="
                  selectedTemplate === tpl.name
                    ? 'border-primary bg-primary/5 ring-1 ring-primary'
                    : 'hover:bg-muted'
                "
                @click="pickTemplate(tpl.name)"
              >
                <span class="block text-sm font-medium">{{ tpl.label }}</span>
                <span class="block truncate font-mono text-xs text-muted-foreground">{{ tpl.target_host }}</span>
              </button>
            </div>
          </div>

          <!-- 下段：手动添加（高级）折叠 -->
          <details class="rounded-md border px-3 py-2">
            <summary class="cursor-pointer text-sm font-medium">手动添加（高级）</summary>
            <div class="mt-3 space-y-3">
              <div class="space-y-1">
                <Label for="route-name">名称</Label>
                <Input id="route-name" v-model="addName" placeholder="如 gemini" />
              </div>
              <div class="space-y-1">
                <Label for="route-host">目标域名</Label>
                <Input id="route-host" v-model="addHost" placeholder="generativelanguage.googleapis.com" />
                <p class="text-xs text-muted-foreground">只填域名本身，不带 https:// 与路径</p>
              </div>
              <div class="space-y-1">
                <Label for="route-upstream">上游线路</Label>
                <select
                  id="route-upstream"
                  v-model="addUpstream"
                  class="h-8 w-full rounded-lg border bg-background px-2 text-sm outline-none"
                >
                  <option value="">自动决策（缺省）</option>
                  <option value="worker">{{ LINE_LABELS.worker }}</option>
                  <option value="vercel">{{ LINE_LABELS.vercel }}</option>
                </select>
                <p class="text-xs text-muted-foreground">自动决策按服务端规则选择出口，无需手动干预</p>
              </div>
            </div>
          </details>

          <p v-if="addError" class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ addError }}</p>
        </div>

        <DialogFooter>
          <!-- UX-8：禁用不静默，灰字说明缺什么 -->
          <span v-if="!addName.trim() || !addHost.trim()" class="mr-auto self-center text-xs text-muted-foreground">需填写名称与目标域名后可创建</span>
          <Button variant="outline" :disabled="adding" @click="showAdd = false">取消</Button>
          <Button :disabled="adding || !addName.trim() || !addHost.trim()" data-icon="inline-start" @click="addRoute">
            <LoaderCircle v-if="adding" class="animate-spin" />
            创建
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 删除确认 -->
    <ConfirmDialog
      :open="deleteTarget !== null"
      :title="`删除服务「${deleteTarget?.name ?? ''}」？`"
      :description="`目标 ${deleteTarget?.target_host ?? ''} 将立即从网关移除，使用中的客户端会无法访问该服务。`"
      confirm-text="确认删除"
      destructive
      :busy="deleting"
      @update:open="(v: boolean) => !v && (deleteTarget = null)"
      @confirm="doDelete"
    />
  </div>
</template>
