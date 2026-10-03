<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { Minus, Square, Copy, X } from '@lucide/vue'
import { isTauri } from '@/lib/config'

const isMaximized = ref(false)

async function getAppWindow() {
  if (!isTauri()) return null
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    return getCurrentWindow()
  } catch {
    return null
  }
}

async function updateMaximizedState() {
  const win = await getAppWindow()
  if (win) {
    try {
      isMaximized.value = await win.isMaximized()
    } catch {}
  }
}

async function handleMinimize() {
  const win = await getAppWindow()
  if (win) {
    try {
      await win.minimize()
    } catch (e) {
      console.error('Failed to minimize window:', e)
    }
  }
}

async function handleToggleMaximize() {
  const win = await getAppWindow()
  if (win) {
    try {
      await win.toggleMaximize()
      await updateMaximizedState()
    } catch (e) {
      console.error('Failed to toggle maximize window:', e)
    }
  }
}

async function handleClose() {
  const win = await getAppWindow()
  if (win) {
    try {
      await win.close()
    } catch (e) {
      console.error('Failed to close window:', e)
    }
  }
}

let unlistenResize: (() => void) | null = null

onMounted(async () => {
  if (isTauri()) {
    await updateMaximizedState()
    const win = await getAppWindow()
    if (win) {
      try {
        unlistenResize = await win.onResized(() => {
          void updateMaximizedState()
        })
      } catch {}
    }
  }
})

onUnmounted(() => {
  if (unlistenResize) {
    unlistenResize()
  }
})
</script>

<template>
  <header
    data-tauri-drag-region
    class="titlebar-frosted h-9 w-full shrink-0 flex items-center justify-between px-3 select-none z-50 border-b border-border/40 bg-background/80 dark:bg-background/80 backdrop-blur-xl"
  >
    <!-- 左侧：图标与应用标识 -->
    <div data-tauri-drag-region class="flex items-center gap-2 pointer-events-none">
      <div class="size-4.5 rounded-md bg-primary flex items-center justify-center text-primary-foreground shadow-xs">
        <svg
          class="size-3 fill-current"
          viewBox="0 0 24 24"
        >
          <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
        </svg>
      </div>
      <span class="text-xs font-medium tracking-tight text-foreground/90">Pony Proxy</span>
    </div>

    <!-- 中间：拖拽占位区域 -->
    <div data-tauri-drag-region class="flex-1 h-full" />

    <!-- 右侧：原生窗口操作按钮组 -->
    <div class="flex items-center -mr-1.5 h-full pointer-events-auto">
      <button
        type="button"
        class="inline-flex size-8 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted/80 hover:text-foreground transition-colors duration-150"
        title="最小化"
        aria-label="最小化"
        @click="handleMinimize"
      >
        <Minus class="size-3.5" />
      </button>

      <button
        type="button"
        class="inline-flex size-8 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted/80 hover:text-foreground transition-colors duration-150"
        :title="isMaximized ? '还原' : '最大化'"
        :aria-label="isMaximized ? '还原' : '最大化'"
        @click="handleToggleMaximize"
      >
        <Copy v-if="isMaximized" class="size-3" />
        <Square v-else class="size-3" />
      </button>

      <button
        type="button"
        class="inline-flex size-8 items-center justify-center rounded-sm text-muted-foreground hover:bg-red-500 hover:text-white transition-colors duration-150"
        title="关闭"
        aria-label="关闭"
        @click="handleClose"
      >
        <X class="size-3.5" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.titlebar-frosted {
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  backdrop-filter: blur(20px) saturate(180%);
}
</style>
