<script setup lang="ts">
// 设备密钥（M7 SPEC §3.5）：列表五态 + 创建（有效期档位 chips）+
// 接入配置弹窗（打开即自动复制 / Tab v-if 切换 / 环境变量示例双版本 /
// 关闭防护：未复制先二次确认）/ 撤销确认。
// 安全纪律：明文仅存于本组件临时 ref；一切复制走 useSecretCopy 唯一入口；
// 任何路径关闭接入弹窗都 releaseAll() 并清空本地明文引用。
import { onMounted, ref } from 'vue'

import { LoaderCircle } from '@lucide/vue'

import { api, type TokenDto } from '@/api/client'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import SkeletonTable from '@/components/common/SkeletonTable.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { copySecret, releaseAll } from '@/composables/useSecretCopy'
import { useToast } from '@/composables/useToast'
import { loadBackendUrl, loadDataPlaneUrl } from '@/lib/config'
import { errText } from '@/lib/errors'
import { resolveExpiresDays, type ExpiryMode } from '@/lib/expiry'
import { fmtDate, fmtRelative } from '@/lib/format'
import { tokenStatusLabel } from '@/lib/statusLabels'
import { deriveDataPlane } from '@/lib/urls'

const toast = useToast()

// ---- 列表状态 ----
const tokens = ref<TokenDto[]>([])
const loading = ref(false)
const loadError = ref('')

async function refresh(): Promise<void> {
  loading.value = true
  loadError.value = ''
  try {
    tokens.value = (await api.listTokens()).tokens
  } catch (e) {
    loadError.value = errText(e)
  } finally {
    loading.value = false
  }
}

onMounted(refresh)

function isAdminRow(t: TokenDto): boolean {
  return t.name === '__admin__'
}

/** 行名称展示：内置管理员行不直出原始名（SPEC §3.5）。 */
function rowName(t: TokenDto): string {
  return isAdminRow(t) ? '系统管理员（内置）' : t.name
}

// ---- 创建对话框 ----
const showCreate = ref(false)
const creating = ref(false)
const createName = ref('')
const createError = ref('')
const expiryMode = ref<ExpiryMode>('never')
const customDays = ref('')

const EXPIRY_CHOICES: { key: ExpiryMode; label: string }[] = [
  { key: 'never', label: '永久' },
  { key: 'd30', label: '30 天' },
  { key: 'd90', label: '90 天' },
  { key: 'custom', label: '自定义' },
]

function openCreate(): void {
  createError.value = ''
  createName.value = ''
  expiryMode.value = 'never'
  customDays.value = ''
  showCreate.value = true
}

/** 创建中忽略 Esc/遮罩关闭（UX-7③）：busy 态下 @update:open 的 false 直接忽略。 */
function onCreateOpenChange(v: boolean): void {
  if (!v && creating.value) return
  showCreate.value = v
}

async function createToken(): Promise<void> {
  createError.value = ''
  // 档位 → expires_days 走 lib/expiry 唯一出口（ENG-1）：
  // 服务端缺 expires_days 键 = 永不过期，故除 never 外必须显式携带天数
  let expiresDays: number | undefined
  try {
    expiresDays = resolveExpiresDays(expiryMode.value, Number(customDays.value))
  } catch {
    createError.value = '有效期不合法：需为正整数天数'
    return
  }
  creating.value = true
  try {
    const r = await api.createToken({
      name: createName.value,
      ...(expiresDays !== undefined ? { expires_days: expiresDays } : {}),
    })
    showCreate.value = false
    await refresh()
    openConfig(r.token, r.name) // 创建成功 → 接入配置弹窗（打开即自动复制）
  } catch (e) {
    // 错误渲染在 Dialog 内部错误区：Dialog 不关、表单不清空
    createError.value = errText(e)
  } finally {
    creating.value = false
  }
}

// ---- 接入配置弹窗（明文一次性）----
const showConfig = ref(false)
const confirmDiscard = ref(false)
const copied = ref(false) // 是否已成功复制过任一秘密片段（关闭防护判据）
const activeTab = ref<'general' | 'anthropic' | 'openai'>('general')
const tokenPlain = ref('') // 本地明文引用：关闭即清空
const configName = ref('')
const dataBase = ref('') // 打开时定格的数据面底座

interface TabMeta {
  key: 'general' | 'anthropic' | 'openai'
  label: string
  /** URL 里的路由段；通用 Tab 用占位符 */
  segment: string
  /** 环境变量示例的变量名（general 无标准约定；SEC-6 起该 Tab 仅注释式示意、不消费此字段） */
  envVar: string
  envNote: string
}

