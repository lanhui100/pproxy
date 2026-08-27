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
// 代际号：releaseAll/新复制使在途 write 作废，杜绝「关闭后定时器复活」竞态（R2-SEC-2）
let epoch = 0

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
  const my = ++epoch
  try {
    await deps.write(text)
  } catch (e) {
    // 写失败：旧值若仍在剪贴板则重建其清理任务（场景 B：取消与重设间无原子性）
    if (lastValue) void tryClear(lastValue)
    throw e
  }
  if (my !== epoch) {
    // 在途期间发生 releaseAll/新复制：作废本次结果，立即尽力擦除刚写入的值
    void tryClear(text)
    return
  }
  lastValue = text
  opts?.onCopied?.()
  clearTimer = setTimeout(() => {
    clearTimer = null
    void tryClear(lastValue)
  }, CLEAR_DELAY_MS)
}

/**
 * 释放本地暂存与取消待清定时器。
 * 供弹窗关闭/组件销毁时调用：解除本地引用，保留用户粘贴窗口。
 */
export function releaseAll(): void {
  epoch++
  if (clearTimer) {
    clearTimeout(clearTimer)
    clearTimer = null
  }
  lastValue = ''
}

/**
 * 主动安全擦除（仅在显式退出登录或敏感销毁时调用）。
 */
export function forcePurgeClipboard(): void {
  const value = lastValue
  releaseAll()
  if (value) void tryClear(value)
}
