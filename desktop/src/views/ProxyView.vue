<script setup lang="ts">
// Proxy 页（M6 spec §4）：白名单 CRUD + 系统代理总开关
import { onMounted, ref } from 'vue'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'

import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}
async function tauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error('browser dev 模式下代理功能需在应用内使用')
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

const entries = ref<string[]>([])
const newEntry = ref('')
const enabled = ref(false)
const error = ref('')

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
  try {
    await tauri(on ? 'proxy_enable' : 'proxy_disable')
    enabled.value = on
  } catch (e) {
    error.value = errorMessage(e)
  }
}

function errorMessage(e: unknown): string {
  return String(e).replace(/^"|"$/g, '')
}

onMounted(refresh)
</script>

<template>
  <div class="max-w-2xl space-y-6">
    <h1 class="text-xl font-semibold">Proxy</h1>

    <p v-if="error" class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ error }}</p>

    <Card>
      <CardHeader><CardTitle class="text-sm">系统代理总开关（PAC 模式）</CardTitle></CardHeader>
      <CardContent class="flex items-center gap-3">
        <Switch :checked="enabled" @update:checked="toggleProxy" />
        <span class="text-sm text-muted-foreground">{{ enabled ? '已启用：白名单流量经隧道，其余直连' : '未启用' }}</span>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle class="text-sm">白名单（域名后缀匹配）</CardTitle></CardHeader>
      <CardContent class="space-y-3">
        <div class="flex gap-2">
          <Input v-model="newEntry" placeholder="如 example.com（含全部子域）" @keyup.enter="addEntry" />
          <Button :disabled="!newEntry.trim()" @click="addEntry">添加</Button>
        </div>
        <Table>
          <TableHeader>
            <TableRow><TableHead>域名</TableHead><TableHead class="text-right">操作</TableHead></TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="(e, i) in entries" :key="e">
              <TableCell>{{ e }}</TableCell>
              <TableCell class="text-right">
                <Button variant="destructive" size="sm" @click="removeEntry(i)">移除</Button>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  </div>
</template>