const TABS: TabMeta[] = [
  { key: 'general', label: '通用', segment: '<服务>', envVar: 'BASE_URL', envNote: '无标准环境变量约定，按所用 SDK 替换变量名' },
  { key: 'anthropic', label: 'Anthropic', segment: 'anthropic', envVar: 'ANTHROPIC_BASE_URL', envNote: '' },
  { key: 'openai', label: 'OpenAI', segment: 'openai', envVar: 'OPENAI_BASE_URL', envNote: '' },
]

function tabMeta(key: TabMeta['key']): TabMeta {
  return TABS.find((t) => t.key === key) ?? TABS[0]
}

/** 当前 Tab 的接入 base_url：底座为空返回空串（片段区显示未配置提示而非拼坏链）。 */
function currentUrl(key: TabMeta['key']): string {
  if (!dataBase.value || !tokenPlain.value) return ''
  return `${dataBase.value}/${tokenPlain.value}/${tabMeta(key).segment}`
}

/** bash 版环境变量示例（措辞与 crates/cli/src/export.rs 对齐）。 */
function bashSnippet(key: TabMeta['key']): string {
  const m = tabMeta(key)
  const url = currentUrl(key)
  const head = m.key === 'general' ? `# pony proxy — ${m.segment}（${m.envNote}）` : `# pony proxy — ${m.segment}`
  if (m.key === 'general') {
    // SEC-6：BASE_URL 属前端发明变量名，对齐 CLI CommentOnly 分支改注释式示例
    return `${head}\n# base_url = ${url}（按所用 SDK 替换变量名）\n# api_key = <你的上游密钥>\n`
  }
  return `${head}\nexport ${m.envVar}=${url}\nexport ${m.envVar.replace('BASE_URL', 'API_KEY')}=<你的上游密钥>\n`
}

/** PowerShell 版环境变量示例（与 bash 版同信息量，$env: 赋值加引号）。 */
function psSnippet(key: TabMeta['key']): string {
  const m = tabMeta(key)
  const url = currentUrl(key)
  const head = m.key === 'general' ? `# pony proxy — ${m.segment}（${m.envNote}）` : `# pony proxy — ${m.segment}`
  if (m.key === 'general') {
    // SEC-6：同 bash 版，注释式示意 $env: 用法，不发明具体变量名
    return `${head}\n# $env:你的变量名 = "${url}"\n# $env:你的密钥变量名 = "<你的上游密钥>"\n`
  }
  return `${head}\n$env:${m.envVar}="${url}"\n$env:${m.envVar.replace('BASE_URL', 'API_KEY')}="<你的上游密钥>"\n`
}

/** 秘密复制唯一入口包装：toast 文案固定，成功即记 copied=true（关闭防护放行）。 */
async function copyWithToast(text: string): Promise<void> {
  try {
    await copySecret(text, {
      onCopied() {
        copied.value = true
        toast.success('已复制（60 秒后自动清空剪贴板）')
      },
    })
  } catch {
    toast.error('自动复制失败，请手动选择文本复制')
  }
}

function openConfig(token: string, name: string): void {
  tokenPlain.value = token
  configName.value = name
  // 接入底座一律：显式数据面地址优先，否则按管理面推导（F2 底座）
  dataBase.value = loadDataPlaneUrl() || deriveDataPlane(loadBackendUrl()) || ''
  activeTab.value = 'general'
  copied.value = false
  confirmDiscard.value = false
  showConfig.value = true
  // 打开即自动复制明文密钥（UX-08/SEC-4）
  void copyWithToast(token)
}

function closeConfig(): void {
  // 任何路径关闭都要：取消全部待清定时器并尽力清一次剪贴板 + 清空本地明文引用
  releaseAll()
  tokenPlain.value = ''
  configName.value = ''
  dataBase.value = ''
  copied.value = false
  confirmDiscard.value = false
  showConfig.value = false
}

/** 关闭请求统一入口：尚未复制过则拦截并弹二次确认。 */
function requestClose(): void {
  if (copied.value) {
    closeConfig()
  } else {
    confirmDiscard.value = true
  }
}

function onConfigOpenChange(v: boolean): void {
  if (!v) requestClose() // Esc/遮罩/X 按钮全部汇入防护
}

// ---- 撤销确认 ----
const revokeTarget = ref<TokenDto | null>(null)
const revoking = ref(false)

function askRevoke(t: TokenDto): void {
  revokeTarget.value = t
}

async function doRevoke(): Promise<void> {
  if (!revokeTarget.value) return
  revoking.value = true
  try {
    await api.revokeToken(revokeTarget.value.id)
    revokeTarget.value = null
    await refresh()
  } catch (e) {
    // admin 行禁撤等错误经 errText 译中文；失败统一 toast（UX-5 / SPEC §2-5 一切失败必 toast）
    toast.error(errText(e))
    revokeTarget.value = null
  } finally {
    revoking.value = false
  }
}
</script>

