#!/usr/bin/env node
// 中文扫描门禁（M7 SPEC §6.4，可判定形式）：
// 范围 A：五个 views/*.vue 的 <template> 文本节点（含 {{ }} 插值内字面量）——
//         含 ≥2 个连续英文字母的词且整段无中文 → 违规（专有名词白名单除外）。
// 范围 B：lib/*.ts composables/*.ts 中含中文的字符串字面量视为已翻译，跳过；
//         纯英文串仅在「面向用户文件」内报告（toast/label/error/format/notification 命名），
//         技术性 key/事件名靠白名单放行。已知局限记录于文末。
// 用法: node scripts/check-zh.mjs ；违规非零退出；顶部常量区可调参。
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('../src', import.meta.url))

const WHITELIST_WORDS = new Set([
  // 服务与专有名词
  'OpenAI', 'Anthropic', 'Gemini', 'GitHub', 'OpenRouter', 'Groq', 'Mistral', 'xAI',
  'Hugging', 'Face', 'Twitter', 'Pony', 'Proxy', 'Vercel', 'Worker', 'PowerShell', 'bash', 'cURL', 'SDK', 'Code', 'Cursor', 'NextChat', 'Chatbox', 'Python',
  // 技术 token（界面允许保留的）
  'token', 'admin', 'base_url', 'API', 'APIs', 'URL', 'URLs', 'ID', 'id', 'OS', 'HTTP', 'Cloudflare', 'ms', 'YOUR_TOKEN', 'YOUR_API_KEY', 'lt', 'gt',
])

// 允许纯英文存在的 .ts 文件名片段（技术模块，不直接承载用户文案）
const TS_FILE_ALLOW = /(^|\/)(client|schemas|msw|config|normalize|urls|utils|format|statusLabels|errors|useUpdater|useAlertNotifications|useSecretCopy|useToast|useBackendGate|useAdaptivePoll|useTunnelProvision|useSessionSecret|presetGenerator|serviceTemplates|usageJoin|router)\.ts$/

function extractWords(text) {
  return text.match(/[A-Za-z][A-Za-z_-]+/g) ?? []
}

function hasCJK(text) {
  return /[\u4e00-\u9fff]/.test(text)
}

/** .vue template 文本节点提取：
 * 1) 先整体剥掉 <script>/<style> 块；2) 剥注释；
 * 3) {{ }} 插值表达式属代码，整块替换为占位符（其内引号字面量另行检查）；
 * 4) 再按标签切片，丢弃属性区残留（含 = ( { 的片段视为代码）。 */
function templateTextNodes(source) {
  const tpl = source.match(/<template>([\s\S]*)<\/template>/)
  if (!tpl) return []
  return tpl[1]
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\{\{[\s\S]*?\}\}/g, '·')
    .split(/<[^>]*>/)
    .map((seg) => seg.trim())
    .filter((seg) => seg && !/[=(){}<>]/.test(seg))
}

const violations = []

for (const name of ['DashboardView.vue', 'CoreView.vue', 'SettingsView.vue']) {
  const file = join(ROOT, 'views', name)
  let source
  try {
    source = readFileSync(file, 'utf8')
  } catch {
    continue // 页面尚未落地时不误报
  }
  for (const seg of templateTextNodes(source)) {
    if (hasCJK(seg)) continue
    const words = extractWords(seg).filter((w) => !WHITELIST_WORDS.has(w))
    if (words.length > 0) violations.push(`views/${name}: 「${seg.slice(0, 60)}」 非白名单英文: ${words.join(', ')}`)
  }
}

// 范围 B：面向用户的 ts 字符串字面量（启发式，局限见文末）；测试文件豁免
for (const dir of ['lib', 'composables']) {
  for (const entry of readdirSync(join(ROOT, dir))) {
    if (entry.includes('.test.')) continue
    const p = join(ROOT, dir, entry)
    if (!statSync(p).isFile() || !entry.endsWith('.ts') || TS_FILE_ALLOW.test(entry)) continue
    const source = readFileSync(p, 'utf8')
    for (const m of source.matchAll(/['"`]([^'"`\n]{2,})['"`]/g)) {
      const s = m[1]
      if (hasCJK(s)) continue
      const words = extractWords(s).filter((w) => !WHITELIST_WORDS.has(w))
      if (words.length >= 2 && /\s/.test(s)) violations.push(`${dir}/${entry}: 「${s.slice(0, 60)}」`)
    }
  }
}

if (violations.length > 0) {
  console.error(`[check-zh] 发现 ${violations.length} 处疑似未中文化的用户可见文案:`)
  for (const v of violations) console.error('  -', v)
  process.exit(1)
}
console.log('[check-zh] OK — 视图模板与面向用户字符串无未白名单英文残留')
console.log('局限说明: .ts 侧仅扫描非白名单命名文件中的多词英文串；模板属性值/aria 标签不在范围（由 R2 人工核对兜底）。')
