// 应用自更新（M5 拓展，用户裁决）：tauri-plugin-updater + process。
// 模块级单例状态：App 侧栏角标与 Settings 卡片共享同一份检测结果。
// 浏览器/dev 环境无 updater——全部方法 no-op 守卫。
import { ref } from 'vue'

import { isTauri } from '@/lib/config'

export const updateAvailable = ref(false)
export const updateVersion = ref('')
export const updateNotes = ref('')
export const updateError = ref('')
export const downloading = ref(false)
export const downloadProgress = ref(0) // 0-100；NSIS passive 模式另有系统 UI
export const downloaded = ref(false) // 下载完成待重启

/** 检查更新：静默失败（badge 不亮即无更新/检查失败，错误仅记录）。 */
export async function checkForUpdate(): Promise<void> {
  if (!isTauri()) return
  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    const u = await check()
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
  }
}

/** 下载并安装（NSIS passive 模式自动接管安装 UI），完成后重启应用。 */
export async function downloadAndInstall(): Promise<void> {
  if (!isTauri() || !updateAvailable.value || downloading.value) return
  downloading.value = true
  downloaded.value = false
  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    const u = await check() // 重新获取句柄（插件要求）
    if (!u) return
    let total = 0
    let received = 0
    await u.downloadAndInstall((event) => {
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
          break
      }
    })
    downloaded.value = true
    // passive 安装完成后进程由安装器接管，此处 relaunch 兜底新版本启动
    const { relaunch } = await import('@tauri-apps/plugin-process')
    await relaunch()
  } catch (e) {
    updateError.value = String(e)
  } finally {
    downloading.value = false
  }
}
