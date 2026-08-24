// 秘密复制唯一入口（SPEC §3.2-F2 安全纪律）：
// - 新复制先取消旧的 60s 自清定时器再重设，杜绝多路并发清理
// - releaseAll()：对话框关闭时调用，取消全部待清任务并尽力清一次剪贴板
// - toast 提示由调用方经 onCopied 回调给出（本模块不弹 UI、不落盘、不进日志）
type ClipboardDeps = {
  write: (text: string) => Promise<void>
  read: () => Promise<string>
}

const CLEAR_DELAY_MS = 60_000

let deps: ClipboardDeps = {
  write: (text) => navigator.clipboard.writeText(text),
  read: () => navigator.clipboard.readText(),
}

/** 测试注入点：node 环境无 clipboard。 */
export function _setClipboardDepsForTest(patch: Partial<ClipboardDeps>): void {
  deps = { ...deps, ...patch }
}

let clearTimer: ReturnType<typeof setTimeout> | null = null
let lastValue = ''

/** 尽力而为清剪贴板：仅当内容仍是我们的值时才写空（防误伤用户后复制的内容）。 */
async function tryClear(value: string): Promise<void> {
  try {
    if ((await deps.read()) === value) await deps.write('')
  } catch {
    // 无读权限时静默
  }
}

export async function copySecret(text: string, opts?: { onCopied?: () => void }): Promise<void> {
  if (clearTimer) clearTimeout(clearTimer)
  await deps.write(text)
  lastValue = text
  opts?.onCopied?.()
  clearTimer = setTimeout(() => {
    clearTimer = null
    void tryClear(lastValue)
  }, CLEAR_DELAY_MS)
}

export function releaseAll(): void {
  if (clearTimer) {
    clearTimeout(clearTimer)
    clearTimer = null
  }
  const value = lastValue
  lastValue = ''
  if (value) void tryClear(value)
}
