// 应用自更新（M5 拓展，用户裁决）：tauri-plugin-updater + process。
// 模块级单例状态：App 侧栏角标与 Settings 卡片共享同一份检测结果。
// 浏览器/dev 环境无 updater——全部方法 no-op 守卫。
//
// 系统通道（2026-08）：代理加速引擎运行时，检查/下载统一走本地代理
// 127.0.0.1:18900——GitHub 下载域经引擎隧道出网，保证国内稳定拉包；
// 引擎未开启则直连（与系统网络一致）。
import { ref } from 'vue'

import { isTauri } from '@/lib/config'

const ENGINE_PROXY = 'http://127.0.0.1:18900'

export const currentVersion = ref('0.3.38')
export const checking = ref(false)
export const updateAvailable = ref(false)
export const updateVersion = ref('')
export const updateNotes = ref('')
export const updateError = ref('')
export const downloading = ref(false)
export const downloadProgress = ref(0) // 0-100；NSIS passive 模式另有系统 UI
export const downloaded = ref(false) // 下载完成待重启

/** 初始化当前客户端版本号 */
export async function initCurrentVersion(): Promise<void> {
  if (!isTauri()) return
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    currentVersion.value = await getVersion()
  } catch {
    // 降级使用静态默认值
  }
}

/** 引擎是否在跑：在跑则返回本地代理地址（下载经系统加速通道），否则 null（直连）。 */
async function engineProxy(): Promise<string | null> {
  if (!isTauri()) return null
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const st = await invoke<{ engine_running: boolean }>('proxy_status')
    return st.engine_running ? ENGINE_PROXY : null
  } catch {
    return null // 查询失败按直连处理，不阻塞更新主流程
  }
}

/** 检查更新：静默失败（badge 不亮即无更新/检查失败，错误仅记录）。 */
export async function checkForUpdate(): Promise<void> {
  if (!isTauri() || checking.value) return
  checking.value = true
  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    const proxy = await engineProxy()
    // 优先尝试使用 proxy（如果引擎运行中），若请求失败或报错，自动 fallback 到直连检查，
    // 避免因本地代理端口或本地路由异常导致更新检查中断
    let u = null
    try {
      u = await check(proxy ? { proxy } : undefined)
    } catch (firstErr) {
      if (proxy) {
        // Fallback 到直连无 proxy 重试一次
        u = await check(undefined)
      } else {
        throw firstErr
      }
    }
    if (u) {
      updateAvailable.value = true
      updateVersion.value = u.version
      updateNotes.value = u.body ?? ''
      updateError.value = ''
    } else {
      updateAvailable.value = false
      updateVersion.value = ''
      updateNotes.value = ''
    }
  } catch (e) {
    updateError.value = String(e)
  } finally {
    checking.value = false
  }
}

/** 下载并安装（NSIS passive 模式自动接管安装 UI），完成后重启应用。 */
export async function downloadAndInstall(): Promise<void> {
  if (!isTauri() || !updateAvailable.value || downloading.value) return
  downloading.value = true
  downloaded.value = false
  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    // 重新获取句柄（插件要求）；同样走系统通道，支持 fallback
    const proxy = await engineProxy()
    let u = null
    try {
      u = await check(proxy ? { proxy } : undefined)
    } catch (firstErr) {
      if (proxy) {
        u = await check(undefined)
      } else {
        throw firstErr
      }
    }
    if (!u) return
    let total = 0
    let received = 0
    await u.downloadAndInstall(async (event) => {
      switch (event.event) {
        case 'Started':
          total = event.data.contentLength ?? 0
          break
        case 'Progress':
          received += event.data.chunkLength
          downloadProgress.value = total > 0 ? Math.min(100, Math.round((received / total) * 100)) : 0
          break
        case 'Finished':
          downloadProgress.value = 100
          // 标记下载完成，进入安装并重启阶段
          downloaded.value = true
          // 在 Windows 安装器唤起退出前，提前通知 Rust 端清理锁文件和系统代理
          try {
            const { invoke } = await import('@tauri-apps/api/core')
            await invoke('proxy_prepare_update_exit')
          } catch {
            // 兜底忽略
          }
          break
      }
    })
    // 正常情况下 Windows 下 u.downloadAndInstall 唤起安装包后 Rust 侧会立即执行 exit(0)
    // 若在其他平台或未立即退出，调用 relaunch 兜底拉起
    downloaded.value = true
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process')
      await relaunch()
    } catch {
      // 忽略
    }
  } catch (e) {
    updateError.value = String(e)
  } finally {
    downloading.value = false
  }
}