<template>
  <div>
    <PageHeader title="设备密钥" subtitle="即 token · 为每台设备发一把钥匙">
      <template #actions>
        <Button @click="openCreate">创建</Button>
      </template>
    </PageHeader>

    <!-- 加载骨架 -->
    <SkeletonTable v-if="loading && tokens.length === 0" :rows="6" />

    <!-- 加载失败（可重试） -->
    <div v-else-if="loadError" class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">
      {{ loadError }}
      <Button variant="outline" size="sm" class="ml-2" @click="refresh">重试</Button>
    </div>

    <!-- 空态（CTA 下一步）：EmptyState 仅渲染具名插槽 #actions -->
    <EmptyState
      v-else-if="tokens.length === 0"
      title="还没有设备密钥"
      description="为每台设备发一把独立钥匙，随时可单独撤销而不影响其他设备。"
    >
      <template #actions>
        <Button @click="openCreate">创建第一个设备密钥</Button>
      </template>
    </EmptyState>

    <!-- 列表 -->
    <div v-else class="overflow-hidden rounded-lg border">
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
            <TableCell class="font-medium">{{ rowName(t) }}</TableCell>
            <TableCell><StatusDot v-bind="tokenStatusLabel(t.status)" /></TableCell>
            <TableCell class="text-muted-foreground">{{ fmtDate(t.created_at * 1000) }}</TableCell>
            <TableCell class="text-muted-foreground">{{ t.expires_at ? fmtDate(t.expires_at * 1000) : '永不过期' }}</TableCell>
            <TableCell class="text-muted-foreground">{{ t.last_used_at ? fmtRelative(t.last_used_at * 1000) : '—' }}</TableCell>
            <TableCell class="text-right">
              <span v-if="isAdminRow(t)" class="text-muted-foreground">—</span>
              <Button v-else variant="destructive" size="sm" @click="askRevoke(t)">撤销</Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <!-- 创建对话框：错误在内部渲染，失败不关不清；busy 中 Esc/遮罩不可关（UX-7③） -->
    <Dialog :open="showCreate" @update:open="onCreateOpenChange">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>创建设备密钥</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="space-y-1">
            <Label for="token-name">名称</Label>
            <Input id="token-name" v-model="createName" placeholder="my-laptop" />
            <p class="text-xs text-muted-foreground">建议用设备名命名，如 my-laptop</p>
          </div>
          <div class="space-y-1">
            <Label>有效期</Label>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="c in EXPIRY_CHOICES"
                :key="c.key"
                type="button"
                class="rounded-full border px-3 py-1 text-sm transition-colors"
                :class="
                  expiryMode === c.key
                    ? 'border-primary bg-primary/10 font-medium text-primary'
                    : 'border-border text-muted-foreground hover:bg-muted'
                "
                :aria-pressed="expiryMode === c.key"
                @click="expiryMode = c.key"
              >
                {{ c.label }}
              </button>
            </div>
            <Input
              v-if="expiryMode === 'custom'"
              v-model="customDays"
              type="number"
              min="1"
              placeholder="天数（如 180）"
              class="mt-2 w-40"
            />
          </div>
          <p v-if="createError" class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800">{{ createError }}</p>
        </div>
        <DialogFooter>
          <!-- UX-8：禁用不静默，灰字说明缺什么 -->
          <span v-if="!createName.trim()" class="mr-auto self-center text-xs text-muted-foreground">需填写名称后可创建</span>
          <Button variant="outline" :disabled="creating" @click="showCreate = false">取消</Button>
          <Button :disabled="creating || !createName.trim()" data-icon="inline-start" @click="createToken">
            <LoaderCircle v-if="creating" class="animate-spin" />
            创建
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 接入配置弹窗（明文一次性）：Tab v-if 切换，DOM 同一时刻仅一个片段 -->
    <Dialog :open="showConfig" @update:open="onConfigOpenChange">
      <DialogContent class="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>接入配置 ·「{{ configName }}」已创建</DialogTitle>
        </DialogHeader>

        <p class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs font-medium text-red-800">
          明文仅此一次展示；片段含明文令牌，请勿截图外发
        </p>
        <!-- UX-4：自动复制了什么、去哪找，说清楚（成功复制后显示） -->
        <p v-if="copied" class="text-xs font-medium text-emerald-700">
          ✓ 密钥已自动复制到剪贴板（60 秒后自动清空）；下方地址请单独点『复制地址』
        </p>

        <!-- Tab 切换 chips -->
        <div class="flex flex-wrap gap-2">
          <button
            v-for="t in TABS"
            :key="t.key"
            type="button"
            class="rounded-full border px-3 py-1 text-sm transition-colors"
            :class="
              activeTab === t.key
                ? 'border-primary bg-primary/10 font-medium text-primary'
                : 'border-border text-muted-foreground hover:bg-muted'
            "
            :aria-pressed="activeTab === t.key"
            @click="activeTab = t.key"
          >
            {{ t.label }}
          </button>
        </div>

        <template v-if="activeTab === 'general'">
          <div v-if="!dataBase" class="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800">
            未配置数据面地址，请先到设置完成连接
          </div>
          <div v-else class="space-y-3">
            <div class="space-y-1">
              <Label>接入地址（把「&lt;服务&gt;」替换为实际路由名）</Label>
              <code class="block break-all rounded-md bg-muted p-3 font-mono text-sm">{{ currentUrl('general') }}</code>
            </div>
            <div class="flex items-center gap-2">
              <Button variant="outline" size="sm" @click="copyWithToast(currentUrl('general'))">复制地址</Button>
              <Button variant="outline" size="sm" @click="copyWithToast(tokenPlain)">复制密钥</Button>
            </div>
            <details class="rounded-md border px-3 py-2">
              <summary class="cursor-pointer text-sm font-medium">环境变量示例</summary>
              <div class="mt-2 space-y-2">
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">bash</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(bashSnippet('general'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ bashSnippet('general') }}</pre>
                </div>
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">PowerShell</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(psSnippet('general'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ psSnippet('general') }}</pre>
                </div>
              </div>
            </details>
          </div>
        </template>

        <template v-else-if="activeTab === 'anthropic'">
          <div v-if="!dataBase" class="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800">
            未配置数据面地址，请先到设置完成连接
          </div>
          <div v-else class="space-y-3">
            <div class="space-y-1">
              <Label>接入地址（供 Claude Code 等 Anthropic SDK 使用）</Label>
              <code class="block break-all rounded-md bg-muted p-3 font-mono text-sm">{{ currentUrl('anthropic') }}</code>
            </div>
            <div class="flex items-center gap-2">
              <Button variant="outline" size="sm" @click="copyWithToast(currentUrl('anthropic'))">复制地址</Button>
              <Button variant="outline" size="sm" @click="copyWithToast(tokenPlain)">复制密钥</Button>
            </div>
            <details class="rounded-md border px-3 py-2">
              <summary class="cursor-pointer text-sm font-medium">环境变量示例</summary>
              <div class="mt-2 space-y-2">
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">bash</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(bashSnippet('anthropic'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ bashSnippet('anthropic') }}</pre>
                </div>
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">PowerShell</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(psSnippet('anthropic'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ psSnippet('anthropic') }}</pre>
                </div>
              </div>
            </details>
          </div>
        </template>

        <template v-else>
          <div v-if="!dataBase" class="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800">
            未配置数据面地址，请先到设置完成连接
          </div>
          <div v-else class="space-y-3">
            <div class="space-y-1">
              <Label>接入地址（供 OpenAI SDK 使用）</Label>
              <code class="block break-all rounded-md bg-muted p-3 font-mono text-sm">{{ currentUrl('openai') }}</code>
            </div>
            <div class="flex items-center gap-2">
              <Button variant="outline" size="sm" @click="copyWithToast(currentUrl('openai'))">复制地址</Button>
              <Button variant="outline" size="sm" @click="copyWithToast(tokenPlain)">复制密钥</Button>
            </div>
            <details class="rounded-md border px-3 py-2">
              <summary class="cursor-pointer text-sm font-medium">环境变量示例</summary>
              <div class="mt-2 space-y-2">
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">bash</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(bashSnippet('openai'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ bashSnippet('openai') }}</pre>
                </div>
                <div class="space-y-1">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-medium text-muted-foreground">PowerShell</span>
                    <Button variant="ghost" size="xs" @click="copyWithToast(psSnippet('openai'))">复制</Button>
                  </div>
                  <pre class="overflow-x-auto rounded bg-muted p-2 text-xs">{{ psSnippet('openai') }}</pre>
                </div>
              </div>
            </details>
          </div>
        </template>

        <DialogFooter>
          <Button @click="requestClose">我已完成保存，关闭</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 关闭防护：未复制先拦截，二次确认后才真关 -->
    <ConfirmDialog
      :open="confirmDiscard"
      title="确定要关闭吗？"
      description="尚未复制，关闭后将无法再次查看，需要重新创建。"
      confirm-text="仍要关闭"
      destructive
      @update:open="(v: boolean) => !v && (confirmDiscard = false)"
      @confirm="closeConfig"
    />

    <!-- 撤销确认 -->
    <ConfirmDialog
      :open="revokeTarget !== null"
      :title="`撤销设备密钥「${revokeTarget ? rowName(revokeTarget) : ''}」？`"
      description="该设备的所有请求会立即失败，且无法恢复。"
      confirm-text="确认撤销"
      destructive
      :busy="revoking"
      @update:open="(v: boolean) => !v && (revokeTarget = null)"
      @confirm="doRevoke"
    />
  </div>
</template>
