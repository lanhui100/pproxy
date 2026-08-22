<script setup lang="ts">
// Tokens（spec §4）：列表 + 创建（明文一次性展示/复制/60s 剪贴板自清）+ 撤销确认
import { onMounted, ref } from 'vue'

import { api, errorMessage, type TokenDto } from '@/api/client'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

const tokens = ref<TokenDto[]>([])
const error = ref('')
const loading = ref(false)

const showAdd = ref(false)
const addName = ref('')
const addExpires = ref('') // 空=永不过期

// 明文一次性展示状态
const plaintext = ref('')
const plaintextName = ref('')
const clipboardTimer: ReturnType<typeof setTimeout>[] = []

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    tokens.value = (await api.listTokens()).tokens
  } catch (e) {
    error.value = errorMessage(e)
  } finally {
    loading.value = false
  }
}

function statusBadge(s: TokenDto['status']): string {
  return s === 'active'
    ? 'bg-emerald-100 text-emerald-800'
    : s === 'expired'
      ? 'bg-amber-100 text-amber-800'
      : 'bg-zinc-100 text-zinc-600'
}

async function createToken(): Promise<void> {
  error.value = ''
  try {
    const r = await api.createToken({
      name: addName.value,
      ...(addExpires.value ? { expires_days: Number(addExpires.value) } : {}),
    })
    showAdd.value = false
    addName.value = ''
    addExpires.value = ''
    // 明文一次性：关闭对话框即销毁引用（JS 字符串不可变——不落盘不进日志为可达承诺）
    plaintext.value = r.token
    plaintextName.value = r.name
    await refresh()
  } catch (e) {
    error.value = errorMessage(e)
  }
}

/** 复制并 60s 后自动清空剪贴板（R7/F9：防 Win+V 历史/云同步长期留存）。 */
async function copyPlaintext(): Promise<void> {
  const value = plaintext.value
  await navigator.clipboard.writeText(value)
  clipboardTimer.push(
    setTimeout(async () => {
      try {
        if ((await navigator.clipboard.readText()) === value) await navigator.clipboard.writeText('')
      } catch {
        // 无剪贴板读权限时静默（写入的自动清空尽力而为）
      }
    }, 60_000),
  )
}

function closePlaintext(): void {
  plaintext.value = ''
  plaintextName.value = ''
}

const revokeTarget = ref<TokenDto | null>(null)
async function doRevoke(): Promise<void> {
  if (!revokeTarget.value) return
  error.value = ''
  try {
    await api.revokeToken(revokeTarget.value.id)
    revokeTarget.value = null
    await refresh()
  } catch (e) {
    error.value = errorMessage(e) // admin 行禁撤 → cannot revoke admin 透出
    revokeTarget.value = null
  }
}

onMounted(refresh)
</script>

<template>
  <div>
    <div class="mb-4 flex items-center justify-between">
      <h1 class="text-xl font-semibold">Tokens</h1>
      <Button @click="showAdd = true">创建 token</Button>
    </div>

    <p v-if="error" class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ error }}</p>

    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>名称</TableHead>
          <TableHead>状态</TableHead>
          <TableHead>创建于</TableHead>
          <TableHead>过期</TableHead>
          <TableHead>最后使用</TableHead>
          <TableHead class="text-right">操作</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="t in tokens" :key="t.id">
          <TableCell class="font-medium">{{ t.name }}</TableCell>
          <TableCell><Badge :class="statusBadge(t.status)">{{ t.status }}</Badge></TableCell>
          <TableCell>{{ new Date(t.created_at * 1000).toLocaleDateString() }}</TableCell>
          <TableCell>{{ t.expires_at ? new Date(t.expires_at * 1000).toLocaleDateString() : '永不' }}</TableCell>
          <TableCell>{{ t.last_used_at ? new Date(t.last_used_at * 1000).toLocaleString() : '—' }}</TableCell>
          <TableCell class="text-right">
            <Button
              v-if="t.status === 'active' && t.name !== '__admin__'"
              variant="destructive"
              size="sm"
              @click="revokeTarget = t"
            >
              撤销
            </Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <!-- 创建对话框 -->
    <Dialog :open="showAdd" @update:open="showAdd = $event">
      <DialogContent class="max-w-md">
        <DialogHeader>
          <DialogTitle>创建 token</DialogTitle>
        </DialogHeader>
        <div class="space-y-3">
          <div class="space-y-1">
            <Label for="token-name">名称</Label>
            <Input id="token-name" v-model="addName" placeholder="如 my-laptop" />
          </div>
          <div class="space-y-1">
            <Label for="token-exp">有效期（天，空=永不过期）</Label>
            <Input id="token-exp" v-model="addExpires" type="number" min="1" placeholder="30" />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="showAdd = false">取消</Button>
          <Button :disabled="!addName" @click="createToken">创建</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 明文一次性展示 -->
    <Dialog :open="plaintext !== ''" @update:open="(v: boolean) => !v && closePlaintext()">
      <DialogContent class="max-w-lg">
        <DialogHeader>
          <DialogTitle>token「{{ plaintextName }}」已创建</DialogTitle>
        </DialogHeader>
        <p class="text-sm font-medium text-destructive">明文仅此一次展示，关闭后不可再看。请立即复制保存。</p>
        <code class="block break-all rounded-md bg-zinc-100 p-3 text-xs dark:bg-zinc-800">{{ plaintext }}</code>
        <DialogFooter>
          <Button variant="outline" @click="closePlaintext">我已保存，关闭</Button>
          <Button @click="copyPlaintext">复制（60s 后剪贴板自动清空）</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 撤销确认 -->
    <Dialog :open="revokeTarget !== null" @update:open="revokeTarget = null">
      <DialogContent class="max-w-sm">
        <DialogHeader>
          <DialogTitle>撤销 token「{{ revokeTarget?.name }}」？</DialogTitle>
        </DialogHeader>
        <p class="text-sm text-muted-foreground">撤销即时生效，使用该 token 的客户端将收到 401。</p>
        <DialogFooter>
          <Button variant="outline" @click="revokeTarget = null">取消</Button>
          <Button variant="destructive" @click="doRevoke">确认撤销</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
