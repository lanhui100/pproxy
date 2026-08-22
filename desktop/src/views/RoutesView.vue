<script setup lang="ts">
// Routes（spec §4）：列表 + 添加 + 行内 test + enable/disable（PATCH 三态）+ 删除确认
import { onMounted, ref } from 'vue'

import { api, errorMessage, type RouteDto } from '@/api/client'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

const routes = ref<RouteDto[]>([])
const error = ref('')
const loading = ref(false)

// 添加表单
const showAdd = ref(false)
const addName = ref('')
const addHost = ref('')
const addOverride = ref('')

// 删除确认
const deleteTarget = ref<RouteDto | null>(null)

// 行内 test 结果：name → 展示串
const testResults = ref<Record<string, string>>({})
const testingName = ref('')
const togglingName = ref('')

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    routes.value = (await api.listRoutes()).routes
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

async function addRoute(): Promise<void> {
  error.value = ''
  try {
    await api.createRoute({
      name: addName.value,
      target_host: addHost.value,
      ...(addOverride.value ? { override_upstream: addOverride.value } : {}),
    })
    showAdd.value = false
    addName.value = ''
    addHost.value = ''
    addOverride.value = ''
    await refresh()
  } catch (e) {
    error.value = errorMessage(e) // 服务端 SSRF/名称校验文案原样透出
  }
}

async function toggle(r: RouteDto): Promise<void> {
  togglingName.value = r.name
  error.value = ''
  try {
    // 三态语义：仅动 enabled，override_upstream 字段整体缺席=不改（F17 分支之一）
    const resp = await api.patchRoute(r.name, { enabled: !r.enabled })
    r.enabled = resp.enabled
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    togglingName.value = ''
  }
}

async function testRoute(r: RouteDto): Promise<void> {
  testingName.value = r.name
  try {
    const t = await api.testRoute(r.name, { skipAuthRedirect: true })
    testResults.value[r.name] = t.ok ? `✓ ${t.latency_ms ?? '?'}ms` : `✗ ${t.error ?? 'failed'}`
  } catch (e) {
    testResults.value[r.name] = `✗ ${errorMessage(e)}`
  } finally {
    testingName.value = ''
  }
}

async function doDelete(): Promise<void> {
  if (!deleteTarget.value) return
  error.value = ''
  try {
    await api.deleteRoute(deleteTarget.value.name)
    deleteTarget.value = null
    await refresh()
  } catch (e) {
    error.value = errorMessage(e)
    deleteTarget.value = null
  }
}

onMounted(refresh)
</script>

<template>
  <div>
    <div class="mb-4 flex items-center justify-between">
      <h1 class="text-xl font-semibold">Routes</h1>
      <Button @click="showAdd = true">添加路由</Button>
    </div>

    <p v-if="error" class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ error }}</p>

    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>名称</TableHead>
          <TableHead>目标 host</TableHead>
          <TableHead>上游决策</TableHead>
          <TableHead>启用</TableHead>
          <TableHead>连通性</TableHead>
          <TableHead class="text-right">操作</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="r in routes" :key="r.name">
          <TableCell class="font-medium">{{ r.name }}</TableCell>
          <TableCell>{{ r.target_host }}</TableCell>
          <TableCell>
            <Badge variant="secondary">{{ r.effective_upstream }}</Badge>
            <span v-if="r.override_upstream" class="ml-1 text-xs text-muted-foreground">(override)</span>
          </TableCell>
          <TableCell>
            <Switch
              :checked="r.enabled"
              :disabled="togglingName === r.name"
              @update:checked="toggle(r)"
            />
          </TableCell>
          <TableCell class="text-xs">{{ testResults[r.name] ?? '—' }}</TableCell>
          <TableCell class="space-x-1 text-right">
            <Button variant="outline" size="sm" :disabled="testingName === r.name" @click="testRoute(r)">
              {{ testingName === r.name ? '测试中…' : 'test' }}
            </Button>
            <Button variant="destructive" size="sm" @click="deleteTarget = r">删除</Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <!-- 添加对话框 -->
    <Dialog :open="showAdd" @update:open="showAdd = $event">
      <DialogContent class="max-w-md">
        <DialogHeader>
          <DialogTitle>添加路由</DialogTitle>
        </DialogHeader>
        <div class="space-y-3">
          <div class="space-y-1">
            <Label for="route-name">名称</Label>
            <Input id="route-name" v-model="addName" placeholder="如 gemini" />
          </div>
          <div class="space-y-1">
            <Label for="route-host">目标 host</Label>
            <Input id="route-host" v-model="addHost" placeholder="generativelanguage.googleapis.com" />
          </div>
          <div class="space-y-1">
            <Label for="route-override">override 上游（可选：worker / vercel）</Label>
            <Input id="route-override" v-model="addOverride" placeholder="缺省自动选择" />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="showAdd = false">取消</Button>
          <Button :disabled="!addName || !addHost" @click="addRoute">创建</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 删除确认 -->
    <Dialog :open="deleteTarget !== null" @update:open="deleteTarget = null">
      <DialogContent class="max-w-sm">
        <DialogHeader>
          <DialogTitle>删除路由「{{ deleteTarget?.name }}」？</DialogTitle>
        </DialogHeader>
        <p class="text-sm text-muted-foreground">目标 host：{{ deleteTarget?.target_host }}。删除即时生效。</p>
        <DialogFooter>
          <Button variant="outline" @click="deleteTarget = null">取消</Button>
          <Button variant="destructive" @click="doDelete">确认删除</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
